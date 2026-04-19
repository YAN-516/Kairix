#![allow(missing_docs)]
use crate::mm::UserBuffer;
use spin::Mutex;
use crate::fs::vfs::{Dentry};
use alloc::sync::{Arc,Weak};
use alloc::vec::Vec;
use crate::fs::Inode;
use spin::MutexGuard;
use lwext4_rust::Lwext4File;
use crate::fs::vfs::kstat::Kstat;
use alloc::string::String;
use crate::mm::FrameTracker;
use spin::rwlock::RwLock;
use crate::fs::page::pagecache::Page;
use crate::fs::page::pagecache::PAGE_CACHE;
use crate::fs::vfs::OpenFlags;
use crate::fs::vfs::path::split_parent_and_name;
use crate::fs::vfs::path::resolve_path;
use crate::fs::vfs::inode::InodeMode;
use crate::fs::GLOBAL_DCACHE;
use crate::fs::get_filesystem;
use crate::alloc::string::ToString;
#[allow(unused)]
pub struct FileInner {
    pub offset: usize,
    pub dentry: Arc<dyn Dentry>,

}


/// File trait
pub trait File: Send + Sync {
    ///Get the FileInner
    fn get_fileinner(&self)->MutexGuard<'_, FileInner>;
    /// If readable
    fn readable(&self) -> bool;
    /// If writable
    fn writable(&self) -> bool;
    /// Read file to `UserBuffer`
    fn read(&self, buf: UserBuffer) -> usize;
    /// Write `UserBuffer` to file
    fn write(&self, buf: UserBuffer) -> usize;
    ///get inode from the Dentry of FileInner
    fn get_inode(&self)-> Option<Arc<dyn Inode>>{
        self.get_fileinner().dentry.get_inode()
    }
    /// Do something when the node is opened.
    fn open(&self) -> Result<usize, i32> {
        Ok(0)
    }
    /// Do something when the node is closed.
    fn release(&self) -> Result<usize, i32> {
        Ok(0)
    }
    #[allow(unused)]
    ///chaneg the offset of file
    /// 
    fn seek(&self,new_offset:usize)->usize{
        unimplemented!()
    }
    fn ls(&self) -> Vec<(String, u64, u8)> {
        alloc::vec::Vec::new() 
    }
    fn get_offset(&self) -> usize {
        self.get_fileinner().offset
    }
    fn set_offset(&self, new_offset: usize) {
        self.get_fileinner().offset = new_offset;
    }
    fn get_dentry(&self) -> Arc<dyn Dentry> {
        self.get_fileinner().dentry.clone()
    }
    fn get_stat(&self, stat: &mut Kstat) -> Result<(), isize> {
        let inode = self.get_inode().ok_or(-1isize)?;
        stat.st_ino = inode.get_ino() as u64;
        stat.st_nlink = inode.get_nlink() as u32;
        stat.st_size = inode.get_size() as i64;
        stat.st_mode = inode.get_mode().bits();
        stat.st_blksize = 512;
        stat.st_blocks = (stat.st_size as u64 + 511) / 512;
        let (atime_sec, atime_nsec) = inode.get_atime();
        let (mtime_sec, mtime_nsec) = inode.get_mtime();
        let (ctime_sec, ctime_nsec) = inode.get_ctime();
        stat.st_atime_sec = atime_sec;
        stat.st_atime_nsec = atime_nsec;
        stat.st_mtime_sec = mtime_sec;
        stat.st_mtime_nsec = mtime_nsec;
        stat.st_ctime_sec = ctime_sec;
        stat.st_ctime_nsec = ctime_nsec;
        Ok(())
    }
    /// 把内存里的脏页刷入底层存储
    fn flush(&self) {}

    /// 专门为 mmap / sendfile 提供：获取文件指定页的物理帧（Miss时自动读盘）
    fn get_cache_frame(&self, _page_id: usize) -> Option<Arc<FrameTracker>> {
        None
    }
    
    fn read_all(&self)->Vec<u8>{
        todo!()
    }
    /// ioctl
    fn ioctl(&self, _request: usize, _argp: usize) -> isize {
        -25
    }
}

impl dyn File {
    // /// 获取指定的缓存页，如果 Miss 则自动从磁盘加载并放入缓存
    // fn get_or_load_cache_page(&self, ino: usize, page_id: usize, old_size: usize) -> Arc<RwLock<Page>> {
    //     if let Some(page) = PAGE_CACHE.read().get_page(ino, page_id) {
    //         return page;
    //     }
    //     let mut cache_writer = PAGE_CACHE.write();
    //     if let Some(page) = cache_writer.get_page(ino, page_id) {
    //         return page;
    //     }
    //     let new_page = self.load_page_from_disk(page_id, old_size);
    //     cache_writer.insert_page(ino, page_id, new_page.clone());
    //     new_page
    // }
   
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
) -> Option<Arc<dyn File>> {
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
    let is_append = flags.contains(OpenFlags::O_APPEND);
    let file = target_dentry.open(flags,inode.get_mode())?;
    if is_append {
        file.set_offset(inode.get_size());
    }
    Some(file)
}