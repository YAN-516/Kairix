#![allow(missing_docs)]

use crate::error::{SysError, SyscallResult};
use crate::fs::File;
use crate::fs::config::FD_CLOEXEC_FLAG;
use crate::fs::pipe::make_pipe;
use crate::fs::vfs::OpenFlags;
use crate::mm::translated_byte_buffer;
use crate::task::{current_process, current_user_token};
use crate::trap::_set_sum_bit;

pub fn sys_pipe(pipe: *mut i32, flags: u32) -> SyscallResult {
    let valid_flags = OpenFlags::O_CLOEXEC.bits() | OpenFlags::O_NONBLOCK.bits();
    if flags & !valid_flags != 0 {
        return Err(SysError::EINVAL);
    }

    _set_sum_bit();
    let token = current_user_token();
    let mut user_bufs =
        translated_byte_buffer(token, pipe as *const u8, 2 * core::mem::size_of::<i32>())?;

    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    let (pipe_read, pipe_write) = make_pipe();
    if flags & OpenFlags::O_NONBLOCK.bits() != 0 {
        pipe_read.set_status_flags(OpenFlags::O_NONBLOCK.bits());
        pipe_write.set_status_flags(OpenFlags::O_NONBLOCK.bits());
    }

    let read_fd = inner.alloc_fd()?;
    if flags & OpenFlags::O_CLOEXEC.bits() != 0 && read_fd < inner.fd_flags.len() {
        inner.fd_flags[read_fd] |= FD_CLOEXEC_FLAG;
    }
    inner.fd_table[read_fd] = Some(pipe_read);
    let write_fd = match inner.alloc_fd() {
        Ok(fd) => fd,
        Err(e) => {
            inner.fd_table[read_fd] = None;
            if read_fd < inner.fd_flags.len() {
                inner.fd_flags[read_fd] = 0;
            }
            return Err(e);
        }
    };
    if flags & OpenFlags::O_CLOEXEC.bits() != 0 && write_fd < inner.fd_flags.len() {
        inner.fd_flags[write_fd] |= FD_CLOEXEC_FLAG;
    }
    inner.fd_table[write_fd] = Some(pipe_write);
    drop(inner);

    let fds = [read_fd as i32, write_fd as i32];
    let bytes = unsafe {
        core::slice::from_raw_parts(fds.as_ptr() as *const u8, 2 * core::mem::size_of::<i32>())
    };
    let mut copied = 0usize;
    for buf in user_bufs.iter_mut() {
        let n = buf.len().min(bytes.len() - copied);
        buf[..n].copy_from_slice(&bytes[copied..copied + n]);
        copied += n;
        if copied == bytes.len() {
            break;
        }
    }
    Ok(0)
}
