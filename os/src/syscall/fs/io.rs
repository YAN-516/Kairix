use super::read_user_bytes;
use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::notify::{
    NotifyTarget, notify_access, notify_access_permission, notify_modify,
    notify_target_for_file_if_needed,
};
use crate::fs::tmpfs::inode::{F_SEAL_GROW, F_SEAL_SHRINK, F_SEAL_WRITE};
use crate::fs::vfs::OpenFlags;
use crate::fs::vfs::file::{File, open_file};
use crate::fs::vfs::inode::{Inode, InodeMode};
use crate::mm::{
    UserBuffer, translated_byte_buffer, translated_byte_buffer_for_write, translated_str,
};
use crate::security::landlock::{LANDLOCK_ACCESS_FS_TRUNCATE, landlock_check_dentry};
use crate::task::{current_process, current_user_token};
use crate::timer::realtime_timespec;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use log::{error, warn};
use polyhal::consts::PAGE_SIZE;

/// Linux MAX_LFS_FILESIZE for 64-bit: i64::MAX
const MAX_LFS_FILESIZE: usize = i64::MAX as usize;
static PREAD64_LOG_SEQ: AtomicUsize = AtomicUsize::new(0);
static PWRITE64_LOG_SEQ: AtomicUsize = AtomicUsize::new(0);
static READ_LOG_SEQ: AtomicUsize = AtomicUsize::new(0);
static WRITE_LOG_SEQ: AtomicUsize = AtomicUsize::new(0);
static FSYNC_LOG_SEQ: AtomicUsize = AtomicUsize::new(0);

fn log_file_io_eio<F: File + ?Sized>(
    operation: &str,
    pid: usize,
    fd: usize,
    file: &Arc<F>,
    offset: usize,
    len: usize,
    error: SysError,
) {
    if error != SysError::EIO {
        return;
    }
    let inode = file.get_inode();
    let inode_id = inode
        .as_ref()
        .map(|inode| inode.cache_inode_id().unwrap_or_else(|| inode.get_ino()));
    let file_size = inode.as_ref().map(|inode| inode.get_size());
    let path = file.get_dentry().path();
    error!(
        "[FILE_IO_EIO] op={} pid={} fd={} path={} inode={:?} offset={} len={} file_offset={} file_size={:?} error={:?} writeback_pending={:?} ext4_flush={:?} block_io={:?}",
        operation,
        pid,
        fd,
        path,
        inode_id,
        offset,
        len,
        file.get_offset(),
        file_size,
        error,
        crate::fs::writeback::try_pending_count(),
        crate::fs::lwext4::file::ext4_flush_stats(),
        crate::drivers::block::virtio_blk::virtio_block_io_stats(),
    );
}

/// Lock-free syscall activity counters used by stall diagnostics.
#[derive(Debug, Clone, Copy)]
pub struct IoActivityStats {
    /// Number of read syscall entries.
    pub reads: usize,
    /// Number of write syscall entries.
    pub writes: usize,
    /// Number of positional read syscall entries.
    pub preads: usize,
    /// Number of positional write syscall entries.
    pub pwrites: usize,
    /// Number of fsync/fdatasync syscall entries.
    pub fsyncs: usize,
}

/// Return current I/O activity without acquiring filesystem locks.
pub fn io_activity_stats() -> IoActivityStats {
    IoActivityStats {
        reads: READ_LOG_SEQ.load(Ordering::Relaxed),
        writes: WRITE_LOG_SEQ.load(Ordering::Relaxed),
        preads: PREAD64_LOG_SEQ.load(Ordering::Relaxed),
        pwrites: PWRITE64_LOG_SEQ.load(Ordering::Relaxed),
        fsyncs: FSYNC_LOG_SEQ.load(Ordering::Relaxed),
    }
}

#[cfg(board = "visionfive2")]
fn should_log_iozone_io(_seq: usize) -> bool {
    false
}

#[cfg(not(board = "visionfive2"))]
fn should_log_iozone_io(seq: usize) -> bool {
    seq <= 64 || seq % 256 == 0
}

fn is_registry_integrity_probe(path: &str) -> bool {
    path.ends_with("/.cache/an/yh/anyhow")
        || path.ends_with("/.cache/in/fe/inferno")
        || path.ends_with("/.cache/qe/mu/qemu-plugin")
}

fn user_read_fingerprint(token: usize, ptr: *const u8, len: usize) -> SysResult<(u32, u64)> {
    let buffers = translated_byte_buffer(token, ptr, len)?;
    let mut remaining = len;
    let mut hash = 5381u32;
    let mut prefix = 0u64;
    let mut prefix_len = 0usize;
    for buffer in buffers {
        let take = buffer.len().min(remaining);
        for byte in &buffer[..take] {
            hash = hash.wrapping_mul(33).wrapping_add(*byte as u32);
            if prefix_len < 8 {
                prefix |= (*byte as u64) << (prefix_len * 8);
                prefix_len += 1;
            }
        }
        remaining -= take;
        if remaining == 0 {
            break;
        }
    }
    if remaining == 0 {
        Ok((hash, prefix))
    } else {
        Err(SysError::EFAULT)
    }
}

/// Check whether writing `len` bytes at `offset` would exceed file size limits.
/// Returns EFBIG if it exceeds MAX_LFS_FILESIZE or the process's RLIMIT_FSIZE.
pub(super) fn check_write_size_limit(offset: usize, len: usize) -> SyscallResult {
    let end = match offset.checked_add(len) {
        Some(v) => v,
        None => return Err(SysError::EFBIG),
    };
    if end > MAX_LFS_FILESIZE {
        return Err(SysError::EFBIG);
    }
    let process = current_process();
    let inner = process.inner_exclusive_access();
    let rlimit_fsize = inner.rlimit_fsize.rlim_cur;
    drop(inner);
    if rlimit_fsize != u64::MAX {
        let limit = rlimit_fsize as usize;
        if end > limit {
            return Err(SysError::EFBIG);
        }
    }
    Ok(0)
}

