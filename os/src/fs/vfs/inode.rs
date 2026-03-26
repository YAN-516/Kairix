#![allow(missing_docs)]
use alloc::{string::String, sync::Arc};
use lwext4_rust::InodeTypes;
use alloc::vec::Vec;
#[allow(unused)]
/// Inode:i_ino
pub struct InodeInner{
    pub ino:usize
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
    // //链接部分
    // fn link(&self, name: &str, target: Arc<dyn VfsInode>) -> Result<(), i32>{
    //     unimplemented!()
    // }
    // fn symlink(&self, name: &str, target: &str) -> Result<(), i32>{
    //     unimplemented!()
    // }
    // fn readlink(&self) -> Result<String, i32>{
    //     unimplemented!()
    // }

}
