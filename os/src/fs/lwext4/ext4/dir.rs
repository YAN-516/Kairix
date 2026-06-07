use crate::error::{SysError, SysResult};
use crate::fs::lwext4::lwext4_err_to_sys;
///借用了NighthawkOS的思路，封装了lwext4_rust的目录操作接口
use alloc::string::String;
use core::{ffi::CStr, mem::MaybeUninit};
use log::*;
use lwext4_rust::{
    InodeTypes,
    bindings::{
        ext4_dir, ext4_dir_close, ext4_dir_entry_next, ext4_dir_entry_rewind, ext4_dir_mk,
        ext4_dir_mv, ext4_dir_open, ext4_dir_rm, ext4_direntry, ext4_fclose, ext4_fopen,
    },
};

/// Wrapper for `lwext4_rust` crate's `ext4_dir` struct which represents a directory
/// file which can reads and writes directory entries.
pub struct ExtDir(pub ext4_dir);

/// Wrapper for `lwext4_rust` crate's `ext4_direntry` struct which represents a directory
/// entry.
pub struct ExtDirEntry<'a>(&'a ext4_direntry);

impl Drop for ExtDir {
    fn drop(&mut self) {
        unsafe {
            ext4_dir_close(&mut self.0);
        }
    }
}

impl ExtDir {
    /// Opens a directory file at the given path and returns a handle to it.
    ///
    /// `path` is the absolute path to the file to be opened.
    pub fn open(path: &CStr) -> SysResult<Self> {
        let mut dir = MaybeUninit::uninit();
        let err = unsafe { ext4_dir_open(dir.as_mut_ptr(), path.as_ptr()) };
        match err {
            0 => unsafe { Ok(Self(dir.assume_init())) },
            _ => {
                warn!(
                    "ext4_dir_open failed: path = {}, error = {}",
                    path.to_str().unwrap_or("unknown"),
                    err
                );
                Err(lwext4_err_to_sys(err))
            }
        }
    }

    /// Returns a shared reference to the next directory entry in the directory.
    /// Returns `None` if there are no more entries.
    pub fn next(&mut self) -> Option<ExtDirEntry> {
        unsafe { ext4_dir_entry_next(&mut self.0).as_ref() }.map(ExtDirEntry)
    }
    #[allow(unused)]
    /// Rewinds the directory entry offset to the beginning of the directory file.
    pub fn rewind(&mut self) {
        unsafe {
            ext4_dir_entry_rewind(&mut self.0);
        }
    }
}

impl ExtDirEntry<'_> {
    /// Returns the inode number of the directory entry.
    pub fn ino(&self) -> u32 {
        self.0.inode
    }
    ///
    pub fn file_type(&self) -> InodeTypes {
        InodeTypes::from(self.0.inode_type as usize)
    }

    /// Returns the name of the directory entry.
    pub fn name(&self) -> Result<String, ()> {
        // 防御性处理：底层 name_length 异常时钳位到数组上限，避免污染上层目录遍历结果。
        let raw_len = self.0.name_length as usize;
        let safe_len = raw_len.min(self.0.name.len());
        if safe_len == 0 {
            return Err(());
        }
        let name_bytes = self.0.name[..safe_len].to_vec();
        String::from_utf8(name_bytes).map_err(|_| ())
    }
}
