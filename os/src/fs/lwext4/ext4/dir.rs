use crate::error::{SysError, SysResult};
use crate::fs::lwext4::{
    Lwext4MountGate, Lwext4Op, lwext4_err_to_sys, lwext4_mount_gate_for_path,
    with_lwext4_mount_lock_op, with_lwext4_mount_read_lock_op,
};
///借用了NighthawkOS的思路，封装了lwext4_rust的目录操作接口
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
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
pub struct ExtDir {
    dir: ext4_dir,
    gate: Arc<Lwext4MountGate>,
}

/// Wrapper for `lwext4_rust` crate's `ext4_direntry` struct which represents a directory
/// entry.
pub struct ExtDirEntry(ext4_direntry);

impl Drop for ExtDir {
    fn drop(&mut self) {
        with_lwext4_mount_read_lock_op(&self.gate, Lwext4Op::OpenClose, || unsafe {
            ext4_dir_close(&mut self.dir);
        });
    }
}

impl ExtDir {
    /// Opens a directory file at the given path and returns a handle to it.
    ///
    /// `path` is the absolute path to the file to be opened.
    pub fn open(path: &CStr) -> SysResult<Self> {
        let path_str = path.to_str().map_err(|_| SysError::EINVAL)?;
        let gate = lwext4_mount_gate_for_path(path_str).ok_or(SysError::EIO)?;
        let mut dir = MaybeUninit::uninit();
        let err = with_lwext4_mount_read_lock_op(&gate, Lwext4Op::OpenClose, || unsafe {
            ext4_dir_open(dir.as_mut_ptr(), path.as_ptr())
        });
        match err {
            0 => unsafe {
                Ok(Self {
                    dir: dir.assume_init(),
                    gate,
                })
            },
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

    /// Returns an owned copy of the next directory entry, or `None` at EOF.
    pub fn next(&mut self) -> Option<ExtDirEntry> {
        self.next_batch(1).pop()
    }

    /// Copy up to `limit` directory entries while holding the mount gate once.
    ///
    /// `ext4_dir_entry_next()` returns a pointer into the mutable directory
    /// handle. Returning that pointer after dropping the gate allows another
    /// lwext4 operation to invalidate it, so entries are copied into owned
    /// descriptors before the gate is released.
    pub fn next_batch(&mut self, limit: usize) -> Vec<ExtDirEntry> {
        let mut entries = Vec::with_capacity(limit);
        if limit == 0 {
            return entries;
        }
        with_lwext4_mount_lock_op(&self.gate, Lwext4Op::Directory, || unsafe {
            while entries.len() < limit {
                let Some(entry) = ext4_dir_entry_next(&mut self.dir).as_ref() else {
                    break;
                };
                entries.push(ExtDirEntry(*entry));
            }
        });
        entries
    }
    #[allow(unused)]
    /// Rewinds the directory entry offset to the beginning of the directory file.
    pub fn rewind(&mut self) {
        with_lwext4_mount_lock_op(&self.gate, Lwext4Op::Directory, || unsafe {
            ext4_dir_entry_rewind(&mut self.dir);
        });
    }
}

impl ExtDirEntry {
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