/// Read ahead to populate the page cache.
pub fn sys_readahead(fd: usize, offset: usize, count: usize) -> SyscallResult {
    const MAX_READAHEAD_BYTES: usize = 1024 * 1024;

    let process = current_process();
    let inner = process.inner_exclusive_access();
    let file = match inner.fd_table.get(fd) {
        Some(Some(f)) => f.clone(),
        _ => {
            drop(inner);
            return Err(SysError::EBADF);
        }
    };

    if !file.readable() {
        drop(inner);
        return Err(SysError::EBADF);
    }
    if file.is_pipe() || file.is_socket() {
        drop(inner);
        return Err(SysError::EINVAL);
    }
    let inode = match file.get_inode() {
        Some(i) => i,
        None => {
            drop(inner);
            return Err(SysError::EINVAL);
        }
    };
    if file.is_path_only() {
        drop(inner);
        return Err(SysError::EINVAL);
    }
    if inode.get_mode().get_type() != InodeMode::FILE {
        drop(inner);
        return Err(SysError::EINVAL);
    }

    drop(inner);
    let prefetch_len = count.min(MAX_READAHEAD_BYTES);
    file.populate_page_cache(offset, prefetch_len)?;
    Ok(0)
}

/// Apply a POSIX file-access pattern hint.
pub fn sys_fadvise64(fd: usize, offset: usize, len: usize, advice: i32) -> SyscallResult {
    const POSIX_FADV_NORMAL: i32 = 0;
    const POSIX_FADV_RANDOM: i32 = 1;
    const POSIX_FADV_SEQUENTIAL: i32 = 2;
    const POSIX_FADV_WILLNEED: i32 = 3;
    const POSIX_FADV_DONTNEED: i32 = 4;
    const POSIX_FADV_NOREUSE: i32 = 5;
    const MAX_WILLNEED_BYTES: usize = 1024 * 1024;

    if !matches!(
        advice,
        POSIX_FADV_NORMAL
            | POSIX_FADV_RANDOM
            | POSIX_FADV_SEQUENTIAL
            | POSIX_FADV_WILLNEED
            | POSIX_FADV_DONTNEED
            | POSIX_FADV_NOREUSE
    ) {
        return Err(SysError::EINVAL);
    }
    if offset > i64::MAX as usize || len > i64::MAX as usize {
        return Err(SysError::EINVAL);
    }
    if len != 0 {
        let end = offset.checked_add(len).ok_or(SysError::EINVAL)?;
        if end > i64::MAX as usize {
            return Err(SysError::EINVAL);
        }
    }

    let file = {
        let process = current_process();
        let inner = process.inner_exclusive_access();
        inner
            .fd_table
            .get(fd)
            .and_then(|entry| entry.as_ref())
            .cloned()
            .ok_or(SysError::EBADF)?
    };
    if file.is_pipe() || file.is_socket() {
        return Err(SysError::ESPIPE);
    }
    if file.is_path_only() {
        return Err(SysError::EBADF);
    }
    let inode = file.get_inode().ok_or(SysError::EINVAL)?;
    if inode.get_mode().get_type() != InodeMode::FILE {
        return Err(SysError::EINVAL);
    }

    if advice == POSIX_FADV_WILLNEED && file.readable() {
        let file_size = inode.get_size();
        let requested = if len == 0 {
            file_size.saturating_sub(offset)
        } else {
            len
        };
        // The hint is advisory. Failure to populate cache pages must not turn
        // an otherwise valid posix_fadvise() call into an I/O failure.
        let _ = file.populate_page_cache(offset, requested.min(MAX_WILLNEED_BYTES));
    }
    Ok(0)
}

pub fn sys_write(fd: usize, buf: *const u8, len: usize) -> SyscallResult {
    let token = current_user_token();
    let process = current_process();
    let pid = process.getpid();
    let seq = WRITE_LOG_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let log_this = should_log_iozone_io(seq);
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return Err(SysError::EBADF);
    }
    if let Some(file) = &inner.fd_table[fd] {
        if !file.writable() {
            return Err(SysError::EBADF);
        }
        let inode = file.get_inode();
        if let Some(inode) = inode.as_ref() {
            if (inode.get_seals() & F_SEAL_WRITE) != 0 {
                return Err(SysError::EPERM);
            }
        }
        if file.is_pipe() || file.is_socket() || inode.is_none() {
            let file = file.clone();
            drop(inner);
            return file.write_user(token, buf, len);
        }

        let file = file.clone();
        let notify_target = notify_target_for_file_if_needed(&file);
        let offset = file.get_offset();
        let old_size = inode.as_ref().map(|inode| inode.get_size()).unwrap_or(0);
        let inode_id = file
            .get_inode()
            .as_ref()
            .map(|inode| inode.cache_inode_id().unwrap_or_else(|| inode.get_ino()));
        let path = file.get_dentry().path();
        drop(inner);

        check_write_size_limit(offset, len)?;
        if log_this {
            warn!(
                "[IOZONE_HANG write_enter] seq={} pid={} fd={} len={} buf={:#x} offset={} inode={:?} path={}",
                seq, pid, fd, len, buf as usize, offset, inode_id, path
            );
        }
        let written = match file.write_user(token, buf, len) {
            Ok(written) => written,
            Err(err) => {
                log_file_io_eio("write", pid, fd, &file, offset, len, err);
                warn!(
                    "[IOZONE_HANG write_err] seq={} pid={} fd={} len={} buf={:#x} offset={} inode={:?} path={} err={:?}",
                    seq, pid, fd, len, buf as usize, offset, inode_id, path, err
                );
                return Err(err);
            }
        };
        crate::fs::elf_trace::log_write_result(
            "write", pid, fd, &file, offset, len, written, old_size,
        );
        if log_this {
            warn!(
                "[IOZONE_HANG write_done] seq={} pid={} fd={} len={} buf={:#x} offset={} written={}",
                seq, pid, fd, len, buf as usize, offset, written
            );
        }
        if written > 0 {
            if let Some(target) = notify_target.as_ref() {
                notify_modify(target);
            }
        }
        Ok(written)
    } else {
        Err(SysError::EBADF)
    }
}

