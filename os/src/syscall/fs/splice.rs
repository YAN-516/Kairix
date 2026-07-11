use super::io::check_write_size_limit;
use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::notify::fanotify::{FAN_ACCESS_PERM, fanotify_check_permission_dentry};
use crate::fs::notify::{notify_access, notify_modify, notify_target_for_file_if_needed};
use crate::fs::tmpfs::inode::F_SEAL_WRITE;
use crate::fs::vfs::OpenFlags;
use crate::fs::vfs::file::File;
use crate::fs::vfs::inode::InodeMode;
use crate::mm::{UserBuffer, translated_ref, translated_refmut};
use crate::task::{current_process, current_user_token};
use alloc::sync::Arc;
use alloc::vec;
use log::info;
use polyhal::consts::PAGE_SIZE;
use polyhal::timer::current_time;

/// * out_fd: 目标 fd（通常是 socket）
/// * in_fd: 源 fd（通常是磁盘文件）
/// * offset_ptr: 用户空间的 offset 指针（可空）
/// * count: 要传输的字节数
pub fn sys_sendfile(out_fd: usize, in_fd: usize, offset_ptr: usize, count: usize) -> SyscallResult {
    info!(
        "[DEBUG] sys_sendfile: out_fd={}, in_fd={}, offset_ptr={}, count={}",
        out_fd, in_fd, offset_ptr, count
    );

    let token = current_user_token();
    let process = current_process();
    let inner = process.inner_exclusive_access();

    let (in_file, out_file) = match (inner.fd_table.get(in_fd), inner.fd_table.get(out_fd)) {
        (Some(Some(in_f)), Some(Some(out_f))) => (in_f.clone(), out_f.clone()),
        _ => return Err(SysError::EBADF),
    };
    drop(inner);
    if !in_file.readable() || !out_file.writable() {
        return Err(SysError::EINVAL);
    }
    if in_file.get_inode().is_none() {
        return Err(SysError::EINVAL);
    }
    let file_size = in_file.get_inode().map(|i| i.get_size()).unwrap_or(0);
    let (mut offset, update_fd) = if offset_ptr != 0 {
        (
            *translated_ref(token, offset_ptr as *const isize)? as usize,
            false,
        )
    } else {
        (in_file.get_offset(), true)
    };
    let end = (offset + count).min(file_size);
    let mut total = 0;
    while offset < end {
        let page_id = offset / PAGE_SIZE;
        let page_off = offset % PAGE_SIZE;
        let chunk = (end - offset).min(PAGE_SIZE - page_off);
        let Some(frame) = in_file.get_cache_frame(page_id) else {
            return Err(SysError::EINVAL);
        };
        let bytes = frame.ppn.get_bytes_array();
        let slice = &mut bytes[page_off..page_off + chunk];
        let written = out_file.write(UserBuffer::new(vec![slice]))?;
        if written == 0 {
            break;
        }
        total += written;
        offset += written;
        if written < chunk {
            break;
        }
    }
    if offset_ptr != 0 {
        *translated_refmut(token, offset_ptr as *mut isize)? = offset as isize;
    } else if update_fd {
        in_file.set_offset(offset);
    }
    info!("[DEBUG] sendfile transferred {} bytes", total);
    Ok(total)
}

