#![allow(missing_docs)]
use alloc::{string::String, sync::Arc};
use lwext4_rust::InodeTypes;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
#[allow(unused)]
/// Inode:i_ino
pub struct InodeInner{
    pub ino:usize,
    pub size: AtomicUsize,
    pub nlink: AtomicUsize, 
    pub mode: u32, 
}
impl InodeInner{
    pub fn new(ino:usize, size: usize, mode: u32) -> Self {
        Self{
            ino,
            size: AtomicUsize::new(size),
            nlink: AtomicUsize::new(1), 
            mode,
        }
    }
}

/// VFS 层通用的文件类型抽象
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InodeType {
    File,
    Dir,
}
#[allow(unused)]
/// Node (file/directory) operations.
pub trait Inode: Send + Sync {
    //数据IO部分
    /// Read data from the file at the given offset.
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize, i32> {
        unimplemented!()
    }
    /// Write data to the file at the given offset.
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize, i32> {
        unimplemented!()
    }
    /// 获取inode的属性
    fn get_attr(&self) -> Result<usize, i32> {
        unimplemented!()
    }
    /// Flush the file, synchronize the data to disk.
    fn fsync(&self) -> Result<usize, i32> {
        unimplemented!()
    }
    /// Truncate the file to the given size.
    fn truncate(&self, _size: u64) -> Result<usize, i32> {
        unimplemented!()
    }
    /// Lookup the node with given `path` in the directory.
    ///
    /// Return the node if found.
    fn lookup(&self, _path: &str) -> Option<Arc<dyn Inode>> {
        unimplemented!()
    }

    /// Remove the node with the given `path` in the directory.
    fn remove(&self, _path: &str) -> Result<usize, i32> {
        unimplemented!()
    }
    ///
    fn get_types(&self) -> InodeTypes;


    fn get_ino(&self) -> usize { 0 }
    fn get_size(&self) -> usize { 0 }
    fn get_nlink(&self) -> usize { 1 }
    fn get_mode(&self) -> u32 { 0 }
    fn inc_nlink(&self) {}
    fn dec_nlink(&self) {}

}