pub fn sys_read(fd: usize, buf: *const u8, len: usize) -> SyscallResult {
    let active_task = crate::task::current_task();
    if let Some(task) = active_task.as_ref() {
        task.set_active_syscall_stage(6300);
    }
    let token = current_user_token();
    let process = current_process();
    let pid = process.getpid();
    let seq = READ_LOG_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let log_this = should_log_iozone_io(seq);
    let file = {
        let inner = process.inner_exclusive_access();
        if let Some(task) = active_task.as_ref() {
            task.set_active_syscall_stage(6301);
        }
        if fd >= inner.fd_table.len() {
            return Err(SysError::EBADF);
        }
        inner.fd_table[fd].clone().ok_or(SysError::EBADF)?
    };
    if let Some(task) = active_task.as_ref() {
        task.set_active_syscall_stage(6302);
    }
    if !file.readable() {
        return Err(SysError::EBADF);
    }
    if file.is_pipe() || file.is_socket() || file.get_inode().is_none() {
        return file.read_user(token, buf as *mut u8, len);
    }

    let notify_target = notify_target_for_file_if_needed(&file);
    let offset = file.get_offset();
    let inode_id = file
        .get_inode()
        .as_ref()
        .map(|inode| inode.cache_inode_id().unwrap_or_else(|| inode.get_ino()));
    let path = file.get_dentry().path();
    if let Some(task) = active_task.as_ref() {
        task.set_active_syscall_stage(6303);
    }

    notify_access_permission(notify_target.as_ref())?;
    if let Some(task) = active_task.as_ref() {
        task.set_active_syscall_stage(6304);
    }

    if log_this {
        warn!(
            "[IOZONE_HANG read_enter] seq={} pid={} fd={} len={} buf={:#x} offset={} inode={:?} path={}",
            seq, pid, fd, len, buf as usize, offset, inode_id, path
        );
    }
    if let Some(task) = active_task.as_ref() {
        task.set_active_syscall_stage(6305);
    }
    let read_len = match file.read_user(token, buf as *mut u8, len) {
        Ok(read_len) => read_len,
        Err(err) => {
            if is_registry_integrity_probe(&path) {
                error!(
                    "[EXT4_REGISTRY_READ] failed pid={} path={} offset={} requested={} error={:?}",
                    pid, path, offset, len, err
                );
            }
            warn!(
                "[IOZONE_FREAD read_err] pid={} fd={} len={} buf={:#x} offset={} inode={:?} path={} err={:?}",
                pid, fd, len, buf as usize, offset, inode_id, path, err
            );
            warn!(
                "[IOZONE_HANG read_err] seq={} pid={} fd={} len={} buf={:#x} offset={} inode={:?} path={} err={:?}",
                seq, pid, fd, len, buf as usize, offset, inode_id, path, err
            );
            return Err(err);
        }
    };
    if is_registry_integrity_probe(&path) {
        match user_read_fingerprint(token, buf, read_len) {
            Ok((hash, prefix)) => error!(
                "[EXT4_REGISTRY_READ] done pid={} path={} offset={} requested={} returned={} hash={:#010x} prefix_le={:#018x}",
                pid, path, offset, len, read_len, hash, prefix
            ),
            Err(err) => error!(
                "[EXT4_REGISTRY_READ] fingerprint_failed pid={} path={} offset={} requested={} returned={} error={:?}",
                pid, path, offset, len, read_len, err
            ),
        }
    }
    if let Some(task) = active_task.as_ref() {
        task.set_active_syscall_stage(6306);
    }
    if log_this {
        warn!(
            "[IOZONE_HANG read_done] seq={} pid={} fd={} len={} buf={:#x} offset={} read={}",
            seq, pid, fd, len, buf as usize, offset, read_len
        );
    }
    if read_len > 0 {
        if let Some(target) = notify_target.as_ref() {
            notify_access(target);
        }
    }
    if let Some(task) = active_task.as_ref() {
        task.set_active_syscall_stage(6307);
    }
    Ok(read_len)
}

