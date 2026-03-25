use alloc::sync::Weak;
use lwext4_rust::{InodeTypes, Lwext4File};

use virtio_drivers::device::blk::VirtIOBlk;
use virtio_drivers::transport::mmio::{MmioTransport, VirtIOHeader};
use virtio_drivers::transport::{DeviceType, Transport};
use crate::alloc::string::ToString;
use crate::fs::vfs::dcache::GLOBAL_DCACHE;
use crate::fs::FS_MANAGER;
use crate::drivers::block::BLOCK_DEVICE;
use crate::fs::vfs::inode::Inode;

use alloc::vec;
use alloc::{format, vec::Vec};
use alloc::boxed::Box;
use crate::fs::vfs::path::split_parent_and_name;
use crate::fs::lwext4::inode::{Ext4Inode};
use crate::fs::lwext4::disk::Disk;
use crate::fs::vfs::inode::InodeType;
use crate::fs::vfs::file::File;
use crate::mm::UserBuffer;
use crate::sync::UPSafeCell;
use alloc::sync::Arc;
use bitflags::*;
use lazy_static::*;
use spin::Mutex;
use crate::fs::vfs::FileInner;
use crate::fs::vfs::Dentry;
use core::cell::RefMut;
use spin::MutexGuard;
use alloc::string::String;
use log::{warn,info};
use crate::fs::Ext4Dentry;
use lwext4_rust::bindings::SEEK_SET;
use lwext4_rust::bindings::{O_WRONLY,O_RDONLY,O_RDWR};
 use crate::fs::vfs::path::resolve_path;
///the Ext4File
pub struct Ext4File {
    readable: bool,
    writable: bool,
    inner:Mutex<FileInner>,
}

impl Ext4File {
    /// Construct an Ext4File from a Dentry
    pub fn new(readable: bool, writable: bool, dentry: Arc<dyn Dentry>, types: InodeTypes) -> Result<Self, i32> {
        let path = dentry.path();
        let mut file = Lwext4File::new(path.as_str(), types.clone());
        if types != InodeTypes::EXT4_DE_DIR {
            let open_flags = match (readable, writable) {
                (true, true) => O_RDWR,
                (false, true) => O_WRONLY,
                _ => O_RDONLY,
            };
            file.file_open(path.as_str(), open_flags)?;
        } else {
            info!("Opening a directory: {}, skipping ext4_fopen", path);
        }
        Ok(Self {
            readable,
            writable,
            inner: Mutex::new(FileInner { 
                offset: 0, 
                dentry, 
                ext4file: file 
            }),
        })
    }

    /// Read all data
    pub fn read_all(&self) -> Vec<u8> {
        let mut inner = self.inner.lock();
        let mut buffer = [0u8; 512];
        let mut v: Vec<u8> = Vec::new();    
        loop {
            let current_offset = inner.offset; 
            inner.ext4file.file_seek(current_offset as i64, SEEK_SET).expect("seek failed");
            let len = inner.ext4file.file_read(&mut buffer).unwrap();
            if len == 0 {
                break;
            }
            inner.offset += len;
            v.extend_from_slice(&buffer[..len]);
        }
        v
    }

    #[allow(unused)]
    /// Truncate the inode to the given size
    fn truncate(&self, size: u64) -> Result<usize, i32> {
        info!("truncate file to size={}", size);
        let mut inner = self.inner.lock();
        inner.ext4file.file_truncate(size)
    }
}


impl File for Ext4File {
    fn get_fileinner(&self)->MutexGuard<'_, FileInner> {
        self.inner.lock()
    }
    fn readable(&self) -> bool {
        self.readable
    }
    fn writable(&self) -> bool {
        self.writable
    }

    //read the data
    fn read(&self, mut buf: UserBuffer) -> usize {
        let mut inner = self.get_fileinner();
        let mut total_read_size = 0usize;
        for slice in buf.buffers.iter_mut() {
            let current_offset = inner.offset;
            inner.ext4file.file_seek(current_offset as i64, SEEK_SET).expect("seek failed");
            let read_size = inner.ext4file.file_read(*slice).unwrap();
            if read_size == 0 {
                break;
            }
            inner.offset += read_size;
            total_read_size += read_size;
        }
        total_read_size
    }

    fn write(&self, buf: UserBuffer) -> usize {
        info!("enter Ext4File write");
        let mut inner = self.inner.lock();
        let mut total_write_size = 0usize;
        for slice in buf.buffers.iter() {
            let current_offset = inner.offset;
            inner.ext4file.file_seek(current_offset as i64, SEEK_SET).expect("seek failed");
            let write_size = inner.ext4file.file_write(*slice).unwrap();
            assert_eq!(write_size, slice.len());
            inner.offset += write_size;
            total_write_size += write_size;
        }
        info!("finish Ext4File write");
        total_write_size
    }
    fn ls(&self) -> Vec<(String, u64, u8)> {
        self.get_fileinner().dentry.ls() 
    }
}


