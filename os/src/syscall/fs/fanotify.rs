#![allow(missing_docs)]

use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::config::FD_CLOEXEC_FLAG;
use crate::fs::notify::fanotify::{
    FAN_MARK_DONT_FOLLOW, FanotifyFile, create_fanotify_file, fanotify_file_from_file,
    fanotify_init_cloexec, fanotify_mark_file, fanotify_mark_needs_target, register_fanotify_file,
};
use crate::fs::vfs::path::{get_start_dentry, resolve_path, resolve_path_nofollow_last};
use crate::fs::vfs::{Dentry, File};
use crate::mm::translated_str;
use crate::task::{current_process, current_user_token};
use alloc::sync::Arc;

pub fn sys_fanotify_init(flags: u32, event_f_flags: u32) -> SyscallResult {
    let process = current_process();
    let unprivileged = process.inner_exclusive_access().euid != 0;
    let owner_pid = process.getpid();
    let file = create_fanotify_file(flags, event_f_flags, unprivileged, owner_pid)?;

    let mut inner = process.inner_exclusive_access();
    let fd = inner.alloc_fd()?;
    register_fanotify_file(&file)?;
    inner.fd_table[fd] = Some(file);
    if fanotify_init_cloexec(flags) && fd < inner.fd_flags.len() {
        inner.fd_flags[fd] |= FD_CLOEXEC_FLAG;
    }
    Ok(fd)
}

pub fn sys_fanotify_mark(
    fanotify_fd: usize,
    flags: u32,
    mask: u64,
    dirfd: isize,
    pathname: *const u8,
) -> SyscallResult {
    let fanotify_file = get_fanotify_file(fanotify_fd)?;
    let target = resolve_mark_target(flags, dirfd, pathname)?;
    fanotify_mark_file(fanotify_file, flags, mask, target)?;
    Ok(0)
}

fn get_fanotify_file(fd: usize) -> SysResult<Arc<FanotifyFile>> {
    let process = current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return Err(SysError::EBADF);
    }
    let Some(file) = &inner.fd_table[fd] else {
        return Err(SysError::EBADF);
    };
    fanotify_file_from_file(file).ok_or(SysError::EINVAL)
}

fn resolve_mark_target(
    flags: u32,
    dirfd: isize,
    pathname: *const u8,
) -> SysResult<Option<Arc<dyn Dentry>>> {
    if !fanotify_mark_needs_target(flags) {
        return Ok(None);
    }
    if pathname.is_null() {
        let process = current_process();
        let inner = process.inner_exclusive_access();
        if dirfd < 0 {
            return Err(SysError::EBADF);
        }
        let fd = dirfd as usize;
        let Some(Some(file)) = inner.fd_table.get(fd) else {
            return Err(SysError::EBADF);
        };
        if file.get_inode().is_none() {
            return Err(SysError::EINVAL);
        }
        return Ok(Some(file.get_dentry()));
    }

    let path = translated_str(current_user_token(), pathname)?;
    let start = get_start_dentry(dirfd, &path)?;
    let target = if flags & FAN_MARK_DONT_FOLLOW != 0 {
        resolve_path_nofollow_last(start, &path)?
    } else {
        resolve_path(start, &path)?
    };
    Ok(Some(target))
}