pub fn sys_pread64(fd: usize, buf: *const u8, len: usize, offset: usize) -> SyscallResult {
    let token = current_user_token();
    let process = current_process();
    let pid = process.getpid();
    let seq = PREAD64_LOG_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let log_this = should_log_iozone_io(seq);
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return Err(SysError::EBADF);
    }
    if let Some(file) = &inner.fd_table[fd] {
        let file = file.clone();
        let inode = file.get_inode();
        let inode_id = inode
            .as_ref()
            .map(|inode| inode.cache_inode_id().unwrap_or_else(|| inode.get_ino()));
        let notify_target = notify_target_for_file_if_needed(&file);
        drop(inner);

        if !file.readable() {
            return Err(SysError::EBADF);
        }
        if inode.is_none() {
            return Err(SysError::ESPIPE);
        }
        notify_access_permission(notify_target.as_ref())?;

        if log_this {
            warn!(
                "[IOZONE_HANG pread64_enter] seq={} pid={} fd={} len={} offset={} inode={:?}",
                seq, pid, fd, len, offset, inode_id
            );
        }
        let buffers = match translated_byte_buffer_for_write(token, buf as *mut u8, len) {
            Ok(buffers) => buffers,
            Err(err) => {
                warn!(
                    "[IOZONE_HANG pread64_translate_err] seq={} pid={} fd={} len={} offset={} err={:?}",
                    seq, pid, fd, len, offset, err
                );
                return Err(err);
            }
        };
        let user_buf = UserBuffer::new(buffers);
        let read_len = match file.read_at(offset, user_buf) {
            Ok(read_len) => read_len,
            Err(err) => {
                warn!(
                    "[IOZONE_HANG pread64_read_err] seq={} pid={} fd={} len={} offset={} err={:?}",
                    seq, pid, fd, len, offset, err
                );
                return Err(err);
            }
        };
        if log_this {
            warn!(
                "[IOZONE_HANG pread64_done] seq={} pid={} fd={} len={} offset={} read={}",
                seq, pid, fd, len, offset, read_len
            );
        }
        if read_len > 0 {
            if let Some(target) = notify_target.as_ref() {
                notify_access(target);
            }
        }
        Ok(read_len)
    } else {
        Err(SysError::EBADF)
    }
}

pub fn sys_pwrite64(fd: usize, buf: *const u8, len: usize, offset: usize) -> SyscallResult {
    let token = current_user_token();
    let process = current_process();
    let pid = process.getpid();
    let seq = PWRITE64_LOG_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let log_this = should_log_iozone_io(seq);
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return Err(SysError::EBADF);
    }
    if let Some(file) = &inner.fd_table[fd] {
        let file = file.clone();
        let inode = file.get_inode();
        let old_size = inode.as_ref().map(|inode| inode.get_size()).unwrap_or(0);
        let inode_id = inode
            .as_ref()
            .map(|inode| inode.cache_inode_id().unwrap_or_else(|| inode.get_ino()));
        let notify_target = notify_target_for_file_if_needed(&file);
        drop(inner);

        if !file.writable() {
            return Err(SysError::EBADF);
        }
        if inode.is_none() {
            return Err(SysError::ESPIPE);
        }

        check_write_size_limit(offset, len)?;

        if log_this {
            warn!(
                "[IOZONE_HANG pwrite64_enter] seq={} pid={} fd={} len={} offset={} inode={:?}",
                seq, pid, fd, len, offset, inode_id
            );
        }
        let buffers = match translated_byte_buffer(token, buf, len) {
            Ok(buffers) => buffers,
            Err(err) => {
                warn!(
                    "[IOZONE_HANG pwrite64_translate_err] seq={} pid={} fd={} len={} offset={} err={:?}",
                    seq, pid, fd, len, offset, err
                );
                return Err(err);
            }
        };
        let user_buf = UserBuffer::new(buffers);
        let written = match file.write_at(offset, user_buf) {
            Ok(written) => written,
            Err(err) => {
                log_file_io_eio("pwrite64", pid, fd, &file, offset, len, err);
                warn!(
                    "[IOZONE_HANG pwrite64_write_err] seq={} pid={} fd={} len={} offset={} err={:?}",
                    seq, pid, fd, len, offset, err
                );
                return Err(err);
            }
        };
        crate::fs::elf_trace::log_write_result(
            "pwrite64", pid, fd, &file, offset, len, written, old_size,
        );
        if log_this {
            warn!(
                "[IOZONE_HANG pwrite64_done] seq={} pid={} fd={} len={} offset={} written={}",
                seq, pid, fd, len, offset, written
            );
        }
        if written > 0 {
            if let Some(target) = notify_target.as_ref() {
                notify_modify(target);
            }
        }
        Ok(written)
    } else {
        Err(SysError::EBADF)
    }
}

pub fn sys_lseek(fd: usize, offset: isize, whence: i32) -> SyscallResult {
    const SEEK_DATA: i32 = 3;
    const SEEK_HOLE: i32 = 4;

    let process = current_process();
    let file = {
        let inner = process.inner_exclusive_access();
        if fd >= inner.fd_table.len() {
            return Err(SysError::EBADF);
        }
        match inner.fd_table[fd].as_ref() {
            Some(f) => f.clone(),
            None => return Err(SysError::EBADF),
        }
    };

    if whence == SEEK_DATA || whence == SEEK_HOLE {
        let inode = match file.get_inode() {
            Some(inode) => inode,
            None => return Err(SysError::ESPIPE),
        };
        if inode.get_mode().get_type() == InodeMode::DIR {
            return Err(SysError::EINVAL);
        }
        if offset < 0 {
            return Err(SysError::EINVAL);
        }
        let start = offset as usize;
        let size = inode.get_size();
        if start >= size {
            return Err(SysError::ENXIO);
        }
        let new_off = find_data_or_hole_offset(file.clone(), start, size, whence == SEEK_HOLE)?;
        file.set_offset(new_off);
        return Ok(new_off);
    }

    file.seek_position(offset, whence)
}

