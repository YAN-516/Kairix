use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::sync::{Arc, Weak};
use alloc::{format, vec, vec::Vec};
use core::cell::RefMut;
use core::sync::atomic::{AtomicUsize, Ordering};

use bitflags::*;
use lazy_static::*;
use log::{info, warn};
use spin::{Mutex, MutexGuard, rwlock::RwLock};

use virtio_drivers::device::blk::VirtIOBlk;
use virtio_drivers::transport::mmio::{MmioTransport, VirtIOHeader};
use virtio_drivers::transport::{DeviceType, Transport};

use lwext4_rust::bindings::{O_RDONLY, O_RDWR, O_WRONLY, SEEK_SET};
use lwext4_rust::{InodeTypes, Lwext4File};

use crate::config::PAGE_SIZE;
use crate::drivers::block::BLOCK_DEVICE;
use crate::sync::UPSafeCell;

use crate::mm::{frame_alloc, FrameTracker, UserBuffer};

use crate::fs::vfs::{
    dcache::GLOBAL_DCACHE,
    file::File,
    inode::{Inode, InodeMode},
    kstat::Kstat,
    path::{resolve_path, split_parent_and_name},
    Dentry, FileInner, OpenFlags,
};

use crate::fs::lwext4::{
    dentry::Ext4Dentry, 
    disk::Disk, 
    inode::Ext4Inode
};

use crate::fs::page::pagecache::{Page, PAGE_CACHE};
use crate::fs::get_filesystem;
///the Ext4File
pub struct Ext4File {
    readable: bool,
    writable: bool,
    inner: Mutex<FileInner>,
    ///
    pub ext4file: Mutex<Lwext4File>,
}

