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
    fn get_stat(&self, _stat: &mut Kstat) -> Result<(), isize> {
    unimplemented!()
    }
    /// 把内存里的脏页刷入底层存储
    fn flush(&self) {}

    /// 专门为 mmap 提供：获取文件指定页的物理帧（Miss时自动读盘）
    fn get_cache_frame(&self, _page_id: usize) -> Arc<FrameTracker> {
        unimplemented!("This file type does not support mmap");
    }
}

// impl dyn File {
//     /// 获取指定的缓存页，如果 Miss 则自动从磁盘加载并放入缓存
//     fn get_or_load_cache_page(&self, ino: usize, page_id: usize, old_size: usize) -> Arc<RwLock<Page>> {
//         if let Some(page) = PAGE_CACHE.read().get_page(ino, page_id) {
//             return page;
//         }
//         let mut cache_writer = PAGE_CACHE.write();
//         if let Some(page) = cache_writer.get_page(ino, page_id) {
//             return page;
//         }
//         let new_page = self.load_page_from_disk(page_id, old_size);
//         cache_writer.insert_page(ino, page_id, new_page.clone());
//         new_page
//     }
    
// }