use alloc::sync::Arc;
use core::ffi::CStr;
use core::mem::MaybeUninit;
use log::*;
use lwext4_rust::bindings::ext4_file;
use lwext4_rust::bindings::{
    O_CREAT, O_EXCL, O_WRONLY, ext4_dir_mk, ext4_dir_mv, ext4_dir_rm, ext4_fclose,
    ext4_file_stat_get, ext4_flink, ext4_fopen, ext4_fopen2, ext4_fremove, ext4_frename,
    ext4_fsize, ext4_fsymlink, ext4_inode, ext4_inode_stat, ext4_inode_stat_child_get,
    ext4_inode_stat_get, ext4_mode_set, ext4_raw_inode_fill, ext4_readlink,
};

use crate::error::{SysError, SysResult};
use crate::fs::lwext4::{
    Lwext4MountGate, Lwext4Op, lwext4_err_to_sys, lwext4_mount_gate_for_path,
    with_lwext4_mount_lock_op,
};
use crate::fs::vfs::path;
///
pub struct ExtFS(pub ext4_file);

fn gate_for_path(path: &CStr) -> SysResult<Arc<Lwext4MountGate>> {
    let path = path.to_str().map_err(|_| SysError::EINVAL)?;
    lwext4_mount_gate_for_path(path).ok_or(SysError::EIO)
}

fn gate_for_two_paths(path: &CStr, other: &CStr) -> SysResult<Arc<Lwext4MountGate>> {
    let gate = gate_for_path(path)?;
    let other_gate = gate_for_path(other)?;
    if !Arc::ptr_eq(&gate, &other_gate) {
        return Err(SysError::EXDEV);
    }
    Ok(gate)
}

impl Drop for ExtFS {
    fn drop(&mut self) {
        unsafe {
            ext4_fclose(&mut self.0);
        }
    }
}

impl ExtFS {
    /// Read Linux-visible inode metadata for a path.
    pub fn inode_stat(path: &CStr) -> SysResult<ext4_inode_stat> {
        let gate = gate_for_path(path)?;
        let mut stat = MaybeUninit::<ext4_inode_stat>::uninit();
        let err = with_lwext4_mount_lock_op(&gate, Lwext4Op::Stat, || unsafe {
            ext4_inode_stat_get(path.as_ptr(), stat.as_mut_ptr())
        });
        if err != 0 {
            return Err(lwext4_err_to_sys(err));
        }
        Ok(unsafe { stat.assume_init() })
    }

    /// Read one child's metadata without resolving its parent path again.
    pub(crate) fn inode_stat_child(
        gate: &Lwext4MountGate,
        mount_path: &CStr,
        parent_inode: usize,
        name: &str,
    ) -> SysResult<ext4_inode_stat> {
        if name.is_empty()
            || name.len() > 255
            || name.as_bytes().contains(&0)
            || name.as_bytes().contains(&b'/')
        {
            return Err(SysError::EINVAL);
        }
        let parent_inode = u32::try_from(parent_inode).map_err(|_| SysError::EINVAL)?;
        let mut stat = MaybeUninit::<ext4_inode_stat>::uninit();
        let err = with_lwext4_mount_lock_op(gate, Lwext4Op::Stat, || unsafe {
            ext4_inode_stat_child_get(
                mount_path.as_ptr(),
                parent_inode,
                name.as_ptr().cast(),
                name.len(),
                stat.as_mut_ptr(),
            )
        });
        if err != 0 {
            return Err(lwext4_err_to_sys(err));
        }
        Ok(unsafe { stat.assume_init() })
    }

    /// Read Linux-visible inode metadata through an open lwext4 file.
    pub(crate) fn file_stat(file: &mut ext4_file) -> SysResult<ext4_inode_stat> {
        let mut stat = MaybeUninit::<ext4_inode_stat>::uninit();
        let err = unsafe { ext4_file_stat_get(file, stat.as_mut_ptr()) };
        if err != 0 {
            return Err(lwext4_err_to_sys(err));
        }
        Ok(unsafe { stat.assume_init() })
    }

