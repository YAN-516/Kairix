#![allow(missing_docs)]

use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::config::FD_CLOEXEC_FLAG;
use crate::fs::notify::inotify::{
    IN_DONT_FOLLOW, InotifyFile, create_inotify_file, inotify_add_watch, inotify_file_from_file,
    inotify_init_cloexec, inotify_remove_watch, register_inotify_file,
};
use crate::fs::vfs::path::{resolve_path, resolve_path_nofollow_last};
use crate::mm::translated_str;
use crate::task::{current_process, current_user_token};
use alloc::sync::Arc;

pub fn sys_inotify_init1(flags: i32) -> SyscallResult {
    let file = create_inotify_file(flags)?;
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    let fd = inner.alloc_fd()?;
    register_inotify_file(&file)?;
    inner.fd_table[fd] = Some(file);
    if inotify_init_cloexec(flags) && fd < inner.fd_flags.len() {
        inner.fd_flags[fd] |= FD_CLOEXEC_FLAG;
    }
    Ok(fd)
}

pub fn sys_inotify_add_watch(fd: usize, path: *const u8, mask: u32) -> SyscallResult {
    if path.is_null() {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let raw_path = translated_str(token, path)?;
    let cwd = current_process().inner_exclusive_access().cwd.clone();
    let dentry = if mask & IN_DONT_FOLLOW != 0 {
        resolve_path_nofollow_last(cwd, &raw_path)?
    } else {
        resolve_path(cwd, &raw_path)?
    };

    let inotify_file = get_inotify_file(fd)?;
    Ok(inotify_add_watch(inotify_file, dentry, mask)? as usize)
}

pub fn sys_inotify_rm_watch(fd: usize, wd: i32) -> SyscallResult {
    let inotify_file = get_inotify_file(fd)?;
    inotify_remove_watch(inotify_file, wd)?;
    Ok(0)
}

fn get_inotify_file(fd: usize) -> SysResult<Arc<InotifyFile>> {
    let process = current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return Err(SysError::EBADF);
    }
    let Some(file) = &inner.fd_table[fd] else {
        return Err(SysError::EBADF);
    };
    inotify_file_from_file(file).ok_or(SysError::EINVAL)
}
