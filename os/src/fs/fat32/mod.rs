#![allow(missing_docs, dead_code)]

pub mod dentry;
pub mod file;
pub mod fstype;
pub mod inode;
pub mod io;
pub mod superblock;

use crate::error::SysError;
use fatfs::Error as FatError;

/// Convert a `fatfs::Error` to a [`SysError`].
pub fn fat32_error_to_sys(err: FatError<()>) -> SysError {
    match err {
        FatError::NotFound => SysError::ENOENT,
        FatError::AlreadyExists => SysError::EEXIST,
        FatError::DirectoryIsNotEmpty => SysError::ENOTEMPTY,
        FatError::InvalidInput
        | FatError::InvalidFileNameLength
        | FatError::UnsupportedFileNameCharacter => SysError::EINVAL,
        FatError::NotEnoughSpace => SysError::ENOSPC,
        FatError::Io(())
        | FatError::UnexpectedEof
        | FatError::WriteZero
        | FatError::CorruptedFileSystem => SysError::EIO,
        _ => SysError::EIO,
    }
}