pub fn sys_splice(
    fd_in: usize,
    off_in: usize,
    fd_out: usize,
    off_out: usize,
    len: usize,
    flags: u32,
) -> SyscallResult {
    const SPLICE_F_MOVE: u32 = 0x01;
    const SPLICE_F_NONBLOCK: u32 = 0x02;
    const SPLICE_F_MORE: u32 = 0x04;
    const SPLICE_F_GIFT: u32 = 0x08;
    const VALID_SPLICE_FLAGS: u32 =
        SPLICE_F_MOVE | SPLICE_F_NONBLOCK | SPLICE_F_MORE | SPLICE_F_GIFT;
    const SPLICE_CHUNK_SIZE: usize = PAGE_SIZE;

    if flags & !VALID_SPLICE_FLAGS != 0 {
        return Err(SysError::EINVAL);
    }
    if len == 0 {
        return Ok(0);
    }

    let token = current_user_token();
    let process = current_process();
    let inner = process.inner_exclusive_access();
    let (in_file, out_file) = match (inner.fd_table.get(fd_in), inner.fd_table.get(fd_out)) {
        (Some(Some(in_f)), Some(Some(out_f))) => (in_f.clone(), out_f.clone()),
        _ => return Err(SysError::EBADF),
    };
    drop(inner);

    if !in_file.readable() || !out_file.writable() {
        return Err(SysError::EBADF);
    }
    if !in_file.is_pipe() && !out_file.is_pipe() {
        return Err(SysError::EINVAL);
    }
    if in_file.is_pipe() && off_in != 0 {
        return Err(SysError::ESPIPE);
    }
    if out_file.is_pipe() && off_out != 0 {
        return Err(SysError::ESPIPE);
    }
    // This assignment requires an explicit offset for the regular-file side.
    // Unlike a pipe offset, the pointed-to value is advanced while the file
    // description's own offset remains unchanged.
    if !in_file.is_pipe() && off_in == 0 {
        return Err(SysError::EINVAL);
    }
    if !out_file.is_pipe() && off_out == 0 {
        return Err(SysError::EINVAL);
    }
    if out_file.is_append() {
        return Err(SysError::EINVAL);
    }
    if !in_file.is_pipe() {
        splice_check_nonpipe_input(&in_file)?;
    }
    if !out_file.is_pipe() {
        splice_check_nonpipe_output(&out_file)?;
    }
    if in_file.is_pipe() && out_file.is_pipe() {
        let in_pipe = in_file.pipe_buffer().ok_or(SysError::EINVAL)?;
        let out_pipe = out_file.pipe_buffer().ok_or(SysError::EINVAL)?;
        if in_pipe.id() == out_pipe.id() {
            return Err(SysError::EINVAL);
        }
    }
    if !in_file.is_pipe() && off_in != 0 && in_file.get_inode().is_none() {
        return Err(SysError::ESPIPE);
    }
    if !out_file.is_pipe() && off_out != 0 && out_file.get_inode().is_none() {
        return Err(SysError::ESPIPE);
    }

    let saved_in_offset = in_file.get_offset();
    let saved_out_offset = out_file.get_offset();

    let current_in_off = if off_in != 0 {
        let off = *translated_ref(token, off_in as *const i64)?;
        if off < 0 {
            return Err(SysError::EINVAL);
        }
        off as usize
    } else {
        saved_in_offset
    };

    let current_out_off = if off_out != 0 {
        let off = *translated_ref(token, off_out as *const i64)?;
        if off < 0 {
            return Err(SysError::EINVAL);
        }
        off as usize
    } else {
        saved_out_offset
    };

    // EOF must be reported before waiting for space in the output pipe. In
    // particular, an input offset at or beyond the file size returns zero
    // immediately even when that pipe is currently full.
    if !in_file.is_pipe() {
        let input_inode = in_file.get_inode().ok_or(SysError::EINVAL)?;
        if input_inode.get_mode().get_type() == InodeMode::FILE {
            let input_size = input_inode.get_size();
            if current_in_off >= input_size {
                return Ok(0);
            }
        }
    }

    if current_in_off.checked_add(len).is_none() || current_out_off.checked_add(len).is_none() {
        return Err(SysError::EOVERFLOW);
    }
    if !out_file.is_pipe() && out_file.get_inode().is_some() {
        check_write_size_limit(current_out_off, len)?;
    }
    if let Some(inode) = out_file.get_inode() {
        if (inode.get_seals() & F_SEAL_WRITE) != 0 {
            return Err(SysError::EPERM);
        }
    }

    let splice_nonblock = flags & SPLICE_F_NONBLOCK != 0;
    let in_nonblock = splice_nonblock || in_file.status_flags() & OpenFlags::O_NONBLOCK.bits() != 0;
    let out_nonblock =
        splice_nonblock || out_file.status_flags() & OpenFlags::O_NONBLOCK.bits() != 0;

    let mut total_spliced = 0usize;
    let mut buffer = [0u8; SPLICE_CHUNK_SIZE];

    if let (Some(in_pipe), Some(out_pipe)) = (in_file.pipe_buffer(), out_file.pipe_buffer()) {
        while total_spliced < len {
            let readable = match in_pipe.wait_readable(in_nonblock) {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) if total_spliced > 0 => break,
                Err(err) => return Err(err),
            };
            let writable = match out_pipe.wait_writable(out_nonblock) {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) if total_spliced > 0 => break,
                Err(err) => return Err(err),
            };
            let chunk = (len - total_spliced)
                .min(readable)
                .min(writable)
                .min(SPLICE_CHUNK_SIZE);
            if chunk == 0 {
                break;
            }
            let moved = match in_pipe.transfer_to(&*out_pipe, chunk) {
                Ok(n) => n,
                Err(_) if total_spliced > 0 => break,
                Err(err) => return Err(err),
            };
            if moved == 0 {
                continue;
            }
            total_spliced += moved;
            if moved < chunk {
                break;
            }
        }
    } else if let Some(out_pipe) = out_file.pipe_buffer() {
        if let Some(target) = in_file.get_inode().map(|_| in_file.get_dentry()) {
            fanotify_check_permission_dentry(target, FAN_ACCESS_PERM)?;
        }
        while total_spliced < len {
            let writable = match out_pipe.wait_writable(out_nonblock) {
                Ok(n) => n,
                Err(_) if total_spliced > 0 => break,
                Err(err) => return Err(err),
            };
            let chunk = (len - total_spliced).min(writable).min(SPLICE_CHUNK_SIZE);
            if chunk == 0 {
                break;
            }
            let read_off = current_in_off + total_spliced;
            let read_len =
                match splice_read_nonpipe(&in_file, off_in != 0, read_off, &mut buffer[..chunk]) {
                    Ok(n) => n,
                    Err(_) if total_spliced > 0 => break,
                    Err(err) => return Err(err),
                };
            if read_len == 0 {
                break;
            }
            let written = match out_pipe.write_slice(&buffer[..read_len]) {
                Ok(n) => n,
                Err(_) if total_spliced > 0 => break,
                Err(err) => return Err(err),
            };
            total_spliced += written;
            if written < read_len {
                break;
            }
        }
    } else if let Some(in_pipe) = in_file.pipe_buffer() {
        // A pipe input blocks only until some data becomes available. Once it
        // does, splice consumes at most that currently readable snapshot and
        // may return less than len; it must not wait for future writes merely
        // to fill the caller's requested length.
        let readable = match in_pipe.wait_readable(in_nonblock) {
            Ok(0) => 0,
            Ok(n) => n,
            Err(err) => return Err(err),
        };
        let splice_limit = len.min(readable);
        while total_spliced < splice_limit {
            let chunk = (splice_limit - total_spliced).min(SPLICE_CHUNK_SIZE);
            if chunk == 0 {
                break;
            }
            let peeked = in_pipe.peek_slice(&mut buffer[..chunk]);
            if peeked == 0 {
                break;
            }
            let write_off = current_out_off + total_spliced;
            let written =
                match splice_write_nonpipe(&out_file, off_out != 0, write_off, &buffer[..peeked]) {
                    Ok(n) => n,
                    Err(_) if total_spliced > 0 => break,
                    Err(err) => return Err(err),
                };
            if written == 0 {
                break;
            }
            let discarded = in_pipe.discard_slice(written);
            total_spliced += discarded;
            if discarded < peeked || written < peeked {
                break;
            }
        }
    }

    if off_in != 0 {
        *translated_refmut(token, off_in as *mut i64)? = (current_in_off + total_spliced) as i64;
    } else if !in_file.is_pipe() {
        in_file.set_offset(current_in_off + total_spliced);
    }
    if off_out != 0 {
        *translated_refmut(token, off_out as *mut i64)? = (current_out_off + total_spliced) as i64;
    } else if !out_file.is_pipe() {
        out_file.set_offset(current_out_off + total_spliced);
    }

    if total_spliced > 0 {
        if let Some(target) = notify_target_for_file_if_needed(&in_file) {
            notify_access(&target);
        }
        if let Some(target) = notify_target_for_file_if_needed(&out_file) {
            notify_modify(&target);
        }
    }

    Ok(total_spliced)
}

