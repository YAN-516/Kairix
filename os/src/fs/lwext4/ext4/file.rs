use core::ffi::CStr;
use core::mem::MaybeUninit;
use log::*;
use lwext4_rust::bindings::ext4_file;
use lwext4_rust::bindings::{
    ext4_dir_mk, ext4_dir_mv, ext4_dir_rm, ext4_fclose, ext4_flink, ext4_fopen, ext4_fremove,
    ext4_frename, ext4_fsize, ext4_fsymlink, ext4_mode_set, ext4_readlink,
};

use crate::error::{SysError, SysResult};
use crate::fs::lwext4::lwext4_err_to_sys;
use crate::fs::vfs::path;
///
pub struct ExtFS(pub ext4_file);

impl Drop for ExtFS {
    fn drop(&mut self) {
        unsafe {
            ext4_fclose(&mut self.0);
        }
    }
}

impl ExtFS {
    #[allow(unused)]
    ///
    // create a file at the given path, the path should be absolute path
    pub fn create_file(path: &CStr) -> SysResult<()> {
        let mut file_struct = MaybeUninit::uninit();
        let c_mode = core::ffi::CStr::from_bytes_with_nul(b"wb\0").unwrap();
        let err = unsafe { ext4_fopen(file_struct.as_mut_ptr(), path.as_ptr(), c_mode.as_ptr()) };
        match err {
            0 => unsafe {
                ext4_fclose(file_struct.as_mut_ptr());
                Ok(())
            },
            _ => {
                warn!("ext4_fopen (create file) failed: error = {}", err);
                Err(lwext4_err_to_sys(err))
            }
        }
    }

    /// Create a symbolic link.
    pub fn symlink(target: &CStr, path: &CStr) -> SysResult<()> {
        let err = unsafe { ext4_fsymlink(target.as_ptr(), path.as_ptr()) };
        match err {
            0 => Ok(()),
            _ => {
                warn!(
                    "ext4_fsymlink failed: target = {}, path = {}, error = {}",
                    target.to_str().unwrap_or("unknown"),
                    path.to_str().unwrap_or("unknown"),
                    err
                );
                Err(lwext4_err_to_sys(err))
            }
        }
    }

    /// Read the target of a symbolic link.
    pub fn readlink(path: &CStr, buf: &mut [u8]) -> SysResult<usize> {
        let mut rcnt: usize = 0;
        #[cfg(target_arch = "riscv64")]
        let err = unsafe { ext4_readlink(path.as_ptr(), buf.as_mut_ptr(), buf.len(), &mut rcnt) };
        #[cfg(target_arch = "loongarch64")]
        let err = unsafe {
            ext4_readlink(
                path.as_ptr(),
                buf.as_mut_ptr() as *mut i8,
                buf.len(),
                &mut rcnt,
            )
        };

        match err {
            0 => Ok(rcnt),
            _ => {
                warn!(
                    "ext4_readlink failed: path = {}, error = {}",
                    path.to_str().unwrap_or("unknown"),
                    err
                );
                Err(lwext4_err_to_sys(err))
            }
        }
    }

    /// Change the name or location of a directory.
    pub fn rename(path: &CStr, new_path: &CStr) -> SysResult<()> {
        let err = unsafe { ext4_dir_mv(path.as_ptr(), new_path.as_ptr()) };
        match err {
            0 => Ok(()),
            _ => {
                warn!(
                    "ext4_dir_mv failed: old_path = {}, new_path = {}, error = {}",
                    path.to_str().unwrap_or("unknown"),
                    new_path.to_str().unwrap_or("unknown"),
                    err
                );
                Err(lwext4_err_to_sys(err))
            }
        }
    }

    /// Change the name or location of a regular file.
    pub fn rename_file(path: &CStr, new_path: &CStr) -> SysResult<()> {
        let err = unsafe { ext4_frename(path.as_ptr(), new_path.as_ptr()) };
        match err {
            0 => Ok(()),
            _ => {
                warn!(
                    "ext4_frename failed: old_path = {}, new_path = {}, error = {}",
                    path.to_str().unwrap_or("unknown"),
                    new_path.to_str().unwrap_or("unknown"),
                    err
                );
                Err(lwext4_err_to_sys(err))
            }
        }
    }

    /// Remove a directory at the given path.
    pub fn remove_dir(path: &CStr) -> SysResult<()> {
        let err = unsafe { ext4_dir_rm(path.as_ptr()) };
        match err {
            0 => Ok(()),
            _ => {
                warn!(
                    "ext4_dir_mv (unlink) failed: path = {}, error = {}",
                    path.to_str().unwrap_or("unknown"),
                    err
                );
                Err(lwext4_err_to_sys(err))
            }
        }
    }

    /// remove a file at the given path.
    pub fn remove_file(path: &CStr) -> SysResult<()> {
        let err = unsafe { ext4_fremove(path.as_ptr()) };
        match err {
            0 => Ok(()),
            _ => {
                warn!(
                    "ext4_fremove failed: path = {}, error = {}",
                    path.to_str().unwrap_or("unknown"),
                    err
                );
                Err(lwext4_err_to_sys(err))
            }
        }
    }

    ///create the hard link
    pub fn link(path: &CStr, hardlink_path: &CStr) -> SysResult<()> {
        let err = unsafe { ext4_flink(path.as_ptr(), hardlink_path.as_ptr()) };
        match err {
            0 => Ok(()),
            _ => {
                warn!(
                    "ext4_flink failed: path = {}, hardlink_path = {}, error = {}",
                    path.to_str().unwrap_or("unknown"),
                    hardlink_path.to_str().unwrap_or("unknown"),
                    err
                );
                Err(lwext4_err_to_sys(err))
            }
        }
    }

    /// Creates a directory at the given path.
    pub fn create(path: &CStr) -> SysResult<()> {
        let err = unsafe { ext4_dir_mk(path.as_ptr()) };
        match err {
            0 => Ok(()),
            _ => {
                warn!(
                    "ext4_dir_mk failed: path = {}, error = {}",
                    path.to_str().unwrap_or("unknown"),
                    err
                );
                Err(lwext4_err_to_sys(err))
            }
        }
    }

    /// Set mode bits for a file/directory.
    pub fn mode_set(path: &CStr, mode: u32) -> SysResult<()> {
        let err = unsafe { ext4_mode_set(path.as_ptr(), mode) };
        match err {
            0 => Ok(()),
            _ => {
                warn!(
                    "ext4_mode_set failed: path = {}, mode = {:o}, error = {}",
                    path.to_str().unwrap_or("unknown"),
                    mode,
                    err
                );
                Err(lwext4_err_to_sys(err))
            }
        }
    }
    ///
    pub fn size(&mut self) -> u64 {
        unsafe { ext4_fsize(&mut self.0) }
    }
}
