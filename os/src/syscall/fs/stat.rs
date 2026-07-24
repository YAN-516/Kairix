use crate::error::{SysError, SysResult, SyscallResult};
pub use crate::fs::file_handle::FileHandleHeader;
use crate::fs::file_handle::{FILE_HANDLE_BYTES, FILE_HANDLE_TYPE_INO, encode_file_handle};
use crate::fs::find_superblock_by_path;
use crate::fs::vfs::kstat::STATX_ATTR_MOUNT_ROOT;
use crate::fs::vfs::kstat::kstat_to_statx;
use crate::fs::vfs::kstat::{Kstat, Statfs, Statx};
use crate::fs::vfs::path::{AT_FDCWD, get_start_dentry, resolve_path, resolve_path_nofollow_last};
use crate::mm::{copy_to_user, translated_refmut, translated_str};
use crate::task::{current_process, current_user_token};
use alloc::sync::Arc;
use log::error;

use super::mount::{ST_VALID, statfs_flags_from_mount_flags};
use super::{check_open_path_len, mount_attr_flags_for_path};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct LinuxStat {
    st_dev: u64,
    st_ino: u64,
    st_mode: u32,
    st_nlink: u32,
    st_uid: u32,
    st_gid: u32,
    st_rdev: u64,
    __pad1: u64,
    st_size: i64,
    st_blksize: i32,
    __pad2: i32,
    st_blocks: u64,
    st_atime_sec: i64,
    st_atime_nsec: i64,
    st_mtime_sec: i64,
    st_mtime_nsec: i64,
    st_ctime_sec: i64,
    st_ctime_nsec: i64,
    __glibc_reserved: [i32; 2],
}

const _: [(); 128] = [(); core::mem::size_of::<LinuxStat>()];

fn is_registry_integrity_probe(path: &str) -> bool {
    path.ends_with("/.cache/an/yh/anyhow")
        || path.ends_with("/.cache/in/fe/inferno")
        || path.ends_with("/.cache/qe/mu/qemu-plugin")
}

fn kstat_to_linux_stat(stat: &Kstat) -> LinuxStat {
    LinuxStat {
        st_dev: stat.st_dev,
        st_ino: stat.st_ino,
        st_mode: stat.st_mode,
        st_nlink: stat.st_nlink,
        st_uid: stat.st_uid,
        st_gid: stat.st_gid,
        st_rdev: stat.st_rdev,
        __pad1: stat.__pad,
        st_size: stat.st_size,
        st_blksize: stat.st_blksize,
        __pad2: stat.__pad2,
        st_blocks: stat.st_blocks,
        st_atime_sec: stat.st_atime_sec,
        st_atime_nsec: stat.st_atime_nsec,
        st_mtime_sec: stat.st_mtime_sec,
        st_mtime_nsec: stat.st_mtime_nsec,
        st_ctime_sec: stat.st_ctime_sec,
        st_ctime_nsec: stat.st_ctime_nsec,
        __glibc_reserved: [0; 2],
    }
}

fn copy_linux_stat_to_user(token: usize, stat_buf: *mut u8, stat: &Kstat) -> SyscallResult {
    let user_stat = kstat_to_linux_stat(stat);
    let stat_bytes = unsafe {
        core::slice::from_raw_parts(
            &user_stat as *const _ as *const u8,
            core::mem::size_of::<LinuxStat>(),
        )
    };
    copy_to_user(token, stat_buf, stat_bytes)?;
    Ok(0)
}

