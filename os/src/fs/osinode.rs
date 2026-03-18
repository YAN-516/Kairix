//! (FileWrapper + VfsNodeOps) -> OSInodeInner
//! OSInodeInner -> OSInode
extern crate lwext4_rust;
extern crate virtio_drivers;

use alloc::sync::Weak;
use lwext4_rust::InodeTypes;

use virtio_drivers::device::blk::VirtIOBlk;
use virtio_drivers::transport::mmio::{MmioTransport, VirtIOHeader};
use virtio_drivers::transport::{DeviceType, Transport};


use crate::drivers::block::BLOCK_DEVICE;
use crate::fs::FS_MANAGER;
use crate::fs::vfs::vfs_ops::VfsInode;

use alloc::vec;
use alloc::{format, vec::Vec};
use alloc::boxed::Box;

use super::ext4fs::{Ext4Inode};

use super::disk::Disk;

use super::vfs::file::File;
use crate::mm::UserBuffer;
use crate::sync::UPSafeCell;
use alloc::sync::Arc;
use bitflags::*;
use lazy_static::*;



// lazy_static! {
//     /// ext4 file system
//     pub static ref EXT4_FS: Arc<Ext4FileSystem> = Arc::new(Ext4FileSystem::new(Disk::new(BLOCK_DEVICE.clone())));

//     /// root inode
//     pub static ref ROOT_INODE: Arc<dyn VfsInode> = EXT4_FS.root_dir();
// }

#[allow(unused)]
/// The OS inode inner in 'UPSafeCell'
pub struct OSInodeInner {
    offset: usize,
    inode: Arc<dyn VfsInode>,
    parent:Option<Weak<dyn VfsInode>>,
}

/// A wrapper around a filesystem inode
/// to implement File trait atop
pub struct OSInode {
    readable: bool,
    writable: bool,
    inner: UPSafeCell<OSInodeInner>,
}

impl OSInode {
    /// Construct an OS inode from a Inode
    pub fn new(readable: bool, writable: bool, inode: Arc<dyn VfsInode>) -> Self {
        Self {
            readable,
            writable,
            inner: unsafe { UPSafeCell::new(OSInodeInner { offset: 0, inode ,parent: None}) },
        }
    }

    /// Read all data inside a inode into vector
    pub fn read_all(&self) -> Vec<u8> {
        let mut inner = self.inner.exclusive_access();
        let mut buffer = [0u8; 512];
        let mut v: Vec<u8> = Vec::new();
        loop {
            let len = inner.inode.read_at(inner.offset, &mut buffer).unwrap();
            if len == 0 {
                break;
            }
            inner.offset += len;
            v.extend_from_slice(&buffer[..len]);
        }
        v
    }
}




impl File for OSInode {
    fn readable(&self) -> bool {
        self.readable
    }
    fn writable(&self) -> bool {
        self.writable
    }
    fn read(&self, mut buf: UserBuffer) -> usize {
        let mut inner = self.inner.exclusive_access();
        let mut total_read_size = 0usize;
        for slice in buf.buffers.iter_mut() {
            let read_size = inner.inode.read_at(inner.offset, *slice).unwrap();
            if read_size == 0 {
                break;
            }
            inner.offset += read_size;
            total_read_size += read_size;
        }
        total_read_size
    }
    fn write(&self, buf: UserBuffer) -> usize {
        let mut inner = self.inner.exclusive_access();
        let mut total_write_size = 0usize;
        for slice in buf.buffers.iter() {
            let write_size = inner.inode.write_at(inner.offset, *slice).unwrap();
            assert_eq!(write_size, slice.len());
            inner.offset += write_size;
            total_write_size += write_size;
        }
        total_write_size
    }
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
}

/// 根据路径递归寻找 Inode
/// 待优化.和..的处理,相对路径的处理,当前路径的处理
fn find_inode(path: &str) -> Option<Arc<dyn VfsInode>> {
    let mut current_inode = FS_MANAGER.exclusive_access().get("lwext4").unwrap().root();
    //现在的逻辑都是从根目录开始找
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return Some(current_inode);
    }
    
    //  逐级 lookup
    for part in parts {
        if let Some(next_inode) = current_inode.lookup(part) {
            current_inode = next_inode;
        } else {
            return None; // 中间某一级查找失败
        }
    }
    Some(current_inode)
}

#[allow(unused)]
//open_file已经修改,从开始的从根目录开始扁平查找改成使用find_inode直接找到最终的inode,支持多级目录
//需要添加，需支持.和..的处理,相对路径的处理,当前路径的处理
///Open file with flags
/// 
pub fn open_file(name: &str, flags: OpenFlags) -> Option<Arc<OSInode>> {
    let (readable, writable) = flags.read_write();
    
    let inode_result = find_inode(name);
    if flags.contains(OpenFlags::CREATE) {
        if let Some(inode) = inode_result {
            // clear size
            inode.truncate(0).expect("Error when truncating inode");
            Some(Arc::new(OSInode::new(readable, writable, inode)))
        } else {
            // 注意：简单起见，这里假设在根目录下创建。(查找父目录的功能暂时还未实现)
            let root = FS_MANAGER.exclusive_access().get("lwext4").unwrap().root();
            root
                .create(name, InodeTypes::EXT4_DE_REG_FILE)
                .map(|inode| Arc::new(OSInode::new(readable, writable, inode)))
        }
    } else {
        inode_result.map(|inode| {
            if flags.contains(OpenFlags::TRUNC) {
                inode.truncate(0).expect("Error when truncating inode");
            }
            Arc::new(OSInode::new(readable, writable, inode))
        })
    }
}

/// List all files in the filesystems
pub fn list_apps() {
    let root = FS_MANAGER.exclusive_access().get("lwext4").unwrap().root();
    println!("/**** APPS ****");
    for app in root.ls() {
        println!("{}", app);
    }
    println!("**************/");
}
