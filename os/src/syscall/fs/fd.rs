use super::new_mount::{duplicate_fs_context, remove_fs_context};
use crate::error::{SysError, SyscallResult};
use crate::fs::config::{FD_CLOEXEC_FLAG, FD_FANOTIFY_EVENT};
use crate::fs::notify::{NotifyTarget, notify_close, notify_target_for_file_if_needed};
use crate::socket::{SOCKET_MANAGER, SocketFile};
use crate::task::current_process;
use alloc::sync::Arc;
use alloc::vec::Vec;
use log::warn;

pub fn sys_close(fd: usize) -> SyscallResult {
    let process = current_process();
    let pid = process.getpid();
    let mut inner = process.inner_exclusive_access();

    if fd >= inner.fd_table.len() {
        log::error!(
            "sys_close: pid={} fd={} EBADF len={}",
            pid,
            fd,
            inner.fd_table.len()
        );
        return Err(SysError::EBADF);
    }
    if inner.fd_table[fd].is_none() {
        log::error!("sys_close: pid={} fd={} EBADF none", pid, fd);
        return Err(SysError::EBADF);
    }
    let file = inner.fd_table[fd].take().unwrap();
    let is_socket = file.is_socket();
    let is_managed_socket = is_socket && SOCKET_MANAGER.lock().get_socket(fd, pid).is_some();
    log::info!(
        "sys_close: pid={} fd={} is_socket={} managed_socket={}",
        pid,
        fd,
        is_socket,
        is_managed_socket
    );
    let fd_flags = inner.fd_flags.get(fd).copied().unwrap_or(0);
    let is_socket = file.is_socket();
    let notify = notify_target_for_file_if_needed(&file).map(|target| (target, file.writable()));
    if fd < inner.fd_flags.len() {
        inner.fd_flags[fd] = 0;
    }
    drop(inner);
    if is_socket {
        let _ = SOCKET_MANAGER.lock().close_socket_with_refcount(fd, pid);
    }
    remove_fs_context(pid, fd);
    crate::fs::writeback::queue_file(file);
    if fd_flags & FD_FANOTIFY_EVENT == 0 {
        if let Some((target, writable)) = notify.as_ref() {
            notify_close(target, *writable);
        }
    }
    Ok(0)
}

/// close_range: close or mark file descriptors in the range [first, last].
pub fn sys_close_range(first: usize, last: usize, flags: u32) -> SyscallResult {
    const CLOSE_RANGE_UNSHARE: u32 = 1;
    const CLOSE_RANGE_CLOEXEC: u32 = 2;

    if first > last {
        return Err(SysError::EINVAL);
    }
    if flags & !(CLOSE_RANGE_UNSHARE | CLOSE_RANGE_CLOEXEC) != 0 {
        return Err(SysError::EINVAL);
    }

    let process = current_process();
    let pid = process.getpid();
    let mut inner = process.inner_exclusive_access();

    let max_fd = inner.fd_table.len().saturating_sub(1);
    let end = last.min(max_fd);

    if flags & CLOSE_RANGE_CLOEXEC != 0 {
        let fd_table_len = inner.fd_table.len();
        if inner.fd_flags.len() < fd_table_len {
            inner.fd_flags.resize(fd_table_len, 0);
        }
        for fd in first..=end {
            if inner.fd_table[fd].is_some() {
                inner.fd_flags[fd] |= FD_CLOEXEC_FLAG;
            }
        }
        return Ok(0);
    }

    let mut files_to_close: Vec<(
        usize,
        Arc<dyn crate::fs::File + Send + Sync>,
        Option<(NotifyTarget, bool)>,
        u32,
        bool,
    )> = Vec::new();
    for fd in first..=end {
        if let Some(file) = inner.fd_table[fd].take() {
            let fd_flags = inner.fd_flags.get(fd).copied().unwrap_or(0);
            let is_socket = file.is_socket();
            let notify =
                notify_target_for_file_if_needed(&file).map(|target| (target, file.writable()));
            if fd < inner.fd_flags.len() {
                inner.fd_flags[fd] = 0;
            }
            files_to_close.push((fd, file, notify, fd_flags, is_socket));
        }
    }
    drop(inner);

    for (fd, file, notify, fd_flags, is_socket) in files_to_close {
        if is_socket {
            let _ = SOCKET_MANAGER.lock().close_socket_with_refcount(fd, pid);
        }
        remove_fs_context(pid, fd);
        crate::fs::writeback::queue_file(file);
        if fd_flags & FD_FANOTIFY_EVENT == 0 {
            if let Some((target, writable)) = notify.as_ref() {
                notify_close(target, *writable);
            }
        }
    }

    Ok(0)
}