fn find_data_or_hole_offset(
    file: Arc<dyn File>,
    start: usize,
    size: usize,
    find_hole: bool,
) -> SysResult<usize> {
    let mut pos = start;
    let old_offset = file.get_offset();
    let mut buf = [0u8; PAGE_SIZE];
    let inode = file.get_inode();

    while pos < size {
        if let Some(inode) = inode.as_ref() {
            let page_id = pos / PAGE_SIZE;
            if inode.is_punched_hole_page(page_id) {
                if find_hole {
                    file.set_offset(old_offset);
                    return Ok(pos);
                }
                pos = ((page_id + 1) * PAGE_SIZE).min(size);
                continue;
            }
        }

        let page_end = ((pos / PAGE_SIZE) + 1) * PAGE_SIZE;
        let chunk_end = page_end.min(size);
        let len = chunk_end - pos;
        let static_buf: &'static mut [u8] =
            unsafe { core::slice::from_raw_parts_mut(buf.as_mut_ptr(), len) };
        file.set_offset(pos);
        let read_len = match file.read(UserBuffer::new(vec![static_buf])) {
            Ok(n) => n,
            Err(_) => {
                file.set_offset(old_offset);
                return Err(SysError::EINVAL);
            }
        };
        if read_len == 0 {
            if find_hole {
                file.set_offset(old_offset);
                return Ok(pos);
            }
            break;
        }

        let has_data = buf[..read_len].iter().any(|byte| *byte != 0);
        if find_hole {
            if !has_data {
                file.set_offset(old_offset);
                return Ok(pos);
            }
        } else if has_data {
            file.set_offset(old_offset);
            return Ok(pos);
        }
        pos += read_len;
    }

    file.set_offset(old_offset);
    if find_hole {
        Ok(size)
    } else {
        Err(SysError::ENXIO)
    }
}

pub fn sys_fsync(fd: usize) -> SyscallResult {
    let process = current_process();
    let pid = process.getpid();
    let inner = process.inner_exclusive_access();

    if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
        return Err(SysError::EBADF);
    }
    let file = inner.fd_table[fd].as_ref().unwrap().clone();
    drop(inner);
    if file.is_pipe() || file.is_socket() {
        return Err(SysError::EINVAL);
    }
    if file
        .get_inode()
        .is_some_and(|inode| inode.get_mode().get_type() == InodeMode::FIFO)
    {
        return Err(SysError::EINVAL);
    }
    let seq = FSYNC_LOG_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let inode_id = file
        .get_inode()
        .as_ref()
        .map(|inode| inode.cache_inode_id().unwrap_or_else(|| inode.get_ino()));
    let path = file.get_dentry().path();
    warn!(
        "[IOZONE_HANG fsync_enter] seq={} pid={} fd={} inode={:?} path={}",
        seq, pid, fd, inode_id, path
    );
    if let Err(err) = file.fsync() {
        log_file_io_eio("fsync", pid, fd, &file, file.get_offset(), 0, err);
        return Err(err);
    }
    warn!(
        "[IOZONE_HANG fsync_done] seq={} pid={} fd={} inode={:?} path={}",
        seq, pid, fd, inode_id, path
    );
    Ok(0)
}

pub fn sys_syncfs(fd: usize) -> SyscallResult {
    let process = current_process();
    let pid = process.getpid();
    let inner = process.inner_exclusive_access();

    if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
        return Err(SysError::EBADF);
    }
    let file = inner.fd_table[fd].as_ref().unwrap().clone();
    drop(inner);
    crate::fs::writeback::drain_all();
    if let Err(err) = file.fsync() {
        log_file_io_eio("syncfs", pid, fd, &file, file.get_offset(), 0, err);
        return Err(err);
    }
    Ok(0)
}

pub fn sys_sync_file_range(fd: usize, offset: i64, nbytes: i64, flags: u32) -> SyscallResult {
    const SYNC_FILE_RANGE_WAIT_BEFORE: u32 = 1;
    const SYNC_FILE_RANGE_WRITE: u32 = 2;
    const SYNC_FILE_RANGE_WAIT_AFTER: u32 = 4;
    const VALID_FLAGS: u32 =
        SYNC_FILE_RANGE_WAIT_BEFORE | SYNC_FILE_RANGE_WRITE | SYNC_FILE_RANGE_WAIT_AFTER;

    if flags & !VALID_FLAGS != 0 {
        return Err(SysError::EINVAL);
    }
    if offset < 0 || nbytes < 0 {
        return Err(SysError::EINVAL);
    }
    if nbytes > 0 && offset.checked_add(nbytes).is_none() {
        return Err(SysError::EINVAL);
    }

    let process = current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
        return Err(SysError::EBADF);
    }
    let file = inner.fd_table[fd].as_ref().unwrap().clone();
    drop(inner);

    match file.get_inode() {
        Some(inode) => {
            if !inode.get_mode().contains(InodeMode::FILE) {
                return Err(SysError::ESPIPE);
            }
        }
        None => return Err(SysError::ESPIPE),
    }

    file.flush();
    Ok(0)
}

pub fn sys_ftruncate(fd: usize, length: usize) -> SyscallResult {
    let process = current_process();
    let pid = process.getpid();
    let inner = process.inner_exclusive_access();

    if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
        return Err(SysError::EBADF);
    }
    let file = inner.fd_table[fd].as_ref().unwrap().clone();
    drop(inner);

    if length > MAX_LFS_FILESIZE {
        return Err(SysError::EINVAL);
    }
    if file.is_socket() || file.is_pipe() || !file.writable() {
        return Err(SysError::EINVAL);
    }
    let inode = file.get_inode().ok_or(SysError::EINVAL)?;
    if !inode.get_mode().contains(InodeMode::FILE) {
        return Err(SysError::EINVAL);
    }

    let seals = inode.get_seals();
    let current_size = inode.get_size();
    if length < current_size && (seals & F_SEAL_SHRINK) != 0 {
        return Err(SysError::EPERM);
    }
    if length > current_size && (seals & F_SEAL_GROW) != 0 {
        return Err(SysError::EPERM);
    }

    let target = file.get_dentry();
    landlock_check_dentry(&target, LANDLOCK_ACCESS_FS_TRUNCATE)?;
    crate::fs::elf_trace::log_truncate("before", pid, Some(fd), &file, current_size, length);
    if let Err(err) = file.truncate(length as u64) {
        log_file_io_eio("ftruncate", pid, fd, &file, current_size, length, err);
        return Err(err);
    }
    crate::fs::elf_trace::log_truncate("after", pid, Some(fd), &file, current_size, length);
    notify_modify(&NotifyTarget::new(target));
    Ok(0)
}

