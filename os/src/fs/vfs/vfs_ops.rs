use crate::fs::ext4fs::Ext4Inode;
use alloc::vec::Vec;
use alloc::{string::String, sync::Arc};
use lwext4_rust::InodeTypes;
#[allow(unused)]
/// Filesystem operations.
pub trait VfsSuperBlock: Send + Sync {
    /// Do something when the filesystem is mounted.
    fn mount(&self, _path: &str, _mount_point: Arc<dyn VfsInode>) -> Result<usize, i32> {
        Ok(0)
    }

    /// Do something when the filesystem is unmounted.
    fn umount(&self) -> Result<usize, i32> {
        Ok(0)
    }

    /// Format the filesystem.
    fn format(&self) -> Result<usize, i32> {
        unimplemented!()
    }

    /// Get the attributes of the filesystem.
    fn statfs(&self) -> Result<usize, i32> {
        unimplemented!()
    }

    /// Get the root directory of the filesystem.
    fn root_dir(&self) -> Arc<dyn VfsInode>;
}
#[allow(unused)]
/// Node (file/directory) operations.
pub trait VfsInode: Send + Sync {
    /// Do something when the node is opened.
    fn open(&self) -> Result<usize, i32> {
        Ok(0)
    }

    /// Do something when the node is closed.
    fn release(&self) -> Result<usize, i32> {
        Ok(0)
    }

    /// 获取inode的属性
    fn get_attr(&self) -> Result<usize, i32> {
        unimplemented!()
    }
    //数据IO部分
    /// Read data from the file at the given offset.
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize, i32> {
        unimplemented!()
    }

    /// Write data to the file at the given offset.
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize, i32> {
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
    // directory operations:

    /// Get the parent directory of this directory.
    ///
    /// Return `None` if the node is a file.
    fn parent(&self) -> Option<Arc<dyn VfsInode>> {
        None
    }

    ///
    ///
    ///
    fn find(&self, _name: &str) -> Option<Arc<dyn VfsInode>> {
        unimplemented!()
    }
    /// Lookup the node with given `path` in the directory.
    ///
    /// Return the node if found.
    fn lookup(&self, _path: &str) -> Option<Arc<Ext4Inode>> {
        unimplemented!()
    }

    fn ls(&self) -> Vec<String> {
        unimplemented!()
    }
    /// Create a new node with the given `path` in the directory
    ///
    /// Return [`Ok(())`](Ok) if it already exists.
    fn create(&self, _path: &str, _ty: InodeTypes) -> Option<Arc<dyn VfsInode>> {
        unimplemented!()
    }

    /// Remove the node with the given `path` in the directory.
    fn remove(&self, _path: &str) -> Result<usize, i32> {
        unimplemented!()
    }

    /// Renames or moves existing file or directory.
    fn rename(&self, _src_path: &str, _dst_path: &str) -> Result<usize, i32> {
        unimplemented!()
    }

    /// Convert `&self` to [`&dyn Any`][1] that can use
    /// [`Any::downcast_ref`][2].
    ///
    /// [1]: core::any::Any
    /// [2]: core::any::Any#method.downcast_ref
    fn as_any(&self) -> &dyn core::any::Any {
        unimplemented!()
    }

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