pub fn sys_dup(fd: usize) -> SyscallResult {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();

    let pid = process.getpid();
    let (file_clone, is_managed_socket) = if let Some(Some(file)) = inner.fd_table.get(fd) {
        let is_managed_socket =
            file.is_socket() && SOCKET_MANAGER.lock().get_socket(fd, pid).is_some();
        (file.clone(), is_managed_socket)
    } else {
        return Err(SysError::EBADF);
    };

    let new_fd = inner.alloc_fd()?;
    if is_managed_socket {
        inner.fd_table[new_fd] = Some(Arc::new(SocketFile {
            _fd: new_fd,
            _pid: pid,
        }));
        drop(inner);
        SOCKET_MANAGER.lock().dup_socket(fd, new_fd, pid)?;
    } else {
        inner.fd_table[new_fd] = Some(file_clone);
        drop(inner);
        duplicate_fs_context(pid, fd, new_fd);
    }
    Ok(new_fd)
}

pub fn sys_dup3(old_fd: usize, new_fd: usize, flags: usize) -> SyscallResult {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();

    let max_fd = inner.rlimit_nofile.rlim_cur as usize;
    if new_fd >= max_fd {
        return Err(SysError::EBADF);
    }

    if old_fd == new_fd {
        return Err(SysError::EINVAL);
    }

    const O_CLOEXEC: usize = 0o2000000;
    if flags != 0 && flags != O_CLOEXEC {
        return Err(SysError::EINVAL);
    }

    let pid = process.getpid();
    let (file_clone, old_is_managed_socket) = if let Some(Some(file)) = inner.fd_table.get(old_fd) {
        let is_managed_socket =
            file.is_socket() && SOCKET_MANAGER.lock().get_socket(old_fd, pid).is_some();
        (Some(file.clone()), is_managed_socket)
    } else if SOCKET_MANAGER.lock().get_socket(old_fd, pid).is_some() {
        log::warn!(
            "sys_dup3: pid={} old_fd={} missing from fd_table but present in socket manager",
            pid,
            old_fd
        );
        (None, true)
    } else {
        let open_fds: Vec<usize> = inner
            .fd_table
            .iter()
            .enumerate()
            .filter_map(|(idx, file)| file.as_ref().map(|_| idx))
            .collect();
        log::error!(
            "sys_dup3: pid={} old_fd={} new_fd={} EBADF open_fds={:?}",
            pid,
            old_fd,
            new_fd,
            open_fds
        );
        return Err(SysError::EBADF);
    };
    if new_fd >= inner.fd_table.len() {
        inner.fd_table.resize(new_fd + 1, None);
        inner.fd_flags.resize(new_fd + 1, 0);
    }

    let old_file = inner.fd_table[new_fd].take();
    let replaced_managed_socket = SOCKET_MANAGER.lock().get_socket(new_fd, pid).is_some();

    if old_is_managed_socket {
        inner.fd_table[new_fd] = Some(Arc::new(SocketFile {
            _fd: new_fd,
            _pid: pid,
        }));
    } else {
        inner.fd_table[new_fd] = file_clone;
    }
    if flags == O_CLOEXEC {
        inner.fd_flags[new_fd] = FD_CLOEXEC_FLAG;
    } else {
        inner.fd_flags[new_fd] = 0;
    }
    drop(inner);
    remove_fs_context(pid, new_fd);
    if old_is_managed_socket {
        SOCKET_MANAGER.lock().dup_socket(old_fd, new_fd, pid)?;
    } else {
        if replaced_managed_socket {
            let _ = SOCKET_MANAGER
                .lock()
                .close_socket_with_refcount(new_fd, pid);
        }
        duplicate_fs_context(pid, old_fd, new_fd);
    }
    if !replaced_managed_socket {
        if let Some(old_file) = old_file {
            crate::fs::writeback::queue_file(old_file);
        }
    }
    Ok(new_fd)
}

const F_DUPFD: usize = 0;
const F_GETFD: usize = 1;
const F_SETFD: usize = 2;
const F_GETFL: usize = 3;
const F_SETFL: usize = 4;
const F_DUPFD_CLOEXEC: usize = 1030;
const F_SETPIPE_SZ: usize = 1031;
const F_GETPIPE_SZ: usize = 1032;
const F_GET_SEALS: usize = 1034;
const F_SET_SEALS: usize = 1035;
const F_ADD_SEALS: usize = 1033;