fn splice_inode_type(file: &Arc<dyn File + Send + Sync>) -> SysResult<InodeMode> {
    if file.is_path_only() {
        return Err(SysError::EBADF);
    }
    let inode = file.get_inode().ok_or(SysError::EINVAL)?;
    Ok(inode.get_mode().get_type())
}

fn splice_check_nonpipe_input(file: &Arc<dyn File + Send + Sync>) -> SyscallResult {
    match splice_inode_type(file)? {
        InodeMode::FILE | InodeMode::CHAR => Ok(0),
        _ => Err(SysError::EINVAL),
    }
}

fn splice_check_nonpipe_output(file: &Arc<dyn File + Send + Sync>) -> SyscallResult {
    match splice_inode_type(file)? {
        InodeMode::FILE => Ok(0),
        _ => Err(SysError::EINVAL),
    }
}

fn splice_read_nonpipe(
    file: &Arc<dyn File + Send + Sync>,
    explicit_offset: bool,
    offset: usize,
    buf: &mut [u8],
) -> SysResult<usize> {
    if explicit_offset || file.get_inode().is_some() {
        file.read_at_direct(offset, buf)
    } else {
        let slice = unsafe { core::slice::from_raw_parts_mut(buf.as_mut_ptr(), buf.len()) };
        file.read(UserBuffer::new(vec![slice]))
    }
}