pub fn sys_truncate(path: *const u8, length: usize) -> SyscallResult {
    check_write_size_limit(0, length)?;
    let token = current_user_token();
    let path_str = translated_str(token, path)?;
    let cwd = current_process()
        .inner_exclusive_access()
        .fs_context
        .lock()
        .cwd
        .clone();
    let file = open_file(cwd, &path_str, OpenFlags::WRONLY, InodeMode::FILE)?;
    let inode = file.get_inode().ok_or(SysError::ENOENT)?;
    if !inode.get_mode().contains(InodeMode::FILE) {
        return Err(SysError::EINVAL);
    }
    let target = file.get_dentry();
    landlock_check_dentry(&target, LANDLOCK_ACCESS_FS_TRUNCATE)?;
    let pid = current_process().getpid();
    let current_size = inode.get_size();
    crate::fs::elf_trace::log_truncate("before", pid, None, &file, current_size, length);
    if let Err(err) = file.truncate(length as u64) {
        log_file_io_eio(
            "truncate",
            pid,
            usize::MAX,
            &file,
            inode.get_size(),
            length,
            err,
        );
        return Err(err);
    }
    crate::fs::elf_trace::log_truncate("after", pid, None, &file, current_size, length);
    notify_modify(&NotifyTarget::new(target));
    Ok(0)
}

pub fn sys_fallocate(fd: usize, mode: i32, offset: usize, len: usize) -> SyscallResult {
    const FALLOC_FL_KEEP_SIZE: i32 = 0x01;
    const FALLOC_FL_PUNCH_HOLE: i32 = 0x02;
    const FALLOC_FL_COLLAPSE_RANGE: i32 = 0x08;
    const FALLOC_FL_ZERO_RANGE: i32 = 0x10;
    const FALLOC_FL_INSERT_RANGE: i32 = 0x20;
    let process = current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
        return Err(SysError::EBADF);
    }
    let file = inner.fd_table[fd].as_ref().unwrap().clone();
    drop(inner);

    if !file.writable() {
        return Err(SysError::EBADF);
    }
    let inode = match file.get_inode() {
        Some(inode) => inode,
        None => return Err(SysError::ENODEV),
    };
    if !inode.get_mode().contains(InodeMode::FILE) {
        return Err(SysError::EOPNOTSUPP);
    }
    let notify_target = notify_target_for_file_if_needed(&file);
    if len == 0 {
        return Ok(0);
    }
    if mode == 0 || (mode & FALLOC_FL_ZERO_RANGE) != 0 {
        check_write_size_limit(offset, len)?;
    }
    let end = match offset.checked_add(len) {
        Some(v) => v,
        None => return Err(SysError::EFBIG),
    };
    let supported_modes = FALLOC_FL_KEEP_SIZE
        | FALLOC_FL_PUNCH_HOLE
        | FALLOC_FL_COLLAPSE_RANGE
        | FALLOC_FL_ZERO_RANGE
        | FALLOC_FL_INSERT_RANGE;
    if (mode & !supported_modes) != 0 {
        return Err(SysError::EOPNOTSUPP);
    }
    if (mode & FALLOC_FL_COLLAPSE_RANGE) != 0 {
        if mode & !FALLOC_FL_COLLAPSE_RANGE != 0 {
            return Err(SysError::EINVAL);
        }
        if offset % PAGE_SIZE != 0 || len % PAGE_SIZE != 0 || end > inode.get_size() {
            return Err(SysError::EINVAL);
        }
        let current_size = inode.get_size();
        let new_size = current_size - len;
        shift_file_range(file.clone(), end, offset, current_size - end)?;
        file.truncate(new_size as u64)?;
        inode.clear_punched_holes();
        touch_modified_inode(inode.clone());
        if let Some(target) = notify_target.as_ref() {
            notify_modify(target);
        }
        return Ok(0);
    }
    if (mode & FALLOC_FL_INSERT_RANGE) != 0 {
        if mode & !FALLOC_FL_INSERT_RANGE != 0 {
            return Err(SysError::EINVAL);
        }
        if offset % PAGE_SIZE != 0 || len % PAGE_SIZE != 0 || offset > inode.get_size() {
            return Err(SysError::EINVAL);
        }
        let current_size = inode.get_size();
        let new_size = current_size.checked_add(len).ok_or(SysError::EFBIG)?;
        check_write_size_limit(0, new_size)?;
        let result = (|| -> SysResult<()> {
            file.truncate(new_size as u64)?;
            shift_file_range_reverse(file.clone(), offset, offset + len, current_size - offset)?;
            zero_file_range(file.clone(), offset, len)?;
            Ok(())
        })();
        if let Err(err) = result {
            let _ = file.truncate(current_size as u64);
            return Err(err);
        }
        inode.clear_punched_holes();
        touch_modified_inode(inode.clone());
        if let Some(target) = notify_target.as_ref() {
            notify_modify(target);
        }
        return Ok(0);
    }
    if (mode & FALLOC_FL_ZERO_RANGE) != 0 {
        if mode & !(FALLOC_FL_ZERO_RANGE | FALLOC_FL_KEEP_SIZE) != 0 {
            return Err(SysError::EINVAL);
        }
        let current_size = inode.get_size();
        let zero_len = current_size.saturating_sub(offset).min(len);
        zero_file_range(file.clone(), offset, zero_len)?;
        if (mode & FALLOC_FL_KEEP_SIZE) == 0 && end > current_size {
            file.truncate(end as u64)?;
        }
        touch_modified_inode(inode.clone());
        if let Some(target) = notify_target.as_ref() {
            notify_modify(target);
        }
        return Ok(0);
    }

    if (mode & FALLOC_FL_PUNCH_HOLE) != 0 {
        if (mode & FALLOC_FL_KEEP_SIZE) == 0 {
            return Err(SysError::EOPNOTSUPP);
        }
        if inode.get_seals() & F_SEAL_WRITE != 0 {
            return Err(SysError::EPERM);
        }

        let current_size = inode.get_size();
        let punch_end = end.min(current_size);
        if offset < punch_end {
            use crate::fs::page::pagecache::PAGE_CACHE;
            let ino = inode.cache_inode_id().unwrap_or_else(|| inode.get_ino());
            let start_page = offset / PAGE_SIZE;
            let end_page = (punch_end + PAGE_SIZE - 1) / PAGE_SIZE;
            for page_id in start_page..end_page {
                let cached_page = PAGE_CACHE.get_page(ino, page_id);
                if let Some(page) = cached_page {
                    let mut page_writer = page.write();
                    let page_start = page_id * PAGE_SIZE;
                    let page_end = (page_id + 1) * PAGE_SIZE;
                    let data_start = if page_start < offset {
                        offset - page_start
                    } else {
                        0
                    };
                    let data_end = if page_end > punch_end {
                        punch_end - page_start
                    } else {
                        PAGE_SIZE
                    };
                    if data_start < data_end {
                        page_writer.ensure_resident()?.ppn.get_bytes_array()[data_start..data_end]
                            .fill(0);
                        page_writer.mark_dirty_with_generation(inode.page_cache_generation());
                    }
                }
                let full_page_start = page_id * PAGE_SIZE;
                let full_page_end = full_page_start + PAGE_SIZE;
                if offset <= full_page_start && full_page_end <= punch_end {
                    inode.add_punched_hole_page(page_id);
                }
            }
            touch_modified_inode(inode.clone());
            if let Some(target) = notify_target.as_ref() {
                notify_modify(target);
            }
        }
        return Ok(0);
    }

    let current_size = inode.get_size();
    if mode == 0 && end > current_size {
        file.truncate(end as u64)?;
        if let Some(target) = notify_target.as_ref() {
            notify_modify(target);
        }
    }
    Ok(0)
}