pub fn sys_fcntl(fd: usize, cmd: usize, arg: usize) -> SyscallResult {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
        return Err(SysError::EBADF);
    }

    match cmd {
        F_DUPFD | F_DUPFD_CLOEXEC => {
            let file = inner.fd_table[fd].as_ref().unwrap().clone();
            let pid = process.getpid();
            let is_managed_socket =
                file.is_socket() && SOCKET_MANAGER.lock().get_socket(fd, pid).is_some();
            let max_fd = inner.rlimit_nofile.rlim_cur as usize;
            let mut new_fd = arg;
            while new_fd < max_fd.min(inner.fd_table.len()) && inner.fd_table[new_fd].is_some() {
                new_fd += 1;
            }
            if new_fd >= max_fd {
                return Err(SysError::EMFILE);
            }
            if new_fd >= inner.fd_table.len() {
                inner.fd_table.resize(new_fd + 1, None);
                inner.fd_flags.resize(new_fd + 1, 0);
            }
            if is_managed_socket {
                inner.fd_table[new_fd] = Some(Arc::new(SocketFile {
                    _fd: new_fd,
                    _pid: pid,
                }));
            } else {
                inner.fd_table[new_fd] = Some(file);
            }
            if cmd == F_DUPFD_CLOEXEC {
                inner.fd_flags[new_fd] = FD_CLOEXEC_FLAG;
            } else {
                inner.fd_flags[new_fd] = 0;
            }
            drop(inner);
            if is_managed_socket {
                if let Err(err) = SOCKET_MANAGER.lock().dup_socket(fd, new_fd, pid) {
                    let mut inner = process.inner_exclusive_access();
                    if new_fd < inner.fd_table.len() {
                        inner.fd_table[new_fd] = None;
                        inner.fd_flags[new_fd] = 0;
                    }
                    return Err(err);
                }
            } else {
                duplicate_fs_context(pid, fd, new_fd);
            }
            Ok(new_fd)
        }
        F_GETFD => {
            let pid = process.getpid();
            if let Some(sock) = SOCKET_MANAGER.lock().get_socket(fd, pid) {
                Ok((sock.flags & FD_CLOEXEC_FLAG) as usize)
            } else if fd < inner.fd_flags.len() {
                Ok((inner.fd_flags[fd] & FD_CLOEXEC_FLAG) as usize)
            } else {
                Ok(0)
            }
        }
        F_SETFD => {
            if fd < inner.fd_flags.len() {
                inner.fd_flags[fd] =
                    (inner.fd_flags[fd] & !FD_CLOEXEC_FLAG) | (arg as u32 & FD_CLOEXEC_FLAG);
            }
            let pid = process.getpid();
            if let Some(sock) = SOCKET_MANAGER.lock().get_socket_mut(fd, pid) {
                if (arg & FD_CLOEXEC_FLAG as usize) != 0 {
                    sock.flags |= FD_CLOEXEC_FLAG;
                } else {
                    sock.flags &= !FD_CLOEXEC_FLAG;
                }
            }
            if fd < inner.fd_flags.len() {
                if (arg & FD_CLOEXEC_FLAG as usize) != 0 {
                    inner.fd_flags[fd] |= FD_CLOEXEC_FLAG;
                } else {
                    inner.fd_flags[fd] &= !FD_CLOEXEC_FLAG;
                }
            }
            Ok(0)
        }
        F_GETFL => {
            let pid = process.getpid();
            if let Some(sock) = SOCKET_MANAGER.lock().get_socket(fd, pid) {
                Ok(0o2 | (sock.flags & !1) as usize)
            } else {
                let file = inner.fd_table[fd].as_ref().unwrap().clone();
                Ok(file.status_flags() as usize)
            }
        }
        F_SETFL => {
            let file = inner.fd_table[fd].as_ref().unwrap().clone();
            let pid = process.getpid();
            if let Some(sock) = SOCKET_MANAGER.lock().get_socket_mut(fd, pid) {
                let settable =
                    0o4000 | 0o2000 | 0o10000 | 0o40000 | 0o100000 | 0o1000000 | 0o4000000;
                sock.flags = (sock.flags & 1) | ((arg as u32) & settable);
            } else {
                file.set_status_flags(arg as u32);
            }
            Ok(0)
        }
        F_GETPIPE_SZ => {
            let file = inner.fd_table[fd].as_ref().unwrap().clone();
            drop(inner);
            if let Some(capacity) = file.pipe_capacity() {
                Ok(capacity)
            } else {
                Err(SysError::EINVAL)
            }
        }
        F_ADD_SEALS => {
            let file = inner.fd_table[fd].as_ref().unwrap();
            if let Some(inode) = file.get_inode() {
                inode.set_seals(arg as u64)?;
                Ok(0)
            } else {
                Err(SysError::EINVAL)
            }
        }
        F_SETPIPE_SZ => {
            let file = inner.fd_table[fd].as_ref().unwrap().clone();
            drop(inner);
            file.set_pipe_capacity(arg)?;
            if let Some(capacity) = file.pipe_capacity() {
                Ok(capacity)
            } else {
                Err(SysError::EINVAL)
            }
        }
        F_GET_SEALS => {
            let file = inner.fd_table[fd].as_ref().unwrap();
            if let Some(inode) = file.get_inode() {
                Ok(inode.get_seals() as usize)
            } else {
                Ok(0)
            }
        }
        F_SET_SEALS => {
            let file = inner.fd_table[fd].as_ref().unwrap();
            if let Some(inode) = file.get_inode() {
                inode.set_seals(arg as u64)?;
                Ok(0)
            } else {
                Err(SysError::EINVAL)
            }
        }
        _ => {
            warn!("Unsupported fcntl cmd: {}", cmd);
            Err(SysError::EINVAL)
        }
    }
}