/// find the dentry by the absolute path, if can not find, return None
/// find from the root dentry, and fill the dcache when find the dentry, if can not find, return None
pub fn find_dentry(path: &str) -> Option<Arc<dyn Dentry>> {
    if let Some(cached) = GLOBAL_DCACHE.get(path) {
        return Some(cached);
    }
    let root_dentry = FS_MANAGER.exclusive_access().get("lwext4").unwrap().root();
    if path == "/" || path.is_empty() {
        GLOBAL_DCACHE.insert("/".to_string(), root_dentry.clone());
        return Some(root_dentry);
    }

    let mut current_dentry = root_dentry;
    let mut current_path = String::new();
    for part in path.split('/').filter(|s| !s.is_empty()) {
        current_path.push('/');
        current_path.push_str(part);

        if let Some(cached_parent) = GLOBAL_DCACHE.get(&current_path) {
            current_dentry = cached_parent;
            continue;
        }

        if let Some(next_dentry) = current_dentry.find(part) {
            GLOBAL_DCACHE.insert(current_path.clone(), next_dentry.clone());
            current_dentry = next_dentry;
        } else {
            return None; 
        }
    }
    Some(current_dentry)
}


#[allow(unused)]
/// path will be resolved to an absolute path, flags is the open flags
pub fn open_file(cwd: Arc<dyn Dentry>, path: &str, flags: OpenFlags) -> Option<Arc<Ext4File>> {
    let (readable, writable) = flags.read_write();
    let target_dentry = if flags.contains(OpenFlags::CREATE) {
        let (parent_path, name) = split_parent_and_name(path);
        let parent = resolve_path(cwd.clone(), parent_path.as_str())?;
        parent.find(name.as_str()).or_else(|| {
            parent.create(name.as_str(), InodeType::File)
        })?
    } else {
        resolve_path(cwd, path)?
    };
    let inode = target_dentry.get_inode()?;
    if flags.contains(OpenFlags::TRUNC) {
        inode.truncate(0).ok()?; 
    }
    Some(Arc::new(Ext4File::new(
        readable, 
        writable, 
        target_dentry, 
        inode.get_types()
    ).expect("...")))
}

bitflags! {
    ///Open file flags
    pub struct OpenFlags: u32 {
        ///Read only
        const RDONLY = 0;
        ///Write only
        const WRONLY = 1 << 0;
        ///Read & Write
        const RDWR = 1 << 1;
        ///Allow create
        const CREATE = 1 << 9;
        ///Clear file and return an empty one
        const TRUNC = 1 << 10;
    }
}

impl OpenFlags {
    /// Do not check validity for simplicity
    /// Return (readable, writable)
    pub fn read_write(&self) -> (bool, bool) {
        if self.is_empty() {
            (true, false)
        } else if self.contains(Self::WRONLY) {
            (false, true)
        } else {
            (true, true)
        }
    }
    /// Convert OpenFlags to ext4 open flags (O_RDONLY, O_WRONLY, O_RDWR)
    pub fn into_ext4_flags(&self) -> u32 {
        if self.contains(Self::RDWR) {
            O_RDWR
        } else if self.contains(Self::WRONLY) {
            O_WRONLY
        } else {
            O_RDONLY
        }
    }
}

// /// List all files in the filesystems
// pub fn list_apps() {
//     let root_dentry = FS_MANAGER.exclusive_access().get("lwext4").unwrap().root();
//     println!("/**** APPS ****");
//     for app in root_dentry.ls() {
//         println!("{}", app);
//     }
//     println!("**************/");
// }