fn touch_modified_inode(inode: Arc<dyn Inode>) {
    let (now_sec, now_nsec) = realtime_timespec();
    inode.set_mtime(now_sec, now_nsec);
    inode.set_ctime(now_sec, now_nsec);
}

fn zero_file_range(file: Arc<dyn File>, offset: usize, len: usize) -> SysResult<()> {
    if len == 0 {
        return Ok(());
    }

    let inode = file.get_inode().ok_or(SysError::ENODEV)?;
    let end = offset.checked_add(len).ok_or(SysError::EFBIG)?;
    let start_page = offset / PAGE_SIZE;
    let end_page = (end + PAGE_SIZE - 1) / PAGE_SIZE;
    let zero_page = [0u8; PAGE_SIZE];

    for page_id in start_page..end_page {
        let page_start = page_id * PAGE_SIZE;
        let page_end = page_start + PAGE_SIZE;
        let data_start = offset.saturating_sub(page_start);
        let data_end = end.min(page_end) - page_start;
        if data_start >= data_end {
            continue;
        }
        inode.clear_punched_hole_page(page_id);
        let mut written = 0usize;
        let len = data_end - data_start;
        while written < len {
            let chunk = (len - written).min(PAGE_SIZE);
            let n = write_file_range(
                file.clone(),
                page_start + data_start + written,
                &zero_page[..chunk],
            )?;
            if n == 0 {
                return Err(SysError::EIO);
            }
            written += n;
        }
    }

    Ok(())
}

fn read_file_range(file: Arc<dyn File>, offset: usize, buf: &mut [u8]) -> SysResult<usize> {
    let old_offset = file.get_offset();
    let static_buf: &'static mut [u8] =
        unsafe { core::slice::from_raw_parts_mut(buf.as_mut_ptr(), buf.len()) };
    file.set_offset(offset);
    let ret = file.read(UserBuffer::new(vec![static_buf]));
    file.set_offset(old_offset);
    ret
}

fn write_file_range(file: Arc<dyn File>, offset: usize, buf: &[u8]) -> SysResult<usize> {
    let old_offset = file.get_offset();
    let mut data = Vec::from(buf);
    let static_buf: &'static mut [u8] =
        unsafe { core::slice::from_raw_parts_mut(data.as_mut_ptr(), data.len()) };
    file.set_offset(offset);
    let ret = file.write(UserBuffer::new(vec![static_buf]));
    file.set_offset(old_offset);
    ret
}

fn shift_file_range(
    file: Arc<dyn File>,
    src_offset: usize,
    dst_offset: usize,
    len: usize,
) -> SysResult<()> {
    let mut copied = 0usize;
    let mut buf = [0u8; PAGE_SIZE];
    while copied < len {
        let chunk = (len - copied).min(PAGE_SIZE);
        let read_len = read_file_range(file.clone(), src_offset + copied, &mut buf[..chunk])?;
        if read_len == 0 {
            break;
        }
        let write_len = write_file_range(file.clone(), dst_offset + copied, &buf[..read_len])?;
        if write_len != read_len {
            return Err(SysError::EIO);
        }
        copied += read_len;
    }
    Ok(())
}

fn shift_file_range_reverse(
    file: Arc<dyn File>,
    src_offset: usize,
    dst_offset: usize,
    len: usize,
) -> SysResult<()> {
    let mut remaining = len;
    let mut buf = [0u8; PAGE_SIZE];
    while remaining > 0 {
        let chunk = remaining.min(PAGE_SIZE);
        remaining -= chunk;
        let read_len = read_file_range(file.clone(), src_offset + remaining, &mut buf[..chunk])?;
        if read_len == 0 {
            zero_file_range(file.clone(), dst_offset + remaining, chunk)?;
        } else {
            let write_len =
                write_file_range(file.clone(), dst_offset + remaining, &buf[..read_len])?;
            if write_len != read_len {
                return Err(SysError::EIO);
            }
        }
    }
    Ok(())
}

