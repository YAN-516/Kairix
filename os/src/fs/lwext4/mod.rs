use crate::error::SysError;

/// Convert a lwext4 C FFI error code to a [`SysError`].
///
/// lwext4 APIs in this tree may return either positive or negative errno values.
pub fn lwext4_err_to_sys(err: i32) -> SysError {
    SysError::try_from(err.abs()).unwrap_or(SysError::EIO)
}

///
pub mod dentry;
pub mod disk;
///
pub mod ext4;
///
pub mod file;
///vfs file system type
pub mod fstype;
///
pub mod inode;
///
pub mod superblock;