fn splice_write_nonpipe(
    file: &Arc<dyn File + Send + Sync>,
    explicit_offset: bool,
    offset: usize,
    buf: &[u8],
) -> SysResult<usize> {
    if explicit_offset || file.get_inode().is_some() {
        file.write_at_direct(offset, buf)
    } else {
        let mut data = buf.to_vec();
        let slice = unsafe { core::slice::from_raw_parts_mut(data.as_mut_ptr(), data.len()) };
        file.write(UserBuffer::new(vec![slice]))
    }
}

pub fn sys_copy_file_range(
    fd_in: usize,
    off_in: usize,
    fd_out: usize,
    off_out: usize,
    len: usize,
    flags: usize,
) -> SyscallResult {
    let token = current_user_token();
    let process = current_process();
    let inner = process.inner_exclusive_access();

    let (in_file, out_file) = match (inner.fd_table.get(fd_in), inner.fd_table.get(fd_out)) {
        (Some(Some(in_f)), Some(Some(out_f))) => (in_f.clone(), out_f.clone()),
        _ => return Err(SysError::EBADF),
    };
    drop(inner);

    if flags != 0 {
        return Err(SysError::EINVAL);
    }

    // Check file types first (before permissions), matching Linux kernel order
    if in_file.is_pipe() || out_file.is_pipe() {
        return Err(SysError::EINVAL);
    }
    let file_type_ok = |file: &Arc<dyn File + Send + Sync>| -> SyscallResult {
        if let Some(inode) = file.get_inode() {
            let mode = inode.get_mode();
            let ftype = mode & InodeMode::TYPE_MASK;
            if ftype == InodeMode::DIR {
                return Err(SysError::EISDIR);
            }
            if ftype != InodeMode::FILE {
                return Err(SysError::EINVAL);
            }
        } else {
            return Err(SysError::EINVAL);
        }
        Ok(0)
    };
    file_type_ok(&in_file)?;
    file_type_ok(&out_file)?;

    if !in_file.readable() || !out_file.writable() {
        return Err(SysError::EBADF);
    }

    if out_file.is_append() {
        return Err(SysError::EBADF);
    }

    let saved_in_offset = in_file.get_offset();
    let saved_out_offset = out_file.get_offset();

    let current_in_off = if off_in != 0 {
        let off = *translated_ref(token, off_in as *const i64)?;
        if off < 0 {
            return Err(SysError::EINVAL);
        }
        off as usize
    } else {
        saved_in_offset
    };

    let current_out_off = if off_out != 0 {
        let off = *translated_ref(token, off_out as *const i64)?;
        if off < 0 {
            return Err(SysError::EINVAL);
        }
        off as usize
    } else {
        saved_out_offset
    };

    // Check for offset overflow
    if current_in_off.checked_add(len).is_none() || current_out_off.checked_add(len).is_none() {
        return Err(SysError::EOVERFLOW);
    }

    // Check file size limit for output
    check_write_size_limit(current_out_off, len)?;

    // Check overlapping range for the same file
    if len > 0 {
        if let (Some(in_inode), Some(out_inode)) = (in_file.get_inode(), out_file.get_inode()) {
            if in_inode.get_ino() == out_inode.get_ino() {
                let in_path = in_file.get_dentry().path();
                let out_path = out_file.get_dentry().path();
                if in_path == out_path
                    && current_in_off < current_out_off + len
                    && current_out_off < current_in_off + len
                {
                    return Err(SysError::EINVAL);
                }
            }
        }
    }

    let mut total_copied = 0usize;
    const BUF_SIZE: usize = 4096;
    let mut buffer = [0u8; BUF_SIZE];

    while total_copied < len {
        let chunk = (len - total_copied).min(BUF_SIZE);

        // Read from input file
        let read_off = current_in_off + total_copied;
        in_file.set_offset(read_off);
        let read_buf: &'static mut [u8] =
            unsafe { core::slice::from_raw_parts_mut(buffer.as_mut_ptr(), chunk) };
        let read_bytes = match in_file.read(UserBuffer::new(vec![read_buf])) {
            Ok(n) => n,
            Err(e) => {
                if off_in != 0 {
                    in_file.set_offset(saved_in_offset);
                }
                if off_out != 0 {
                    out_file.set_offset(saved_out_offset);
                }
                return Err(e);
            }
        };
        if read_bytes == 0 {
            break;
        }

        // Write to output file
        let write_off = current_out_off + total_copied;
        out_file.set_offset(write_off);
        let write_buf: &'static mut [u8] =
            unsafe { core::slice::from_raw_parts_mut(buffer.as_mut_ptr(), read_bytes) };
        let written = match out_file.write(UserBuffer::new(vec![write_buf])) {
            Ok(n) => n,
            Err(e) => {
                if off_in != 0 {
                    in_file.set_offset(saved_in_offset);
                }
                if off_out != 0 {
                    out_file.set_offset(saved_out_offset);
                }
                return Err(e);
            }
        };
        total_copied += written;
        if written < read_bytes {
            break;
        }
    }

    // Update offsets according to copy_file_range semantics
    if off_in != 0 {
        *translated_refmut(token, off_in as *mut i64)? = (current_in_off + total_copied) as i64;
        in_file.set_offset(saved_in_offset);
    } else {
        in_file.set_offset(current_in_off + total_copied);
    }

    if off_out != 0 {
        *translated_refmut(token, off_out as *mut i64)? = (current_out_off + total_copied) as i64;
        out_file.set_offset(saved_out_offset);
    } else {
        out_file.set_offset(current_out_off + total_copied);
    }

    if total_copied > 0 {
        out_file.flush();

        let now_us = current_time().as_micros() as i64;
        let now_sec = now_us / 1_000_000;
        let now_nsec = (now_us % 1_000_000) * 1000;
        if let Some(in_inode) = in_file.get_inode() {
            in_inode.set_atime(now_sec, now_nsec);
        }
        if let Some(out_inode) = out_file.get_inode() {
            out_inode.set_mtime(now_sec, now_nsec);
            out_inode.set_ctime(now_sec, now_nsec);
        }
        if let Some(target) = notify_target_for_file_if_needed(&in_file) {
            notify_access(&target);
        }
        if let Some(target) = notify_target_for_file_if_needed(&out_file) {
            notify_modify(&target);
        }
    }

    Ok(total_copied)
}
