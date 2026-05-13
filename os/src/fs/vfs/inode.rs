#![allow(missing_docs)]
use crate::error::{SysError, SysResult, SyscallResult};
use alloc::{string::String, sync::Arc};
use lwext4_rust::InodeTypes;
use alloc::vec::Vec;
use core::sync::atomic::AtomicI64;
use core::sync::atomic::{AtomicUsize, Ordering};

#[allow(unused)]
/// Inode:i_ino
pub struct InodeInner {
    pub ino: usize,
    pub size: AtomicUsize,
    pub nlink: AtomicUsize,
    pub mode: InodeMode,
    pub uid: AtomicUsize,
    pub gid: AtomicUsize,
    pub atime_sec: AtomicI64,
    pub atime_nsec: AtomicI64,
    pub mtime_sec: AtomicI64,
    pub mtime_nsec: AtomicI64,
    pub ctime_sec: AtomicI64,
    pub ctime_nsec: AtomicI64,
}
impl InodeInner {
    pub fn new(ino: usize, size: usize, mode: InodeMode) -> Self {
        Self {
            ino,
            size: AtomicUsize::new(size),
            nlink: AtomicUsize::new(1),
            mode,
            uid: AtomicUsize::new(0),
            gid: AtomicUsize::new(0),
            atime_sec: AtomicI64::new(0),
            atime_nsec: AtomicI64::new(0),
            mtime_sec: AtomicI64::new(0),
            mtime_nsec: AtomicI64::new(0),
            ctime_sec: AtomicI64::new(0),
            ctime_nsec: AtomicI64::new(0),
        }
    }
}

#[allow(unused)]
/// Node (file/directory) operations.
pub trait Inode: Send + Sync {
    //数据IO部分
    /// Read data from the file at the given offset.
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> SysResult<usize> {
        unimplemented!()
    }
    /// Write data to the file at the given offset.
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> SysResult<usize> {
        unimplemented!()
    }
    /// 获取inode的属性
    fn get_attr(&self) -> SysResult<usize> {
        unimplemented!()
    }
    /// Flush the file, synchronize the data to disk.
    fn fsync(&self) -> SysResult<usize> {
        unimplemented!()
    }
    /// Truncate the file to the given size.
    fn truncate(&self, _size: u64) -> SysResult<usize> {
        Err(SysError::ENOSYS)
    }
    /// Lookup the node with given `path` in the directory.
    ///
    /// Return the node if found.
    fn lookup(&self, _path: &str) -> SysResult<Arc<dyn Inode>> {
        unimplemented!()
    }

    /// Remove the node with the given `path` in the directory.
    fn remove(&self, _path: &str) -> SysResult<usize> {
        unimplemented!()
    }
    ///
    fn get_types(&self) -> InodeTypes {
        unimplemented!()
    }

    fn get_ino(&self) -> usize {
        todo!()
    }

    fn get_size(&self) -> usize {
        todo!()
    }
    fn set_size(&self, _new_size: usize) {
        todo!()
    }
    fn get_nlink(&self) -> usize {
        todo!()
    }
    fn get_mode(&self) -> InodeMode {
        todo!()
    }
    fn set_mode(&self, _mode: InodeMode) {}
    fn get_uid(&self) -> usize { 0 }
    fn set_uid(&self, _uid: usize) {}
    fn get_gid(&self) -> usize { 0 }
    fn set_gid(&self, _gid: usize) {}
    fn inc_nlink(&self) {
        todo!()
    }
    fn dec_nlink(&self) {
        todo!()
    }

    fn get_atime(&self) -> (i64, i64) {
        (0, 0)
    }

    fn set_atime(&self, _sec: i64, _nsec: i64) {}

    fn get_mtime(&self) -> (i64, i64) {
        (0, 0)
    }

    fn set_mtime(&self, _sec: i64, _nsec: i64) {}

    fn get_ctime(&self) -> (i64, i64) {
        (0, 0)
    }

    fn set_ctime(&self, _sec: i64, _nsec: i64) {}

    /// Read the target of a symbolic link.
    /// Default returns -EINVAL since symlinks are not yet fully supported.
    fn readlink(&self) -> Result<String, i32> {
        Err(-22)
    }
}

static INODE_NUMBER: AtomicUsize = AtomicUsize::new(0);
pub fn inode_alloc() -> usize {
    INODE_NUMBER.fetch_add(1, Ordering::Relaxed)
}

bitflags! {
    #[derive(Debug, Clone, Copy, Eq, PartialEq)]
    /// Mode of an inode, defining the type and permissions of the inode.
    pub struct InodeMode: u32 {
        /// Type.
        const TYPE_MASK = 0o170000;
        /// FIFO.
        const FIFO  = 0o010000;
        /// Character device.
        const CHAR  = 0o020000;
        /// Directory
        const DIR   = 0o040000;
        /// Block device
        const BLOCK = 0o060000;
        /// Regular file.
        const FILE  = 0o100000;
        /// Symbolic link.
        const LINK  = 0o120000;
        /// Socket
        const SOCKET = 0o140000;

        const S_PERM  = 0o7777;
        /// Set-user-ID on execution.
        const SET_UID = 0o4000;
        /// Set-group-ID on execution.
        const SET_GID = 0o2000;
        /// sticky bit
        const STICKY = 0o1000;
        /// Read, write, execute/search by owner.
        const OWNER_MASK = 0o700;
        /// Read permission, owner.
        const OWNER_READ = 0o400;
        /// Write permission, owner.
        const OWNER_WRITE = 0o200;
        /// Execute/search permission, owner.
        const OWNER_EXEC = 0o100;

        /// Read, write, execute/search by group.
        const GROUP_MASK = 0o70;
        /// Read permission, group.
        const GROUP_READ = 0o40;
        /// Write permission, group.
        const GROUP_WRITE = 0o20;
        /// Execute/search permission, group.
        const GROUP_EXEC = 0o10;

        /// Read, write, execute/search by others.
        const OTHER_MASK = 0o7;
        /// Read permission, others.
        const OTHER_READ = 0o4;
        /// Write permission, others.
        const OTHER_WRITE = 0o2;
        /// Execute/search permission, others.
        const OTHER_EXEC = 0o1;
    }
}
