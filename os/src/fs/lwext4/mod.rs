
use crate::error::SysError;

/// Convert a lwext4 C FFI error code to a [`SysError`].
///
/// lwext4 APIs in this tree may return either positive or negative errno values.
pub fn lwext4_err_to_sys(err: i32) -> SysError {
    SysError::try_from(err.abs()).unwrap_or(SysError::EIO)
}

///
pub mod inode;
pub mod disk;
///
pub mod superblock;
///
pub mod file;
///
pub mod dentry;
///
pub mod ext4;
///vfs file system type
pub mod fstype;