pub fn sys_fstat(fd: usize, stat_buf: *mut u8) -> SyscallResult {
    if stat_buf.is_null() {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let process = current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return Err(SysError::EBADF);
    }
    if let Some(file) = &inner.fd_table[fd] {
        let file = file.clone();
        drop(inner);
        // Pipes, sockets, and other anonymous descriptors have valid fstat
        // implementations but deliberately have no VFS dentry.  Only inspect
        // a path for the registry probe after proving this is inode-backed.
        let registry_probe_path = file.get_inode().and_then(|_| {
            let path = file.get_dentry().path();
            is_registry_integrity_probe(&path).then_some(path)
        });
        let mut stat = Kstat::new();
        match file.get_stat(&mut stat) {
            Ok(_) => {
                if let Some(path) = registry_probe_path.as_ref() {
                    error!(
                        "[EXT4_REGISTRY_FSTAT] pid={} path={} fd={} inode={} size={} mode={:#o} nlink={}",
                        process.getpid(),
                        path,
                        fd,
                        stat.st_ino,
                        stat.st_size,
                        stat.st_mode,
                        stat.st_nlink
                    );
                }
                copy_linux_stat_to_user(token, stat_buf, &stat)
            }
            Err(e) => {
                if let Some(path) = registry_probe_path.as_ref() {
                    error!(
                        "[EXT4_REGISTRY_FSTAT] failed pid={} path={} fd={} error={:?}",
                        process.getpid(),
                        path,
                        fd,
                        e
                    );
                }
                Err(e)
            }
        }
    } else {
        Err(SysError::EBADF)
    }
}

fn stat_from_fd(fd: usize) -> SysResult<Kstat> {
    let process = current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return Err(SysError::EBADF);
    }
    let file = inner.fd_table[fd].as_ref().ok_or(SysError::EBADF)?.clone();
    drop(inner);

    let mut stat = Kstat::new();
    file.get_stat(&mut stat)?;
    Ok(stat)
}

fn stat_from_dentry(dentry: &Arc<dyn crate::fs::vfs::Dentry>) -> SysResult<Kstat> {
    let mut stat = Kstat::new();
    dentry.get_stat(&mut stat)?;
    Ok(stat)
}

