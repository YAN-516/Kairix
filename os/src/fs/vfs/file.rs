#![allow(missing_docs)]
use crate::fs::Inode;
use crate::fs::vfs::Dentry;
use crate::fs::vfs::kstat::Kstat;
use crate::mm::UserBuffer;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use lwext4_rust::Lwext4File;
use polyhal::common::FrameTracker;
use spin::Mutex;
use spin::MutexGuard;
#[allow(unused)]
pub struct FileInner {
    pub offset: usize,
    pub dentry: Arc<dyn Dentry>,
    pub ext4file: Lwext4File,
}

/// File trait
pub trait File: Send + Sync {
    ///Get the FileInner
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner>;
    /// If readable
    fn readable(&self) -> bool;
    /// If writable
    fn writable(&self) -> bool;
    /// Read file to `UserBuffer`
    fn read(&self, buf: UserBuffer) -> usize;
    /// Write `UserBuffer` to file
    fn write(&self, buf: UserBuffer) -> usize;
    ///get inode from the Dentry of FileInner
    fn get_inode(&self) -> Option<Arc<dyn Inode>> {
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
    fn seek(&self, new_offset: usize) -> usize {
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