impl Ext4File {
    /// Construct an Ext4File from a Dentry
    pub fn new(
        readable: bool,
        writable: bool,
        dentry: Arc<dyn Dentry>,
        types: InodeTypes,
    ) -> Result<Self, i32> {
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
            }),
            ext4file: Mutex::new(file),
        })
    }

    /// Read all data
    pub fn read_all(&self) -> Vec<u8> {
        let mut inner = self.inner.lock();
        let mut buffer = [0u8; 512];
        let mut v: Vec<u8> = Vec::new();
        loop {
            let current_offset = inner.offset;
            self
                .ext4file
                .lock()
                .file_seek(current_offset as i64, SEEK_SET)
                .expect("seek failed");
            let len = self.ext4file.lock().file_read(&mut buffer).unwrap();
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
    fn truncate(&mut self, size: u64) -> Result<usize, i32> {
        info!("truncate file to size={}", size);
        // let mut inner = self.inner.lock();
        self.ext4file.lock().file_truncate(size)
    }


    /// 从磁盘加载指定页的数据到物理帧中（如果超出文件范围则清零）
    fn load_page_from_disk(&self, page_id: usize, old_size: usize) -> Arc<RwLock<Page>> {
        let new_frame = Arc::new(frame_alloc().unwrap());
        let page_start_offset = page_id * PAGE_SIZE;
        if page_start_offset < old_size {
            let valid_len = (old_size - page_start_offset).min(PAGE_SIZE);
            self.ext4file.lock().file_seek(page_start_offset as i64, SEEK_SET).unwrap();
            
            let buffer = &mut new_frame.ppn.get_bytes_array()[..valid_len];
            let read_len = self.ext4file.lock().file_read(buffer).unwrap(); 
            assert_eq!(read_len, valid_len); 
        } else {
            new_frame.ppn.get_bytes_array().fill(0);
        }
        Arc::new(RwLock::new(Page {
            frame: new_frame,
            dirty: false, 
        }))
    }
        /// 获取指定的缓存页，如果 Miss 则自动从磁盘加载并放入缓存
    fn get_or_load_cache_page(&self, ino: usize, page_id: usize, old_size: usize) -> Arc<RwLock<Page>> {
        if let Some(page) = PAGE_CACHE.read().get_page(ino, page_id) {
            return page;
        }
        let mut cache_writer = PAGE_CACHE.write();
        if let Some(page) = cache_writer.get_page(ino, page_id) {
            return page;
        }
        let new_page = self.load_page_from_disk(page_id, old_size);
        cache_writer.insert_page(ino, page_id, new_page.clone());
        new_page
    }

}

impl File for Ext4File {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
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
        let inode = inner.dentry.get_inode().unwrap();
        let ino = inode.get_ino();
        //暂时直接调用底层
        let file_size = self.ext4file.lock().file_desc.fsize as usize;
        let mut current_offset = inner.offset;
        let mut total_read_size = 0usize;
        if current_offset >= file_size { return 0; }
        for slice in buf.buffers.iter_mut() {
            let mut slice_offset = 0;
            let slice_len = slice.len();
            while slice_offset < slice_len && current_offset < file_size {
                let target_page = self.get_or_load_cache_page(ino, current_offset / PAGE_SIZE, file_size);
                {
                    let page_reader = target_page.read(); 
                    let page_offset = current_offset % PAGE_SIZE;
                    let left_in_page = PAGE_SIZE - page_offset;
                    let left_in_slice = slice_len - slice_offset;
                    let left_in_file = file_size - current_offset;
                    let read_bytes = left_in_page.min(left_in_slice).min(left_in_file);
                    let src_data = &page_reader.frame.ppn.get_bytes_array()[page_offset..page_offset + read_bytes];
                    slice[slice_offset..slice_offset + read_bytes].copy_from_slice(src_data);
                    
                    current_offset += read_bytes;
                    slice_offset += read_bytes;
                    total_read_size += read_bytes;
                }
            }
        }
        inner.offset = current_offset;
        total_read_size
    }
    
    fn write(&self, buf: UserBuffer) -> usize {
        info!("enter VFS Write-back Cache");
        let mut inner = self.inner.lock();
        let inode = inner.dentry.get_inode().unwrap();
        let ino = inode.get_ino();
        // println!("[DEBUG] 当前操作的 ino: {}", ino);
        let old_size = inode.get_size(); 
        let mut total_write_size = 0usize;
        let mut current_offset = inner.offset;
        for slice in buf.buffers.iter() {
            let mut slice_offset = 0;
            let slice_len = slice.len();
            while slice_offset < slice_len {
                let page_id = current_offset / PAGE_SIZE;
                let page_offset = current_offset % PAGE_SIZE;
                let write_bytes = (PAGE_SIZE - page_offset).min(slice_len - slice_offset);
                // 获取缓存页
                let target_page = self.get_or_load_cache_page(ino, page_id, old_size);
                // 写入数据并标记脏页
                {
                    let mut page_writer = target_page.write();
                    let data_to_write = &slice[slice_offset..slice_offset + write_bytes];
                    page_writer.modify(page_offset, data_to_write);
                }
                current_offset += write_bytes;
                slice_offset += write_bytes;
                total_write_size += write_bytes;
            }
        }
        if current_offset > old_size {
            inode.set_size(current_offset); 
        }
        inner.offset = current_offset;
        total_write_size
    }
    
    fn ls(&self) -> Vec<(String, u64, u8)> {
        self.get_fileinner().dentry.ls()
    }

    fn get_stat(&self, stat: &mut Kstat) -> Result<(), isize> {
        let inner_lock = self.inner.lock();
        let inode = inner_lock.dentry.get_inode().unwrap();

        stat.st_ino = inode.get_ino() as u64;
        stat.st_nlink = inode.get_nlink() as u32;
        stat.st_size = inode.get_size() as i64;
        stat.st_mode = inode.get_mode().bits(); 
        stat.st_blksize = 512;
        stat.st_blocks = (stat.st_size as u64 + 511) / 512;

        stat.st_atime_sec = 0;
        stat.st_mtime_sec = 0;
        stat.st_ctime_sec = 0;
        Ok(())
    }
    ///
    fn flush(&self) {
        //只读不需要写回磁盘
        if !self.writable() {
            info!("File is read-only, skipping flush.");
            return;
        }
        info!("enter VFS flush (write-back to disk)");
        info!("[DEBUG flush] waiting for self.inner.lock()...");
        let  inner = self.inner.lock();
        info!("[DEBUG flush] self.inner locked!");
        let inode = inner.dentry.get_inode().unwrap();
        let inode_id = inode.get_ino();
        let file_size = inode.get_size();
        let max_page_id = (file_size + PAGE_SIZE - 1) / PAGE_SIZE;
        info!("[DEBUG flush] file_size: {}, max_page_id: {}", file_size, max_page_id);
        info!("[DEBUG flush] waiting for PAGE_CACHE.read()...");
        let cache_reader = PAGE_CACHE.read();
        info!("[DEBUG flush] PAGE_CACHE read locked!");
        for page_id in 0..max_page_id {
            if let Some(page_lock) = cache_reader.get_page(inode_id, page_id) {
                let mut page = page_lock.write();
                if page.dirty {
                    let offset = page_id * PAGE_SIZE;
                    let write_len = if offset + PAGE_SIZE > file_size {
                        file_size - offset 
                    } else {
                        PAGE_SIZE 
                    };
                    info!("[DEBUG flush] writing dirty page {} to disk...", page_id);
                    self.ext4file.lock().file_seek(offset as i64, SEEK_SET).unwrap();
                    let buffer = &page.frame.ppn.get_bytes_array()[..write_len];
                    self.ext4file.lock().file_write(buffer).unwrap();
                    page.dirty = false;
                }
            }
        }
        info!("finish VFS flush");
    }

    fn get_cache_frame(&self, page_id: usize) -> Arc<FrameTracker> {
        let  inner = self.inner.lock();
        let inode = inner.dentry.get_inode().unwrap();
        let ino = inode.get_ino();
        // println!("[DEBUG] 当前操作的 ino: {}", ino);
        let file_size = inode.get_size();
        let target_page = self.get_or_load_cache_page(ino, page_id, file_size);
        target_page.read().frame.clone() 
    }
}
#[allow(unused)]
/// find the dentry by the absolute path, if can not find, return None
/// find from the root dentry, and fill the dcache when find the dentry, if can not find, return None
pub fn find_dentry(path: &str) -> Option<Arc<dyn Dentry>> {
    if let Some(cached) = GLOBAL_DCACHE.get(path) {
        return Some(cached);
    }
    let rootfs = get_filesystem("ext4");
    let root_dentry = rootfs.get_sb("/").unwrap().root();
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
pub fn open_file(
    start_dentry: Arc<dyn Dentry>,
    path: &str,
    flags: OpenFlags,
) -> Option<Arc<Ext4File>> {
    let (readable, writable) = flags.read_write();
    let target_dentry = if flags.contains(OpenFlags::O_CREAT) {
        let (parent_path, name) = split_parent_and_name(path);
        let parent = resolve_path(start_dentry, parent_path.as_str())?;
        parent
            .find(name.as_str())
            .or_else(|| parent.create(name.as_str(), InodeMode::FILE))?
    } else {
        resolve_path(start_dentry, path)?
    };
    let inode = target_dentry.get_inode()?;
    if flags.contains(OpenFlags::O_TRUNC) {
        inode.truncate(0).ok()?;
    }
    Some(Arc::new(
        Ext4File::new(readable, writable, target_dentry, inode.get_types()).expect("..."),
    ))
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