pub fn sys_statx(
    fd: isize,
    pathname: *const u8,
    flags: u32,
    mask: usize,
    buf: *mut u8,
) -> SyscallResult {
    fn stage(value: usize) {
        if let Some(task) = crate::task::current_task() {
            task.set_active_syscall_stage(value);
        }
    }

    stage(29100);
    if buf.is_null() {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let raw_path = translated_str(token, pathname)?;
    stage(29101);
    const AT_EMPTY_PATH: u32 = 0x1000;
    const AT_SYMLINK_NOFOLLOW: u32 = 0x100;
    const AT_NO_AUTOMOUNT: u32 = 0x800;
    const AT_STATX_SYNC_TYPE: u32 = 0x6000;
    const STATX_RESERVED: usize = 0x8000_0000;
    const VALID_STATX_FLAGS: u32 =
        AT_EMPTY_PATH | AT_SYMLINK_NOFOLLOW | AT_NO_AUTOMOUNT | AT_STATX_SYNC_TYPE;

    if flags & !VALID_STATX_FLAGS != 0 || mask & STATX_RESERVED != 0 {
        return Err(SysError::EINVAL);
    }
    if !raw_path.is_empty() {
        check_open_path_len(&raw_path)?;
    }

    let stat = if raw_path.is_empty() {
        stage(29110);
        if (flags & AT_EMPTY_PATH) == 0 {
            return Err(SysError::ENOENT);
        }
        let process = current_process();
        if fd == AT_FDCWD {
            let inner = process.inner_exclusive_access();
            let cwd = inner.fs_context.lock().cwd.clone();
            drop(inner);
            let mut stat = Kstat::new();
            cwd.get_stat(&mut stat)?;
            mark_statx_mount_root(&cwd, &mut stat);
            stat
        } else {
            let inner = process.inner_exclusive_access();
            let fd = fd as usize;
            if fd >= inner.fd_table.len() {
                return Err(SysError::EBADF);
            }
            let file = match inner.fd_table[fd].as_ref() {
                Some(file) => file.clone(),
                None => return Err(SysError::EBADF),
            };
            drop(inner);
            let mut stat = Kstat::new();
            file.get_stat(&mut stat)?;
            if file.get_inode().is_some() {
                let dentry = file.get_dentry();
                mark_statx_mount_root(&dentry, &mut stat);
            }
            stat
        }
    } else {
        stage(29120);
        let start_dentry = get_start_dentry(fd, &raw_path)?;
        let target = if flags & AT_SYMLINK_NOFOLLOW != 0 {
            resolve_path_nofollow_last(start_dentry, &raw_path)?
        } else {
            resolve_path(start_dentry, &raw_path)?
        };
        stage(29121);
        let mut stat = Kstat::new();
        target.get_stat(&mut stat)?;
        stage(29122);
        mark_statx_mount_root(&target, &mut stat);
        stat
    };

    stage(29130);
    copy_statx_to_user(token, buf, &stat)
}

fn mark_statx_mount_root(dentry: &Arc<dyn crate::fs::vfs::Dentry>, stat: &mut Kstat) {
    let path = dentry.path();
    if find_superblock_by_path(&path).is_some_and(|sb| {
        let root = sb.root();
        Arc::ptr_eq(&root, dentry)
    }) {
        stat.stx_attributes |= STATX_ATTR_MOUNT_ROOT;
    }
}

fn copy_statx_to_user(token: usize, buf: *mut u8, stat: &Kstat) -> SyscallResult {
    let statx = kstat_to_statx(&stat);
    let stat_bytes = unsafe {
        core::slice::from_raw_parts(
            &statx as *const _ as *const u8,
            core::mem::size_of::<Statx>(),
        )
    };
    copy_to_user(token, buf, stat_bytes)?;

    Ok(0)
}

pub fn sys_fstatat(dirfd: isize, path: *const u8, stat_buf: *mut u8, flags: u32) -> SyscallResult {
    if stat_buf.is_null() {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let raw_path = translated_str(token, path)?;

    const AT_EMPTY_PATH: u32 = 0x1000;
    const AT_SYMLINK_NOFOLLOW: u32 = 0x100;
    const AT_NO_AUTOMOUNT: u32 = 0x800;
    let valid_flags = AT_EMPTY_PATH | AT_SYMLINK_NOFOLLOW | AT_NO_AUTOMOUNT;
    if flags & !valid_flags != 0 {
        return Err(SysError::EINVAL);
    }
    if !raw_path.is_empty() {
        check_open_path_len(&raw_path)?;
    }

    if raw_path.is_empty() {
        if (flags & AT_EMPTY_PATH) == 0 {
            return Err(SysError::ENOENT);
        }
        if dirfd == AT_FDCWD {
            let process = current_process();
            let cwd = process
                .inner_exclusive_access()
                .fs_context
                .lock()
                .cwd
                .clone();
            let stat = stat_from_dentry(&cwd)?;
            return copy_linux_stat_to_user(token, stat_buf, &stat);
        }
        let stat = stat_from_fd(dirfd as usize)?;
        return copy_linux_stat_to_user(token, stat_buf, &stat);
    }

    let start_dentry = get_start_dentry(dirfd, &raw_path)?;
    let target = if flags & AT_SYMLINK_NOFOLLOW != 0 {
        resolve_path_nofollow_last(start_dentry, &raw_path)?
    } else {
        resolve_path(start_dentry, &raw_path)?
    };
    let stat = stat_from_dentry(&target)?;
    copy_linux_stat_to_user(token, stat_buf, &stat)
}

pub fn sys_statfs(path: *const u8, buf: *mut u8) -> SyscallResult {
    if path.is_null() || buf.is_null() {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let raw_path = translated_str(token, path)?;
    let cwd = current_process()
        .inner_exclusive_access()
        .fs_context
        .lock()
        .cwd
        .clone();
    let dentry = match resolve_path(cwd, &raw_path) {
        Ok(d) => d,
        Err(_) => return Err(SysError::ENOENT),
    };
    let abs_path = dentry.path();
    let stat = statfs_for_path(&abs_path).ok_or(SysError::ENOENT)?;
    copy_statfs_to_user(token, buf, &stat)?;
    Ok(0)
}

fn statfs_for_path(path: &str) -> Option<Statfs> {
    let sb = find_superblock_by_path(path)?;
    let mut stat = sb.statfs();
    stat.f_flags |= statfs_flags_from_mount_flags(sb.inner().flags());
    stat.f_flags |= mount_attr_flags_for_path(path) as i64;
    Some(stat)
}

fn pipe_statfs() -> Statfs {
    const PIPEFS_MAGIC: i64 = 0x5049_5045;
    let mut stat = Statfs::new();
    stat.f_type = PIPEFS_MAGIC;
    stat.f_bsize = 4096;
    stat.f_frsize = 4096;
    stat.f_flags = ST_VALID;
    stat
}

fn copy_statfs_to_user(token: usize, buf: *mut u8, stat: &Statfs) -> SyscallResult {
    let stat_bytes = unsafe {
        core::slice::from_raw_parts(
            stat as *const _ as *const u8,
            core::mem::size_of::<Statfs>(),
        )
    };
    copy_to_user(token, buf, stat_bytes)?;
    Ok(0)
}

pub fn sys_fstatfs(fd: usize, buf: *mut u8) -> SyscallResult {
    if buf.is_null() {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let process = current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return Err(SysError::EBADF);
    }
    let file = inner.fd_table[fd].as_ref().ok_or(SysError::EBADF)?.clone();
    drop(inner);

    let stat = if file.is_pipe() {
        pipe_statfs()
    } else if file.get_inode().is_none() {
        return Err(SysError::EINVAL);
    } else {
        let path = file.get_dentry().path();
        statfs_for_path(&path).ok_or(SysError::ENOENT)?
    };
    copy_statfs_to_user(token, buf, &stat)?;
    Ok(0)
}

pub fn sys_name_to_handle_at(
    dirfd: isize,
    pathname: *const u8,
    handle: *mut FileHandleHeader,
    mount_id: *mut i32,
    flags: u32,
) -> SyscallResult {
    const AT_EMPTY_PATH: u32 = 0x1000;
    const AT_SYMLINK_FOLLOW: u32 = 0x400;
    const AT_HANDLE_FID: u32 = 0x200;
    let allowed = AT_EMPTY_PATH | AT_SYMLINK_FOLLOW | AT_HANDLE_FID;
    if flags & !allowed != 0 {
        return Err(SysError::EINVAL);
    }
    if pathname.is_null() || handle.is_null() || mount_id.is_null() {
        return Err(SysError::EFAULT);
    }

    let token = current_user_token();
    let raw_path = translated_str(token, pathname)?;
    if raw_path.is_empty() && flags & AT_EMPTY_PATH == 0 {
        return Err(SysError::ENOENT);
    }

    let dentry = if raw_path.is_empty() {
        let process = current_process();
        let inner = process.inner_exclusive_access();
        if dirfd < 0 {
            return Err(SysError::EBADF);
        }
        let fd = dirfd as usize;
        let Some(Some(file)) = inner.fd_table.get(fd) else {
            return Err(SysError::EBADF);
        };
        file.get_dentry()
    } else {
        let start = get_start_dentry(dirfd, &raw_path)?;
        if flags & AT_SYMLINK_FOLLOW != 0 {
            resolve_path(start, &raw_path)?
        } else {
            resolve_path_nofollow_last(start, &raw_path)?
        }
    };
    let inode = dentry.get_inode().ok_or(SysError::ENOENT)?;
    let handle_bytes = unsafe { (*handle).handle_bytes };
    if handle_bytes < FILE_HANDLE_BYTES {
        unsafe {
            (*handle).handle_bytes = FILE_HANDLE_BYTES;
        }
        return Err(SysError::EOVERFLOW);
    }

    unsafe {
        (*handle).handle_bytes = FILE_HANDLE_BYTES;
        (*handle).handle_type = FILE_HANDLE_TYPE_INO;
    }
    let encoded = encode_file_handle(inode.get_ino() as u64);
    copy_to_user(
        token,
        unsafe { (handle as *mut u8).add(core::mem::size_of::<FileHandleHeader>()) },
        &encoded,
    )?;
    *translated_refmut(token, mount_id)? = 1;
    Ok(0)
}

pub fn sys_open_by_handle_at(
    _mount_fd: isize,
    _handle: *const FileHandleHeader,
    _flags: u32,
) -> SyscallResult {
    Err(SysError::EOPNOTSUPP)
}