pub fn sys_sync() -> SyscallResult {
    crate::fs::writeback::drain_all();
    let mut files = Vec::new();
    let processes = crate::task::all_processes();
    for process in &processes {
        if let Some(inner) = process.inner_try_access() {
            for fd in 0..inner.fd_table.len() {
                if let Some(file) = inner.fd_table[fd].as_ref() {
                    files.push(file.clone());
                }
            }
        }
    }
    for file in files {
        file.flush();
    }
    let _ = crate::fs::lwext4::flush_all_lwext4_mounts();
    crate::mm::reclaim::trim_clean_page_cache_to_limit();
    Ok(0)
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IoVec {
    pub base: usize,
    pub len: usize,
}

const IOV_MAX: usize = 1024;

fn read_iovec(token: usize, iov_ptr: usize, iovcnt: usize) -> SysResult<Vec<IoVec>> {
    if iovcnt > IOV_MAX {
        return Err(SysError::EINVAL);
    }
    if iovcnt == 0 {
        return Ok(Vec::new());
    }
    if iov_ptr == 0 {
        return Err(SysError::EFAULT);
    }

    let iov_size = core::mem::size_of::<IoVec>();
    let bytes_len = iovcnt.checked_mul(iov_size).ok_or(SysError::EINVAL)?;
    let raw = read_user_bytes(token, iov_ptr as *const u8, bytes_len)?;
    let mut iovs = Vec::with_capacity(iovcnt);
    for chunk in raw.chunks_exact(iov_size) {
        let base = usize::from_ne_bytes(chunk[0..8].try_into().map_err(|_| SysError::EFAULT)?);
        let len = usize::from_ne_bytes(chunk[8..16].try_into().map_err(|_| SysError::EFAULT)?);
        iovs.push(IoVec { base, len });
    }
    Ok(iovs)
}

fn total_iov_len(iovs: &[IoVec]) -> SysResult<usize> {
    let mut total = 0usize;
    for iov in iovs {
        total = total.checked_add(iov.len).ok_or(SysError::EINVAL)?;
        if total > isize::MAX as usize {
            return Err(SysError::EINVAL);
        }
    }
    Ok(total)
}

pub fn sys_writev(fd: usize, iov_ptr: usize, iovcnt: usize) -> SyscallResult {
    let process = current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
        return Err(SysError::EBADF);
    }
    let file = inner.fd_table[fd].as_ref().unwrap().clone();
    if !file.writable() {
        return Err(SysError::EBADF);
    }
    let notify_target = notify_target_for_file_if_needed(&file);
    drop(inner);

    let token = current_user_token();
    let mut total_written = 0;
    let iovs = read_iovec(token, iov_ptr, iovcnt)?;
    let start_offset = file.get_offset();
    let old_size = file
        .get_inode()
        .as_ref()
        .map(|inode| inode.get_size())
        .unwrap_or(0);
    let requested_len = total_iov_len(&iovs)?;
    check_write_size_limit(start_offset, requested_len)?;

    let mut buffers = Vec::new();
    for iov in iovs {
        if iov.len == 0 {
            continue;
        }
        match translated_byte_buffer(token, iov.base as *const u8, iov.len) {
            Ok(iov_buffers) => buffers.extend(iov_buffers),
            Err(_) if !buffers.is_empty() => break,
            Err(err) => return Err(err),
        }
    }
    if !buffers.is_empty() {
        let user_buffer = UserBuffer::new(buffers);
        total_written = match file.write(user_buffer) {
            Ok(written) => written,
            Err(err) => {
                if !file.is_pipe() && !file.is_socket() {
                    log_file_io_eio(
                        "writev",
                        process.getpid(),
                        fd,
                        &file,
                        start_offset,
                        requested_len,
                        err,
                    );
                }
                return Err(err);
            }
        };
    }
    crate::fs::elf_trace::log_write_result(
        "writev",
        process.getpid(),
        fd,
        &file,
        start_offset,
        requested_len,
        total_written,
        old_size,
    );
    if total_written > 0 {
        if let Some(target) = notify_target.as_ref() {
            notify_modify(target);
        }
    }
    Ok(total_written)
}

pub fn sys_readv(fd: usize, iov_ptr: usize, iovcnt: usize) -> SyscallResult {
    let process = current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
        return Err(SysError::EBADF);
    }
    let file = inner.fd_table[fd].as_ref().unwrap().clone();
    if !file.readable() {
        return Err(SysError::EBADF);
    }
    let notify_target = notify_target_for_file_if_needed(&file);
    drop(inner);
    notify_access_permission(notify_target.as_ref())?;

    let token = current_user_token();
    let mut total_read = 0;
    let iovs = read_iovec(token, iov_ptr, iovcnt)?;
    total_iov_len(&iovs)?;

    for iov in iovs {
        if iov.len == 0 {
            continue;
        }
        let buffers = match translated_byte_buffer_for_write(token, iov.base as *mut u8, iov.len) {
            Ok(buffers) => buffers,
            Err(_) if total_read != 0 => break,
            Err(err) => return Err(err),
        };
        let user_buffer = UserBuffer::new(buffers);
        let read = match file.read(user_buffer) {
            Ok(read) => read,
            Err(_) if total_read != 0 => break,
            Err(err) => return Err(err),
        };
        total_read += read;
        if read < iov.len {
            break;
        }
    }
    if total_read > 0 {
        if let Some(target) = notify_target.as_ref() {
            notify_access(target);
        }
    }
    Ok(total_read)
}