    /// Return the raw ext4 inode number for a path.
    pub fn raw_inode_ino(path: &CStr) -> SysResult<usize> {
        let gate = gate_for_path(path)?;
        let mut ino = 0u32;
        let mut inode = MaybeUninit::<ext4_inode>::uninit();
        let err = with_lwext4_mount_lock_op(&gate, Lwext4Op::Stat, || unsafe {
            ext4_raw_inode_fill(path.as_ptr(), &mut ino, inode.as_mut_ptr())
        });
        match err {
            0 => Ok(ino as usize),
            _ => {
                warn!(
                    "ext4_raw_inode_fill failed: path = {}, error = {}",
                    path.to_str().unwrap_or("unknown"),
                    err
                );
                Err(lwext4_err_to_sys(err))
            }
        }
    }

    #[allow(unused)]
    ///
    // create a file at the given path, the path should be absolute path
    pub fn create_file(path: &CStr) -> SysResult<()> {
        let gate = gate_for_path(path)?;
        let mut file_struct = MaybeUninit::uninit();
        let flags = (O_WRONLY | O_CREAT | O_EXCL) as i32;
        let err = with_lwext4_mount_lock_op(&gate, Lwext4Op::Metadata, || unsafe {
            let err = ext4_fopen2(file_struct.as_mut_ptr(), path.as_ptr(), flags);
            if err == 0 {
                ext4_fclose(file_struct.as_mut_ptr());
            }
            err
        });
        match err {
            0 => Ok(()),
            _ => {
                warn!("ext4_fopen (create file) failed: error = {}", err);
                Err(lwext4_err_to_sys(err))
            }
        }
    }

    /// Create a symbolic link.
    pub fn symlink(target: &CStr, path: &CStr) -> SysResult<()> {
        let gate = gate_for_path(path)?;
        let err = with_lwext4_mount_lock_op(&gate, Lwext4Op::Metadata, || unsafe {
            ext4_fsymlink(target.as_ptr(), path.as_ptr())
        });
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
        let gate = gate_for_path(path)?;
        let mut rcnt: usize = 0;
        let err = with_lwext4_mount_lock_op(&gate, Lwext4Op::Read, || {
            #[cfg(target_arch = "riscv64")]
            {
                unsafe { ext4_readlink(path.as_ptr(), buf.as_mut_ptr(), buf.len(), &mut rcnt) }
            }
            #[cfg(target_arch = "loongarch64")]
            {
                unsafe {
                    ext4_readlink(
                        path.as_ptr(),
                        buf.as_mut_ptr() as *mut i8,
                        buf.len(),
                        &mut rcnt,
                    )
                }
            }
        });

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
        let gate = gate_for_two_paths(path, new_path)?;
        let err = with_lwext4_mount_lock_op(&gate, Lwext4Op::Metadata, || unsafe {
            ext4_dir_mv(path.as_ptr(), new_path.as_ptr())
        });
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
        let gate = gate_for_two_paths(path, new_path)?;
        let err = with_lwext4_mount_lock_op(&gate, Lwext4Op::Metadata, || unsafe {
            ext4_frename(path.as_ptr(), new_path.as_ptr())
        });
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
        let gate = gate_for_path(path)?;
        let err = with_lwext4_mount_lock_op(&gate, Lwext4Op::Metadata, || unsafe {
            ext4_dir_rm(path.as_ptr())
        });
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
        let gate = gate_for_path(path)?;
        let err = with_lwext4_mount_lock_op(&gate, Lwext4Op::Metadata, || unsafe {
            ext4_fremove(path.as_ptr())
        });
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
        let gate = gate_for_two_paths(path, hardlink_path)?;
        let err = with_lwext4_mount_lock_op(&gate, Lwext4Op::Metadata, || unsafe {
            ext4_flink(path.as_ptr(), hardlink_path.as_ptr())
        });
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
        let gate = gate_for_path(path)?;
        let err = with_lwext4_mount_lock_op(&gate, Lwext4Op::Metadata, || unsafe {
            ext4_dir_mk(path.as_ptr())
        });
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
        let gate = gate_for_path(path)?;
        let err = with_lwext4_mount_lock_op(&gate, Lwext4Op::Metadata, || unsafe {
            ext4_mode_set(path.as_ptr(), mode)
        });
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
