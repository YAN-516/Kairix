use super::Timespec;
use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::vfs::file::File;
use crate::mm::{translated_byte_buffer, translated_ref};
use crate::socket::SOCKET_MANAGER;
use crate::task::{
    ProcessControlBlock, block_current_and_run_next, current_process, current_task,
    current_user_token, suspend_current_and_run_next,
};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use polyhal::timer::current_time;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PollFd {
    pub fd: i32,
    pub events: i16,
    pub revents: i16,
}

const POLL_MAXFDS: usize = 1024;
static ZERO_FD_PSELECT_COUNT: AtomicUsize = AtomicUsize::new(0);
static ZERO_FD_PSELECT_DUMP_COUNT: AtomicUsize = AtomicUsize::new(0);

fn read_poll_user_bytes(token: usize, ptr: *const u8, len: usize) -> SysResult<Vec<u8>> {
    let mut out = Vec::with_capacity(len);
    if len == 0 {
        return Ok(out);
    }
    let parts = translated_byte_buffer(token, ptr, len)?;
    for part in parts {
        out.extend_from_slice(part);
    }
    Ok(out)
}

fn write_poll_user_bytes(token: usize, ptr: *mut u8, src: &[u8]) -> SysResult<()> {
    if src.is_empty() {
        return Ok(());
    }
    let mut copied = 0usize;
    let parts = translated_byte_buffer(token, ptr as *const u8, src.len())?;
    for part in parts {
        let n = part.len();
        part.copy_from_slice(&src[copied..copied + n]);
        copied += n;
    }
    Ok(())
}

fn read_pollfds(token: usize, ufds: usize, nfds: usize) -> SysResult<Vec<PollFd>> {
    if nfds > POLL_MAXFDS {
        return Err(SysError::EINVAL);
    }
    if nfds == 0 {
        return Ok(Vec::new());
    }
    if ufds == 0 {
        return Err(SysError::EFAULT);
    }

    let pollfd_size = core::mem::size_of::<PollFd>();
    let bytes_len = nfds.checked_mul(pollfd_size).ok_or(SysError::EINVAL)?;
    let raw = read_poll_user_bytes(token, ufds as *const u8, bytes_len)?;
    let mut fds = Vec::with_capacity(nfds);
    for chunk in raw.chunks_exact(pollfd_size) {
        let fd = i32::from_ne_bytes(chunk[0..4].try_into().map_err(|_| SysError::EFAULT)?);
        let events = i16::from_ne_bytes(chunk[4..6].try_into().map_err(|_| SysError::EFAULT)?);
        let revents = i16::from_ne_bytes(chunk[6..8].try_into().map_err(|_| SysError::EFAULT)?);
        fds.push(PollFd {
            fd,
            events,
            revents,
        });
    }
    Ok(fds)
}

fn write_pollfds(token: usize, ufds: usize, fds: &[PollFd]) -> SysResult<()> {
    if fds.is_empty() {
        return Ok(());
    }
    let mut raw = Vec::with_capacity(fds.len() * core::mem::size_of::<PollFd>());
    for pollfd in fds {
        raw.extend_from_slice(&pollfd.fd.to_ne_bytes());
        raw.extend_from_slice(&pollfd.events.to_ne_bytes());
        raw.extend_from_slice(&pollfd.revents.to_ne_bytes());
    }
    write_poll_user_bytes(token, ufds as *mut u8, &raw)
}

pub fn sys_ppoll(ufds: usize, nfds: usize, tmo_p: usize, _sigmask: usize) -> SyscallResult {
    const POLLIN: i16 = 0x001;
    const POLLOUT: i16 = 0x004;
    const POLLERR: i16 = 0x008;
    const POLLHUP: i16 = 0x010;

    let token = current_user_token();
    let process = current_process();

    let deadline = if tmo_p != 0 {
        let tmo = *translated_ref(token, tmo_p as *const Timespec)?;
        if tmo.tv_sec < 0 || tmo.tv_nsec < 0 {
            return Err(SysError::EINVAL);
        }
        let timeout_us = tmo.tv_sec as i128 * 1_000_000 + tmo.tv_nsec as i128 / 1_000;
        if timeout_us > 0 {
            Some(current_time().as_micros() as i128 + timeout_us)
        } else {
            Some(current_time().as_micros() as i128)
        }
    } else {
        None
    };

    let mut ready_count;
    let mut pollfds = read_pollfds(token, ufds, nfds)?;

    loop {
        ready_count = 0;
        for pollfd in pollfds.iter_mut() {
            pollfd.revents = 0;
            let fd = pollfd.fd;
            if fd < 0 {
                continue;
            }
            let fd = fd as usize;

            let (readable, writable, _exceptional) = check_fd_ready(&process, fd);
            let events = pollfd.events;
            let mut revents = 0;

            if (events & POLLIN) != 0 && readable {
                revents |= POLLIN;
            }
            if (events & POLLOUT) != 0 && writable {
                revents |= POLLOUT;
            }
            let inner = process.inner_exclusive_access();
            let file = if fd < inner.fd_table.len() {
                inner.fd_table[fd].clone()
            } else {
                None
            };
            drop(inner);
            if let Some(file) = file {
                if file.is_pipe() {
                    if file.readable() && file.pipe_all_write_ends_closed() {
                        revents |= POLLHUP;
                    }
                    if file.writable() && file.pipe_all_read_ends_closed() {
                        revents |= POLLERR;
                    }
                }
            }

            pollfd.revents = revents;
            if revents != 0 {
                ready_count += 1;
            }
        }

        if ready_count > 0 {
            break;
        }

        if let Some(d) = deadline {
            if (current_time().as_micros() as i128) >= d {
                break;
            }
        }

        let task_handle = current_task().unwrap();
        let mut requires_active_poll = false;
        for pollfd in pollfds.iter() {
            if pollfd.fd < 0 {
                continue;
            }
            let fd = pollfd.fd as usize;
            let inner = process.inner_exclusive_access();
            if fd < inner.fd_table.len() {
                if let Some(file) = &inner.fd_table[fd] {
                    requires_active_poll |= file.requires_active_poll();
                    file.register_poll_waker(task_handle.clone());
                }
            }
            drop(inner);
        }

        if deadline.is_some() || requires_active_poll {
            suspend_current_and_run_next();
        } else {
            block_current_and_run_next();
        }

        let task_handle = current_task().unwrap();
        for pollfd in pollfds.iter() {
            if pollfd.fd < 0 {
                continue;
            }
            let fd = pollfd.fd as usize;
            let inner = process.inner_exclusive_access();
            if fd < inner.fd_table.len() {
                if let Some(file) = &inner.fd_table[fd] {
                    file.clear_poll_waker(&task_handle);
                }
            }
            drop(inner);
        }
        if process.inner_exclusive_access().is_zombie
            || crate::syscall::signal::should_interrupt_syscall()
        {
            return Err(SysError::EINTR);
        }
    }

    write_pollfds(token, ufds, &pollfds)?;
    Ok(ready_count)
}

const FD_SETSIZE: usize = 1024;

fn fd_set_words(nfds: usize) -> usize {
    (nfds + 63) / 64
}

fn fd_is_set(fds: &[u64], fd: usize) -> bool {
    if fd >= FD_SETSIZE {
        return false;
    }
    (fds[fd / 64] >> (fd % 64)) & 1 != 0
}

fn fd_set_bit(fds: &mut [u64], fd: usize) {
    if fd < FD_SETSIZE {
        fds[fd / 64] |= 1 << (fd % 64);
    }
}

fn copy_fd_set_from_user(
    token: usize,
    fds_ptr: *mut u64,
    words: usize,
    buf: &mut [u64],
) -> SysResult<()> {
    if fds_ptr.is_null() || words == 0 {
        return Ok(());
    }
    let bytes = words * core::mem::size_of::<u64>();
    let user_bufs = translated_byte_buffer(token, fds_ptr as *const u8, bytes)?;
    let mut offset = 0;
    for user_buf in user_bufs {
        for (i, byte) in user_buf.iter().enumerate() {
            let idx = offset + i;
            if idx >= bytes {
                return Ok(());
            }
            let word_idx = idx / 8;
            let byte_idx = idx % 8;
            buf[word_idx] |= (*byte as u64) << (byte_idx * 8);
        }
        offset += user_buf.len();
    }
    Ok(())
}

fn copy_fd_set_to_user(
    token: usize,
    fds_ptr: *mut u64,
    words: usize,
    buf: &[u64],
) -> SysResult<()> {
    if fds_ptr.is_null() || words == 0 {
        return Ok(());
    }
    let bytes = words * core::mem::size_of::<u64>();
    let user_bufs = translated_byte_buffer(token, fds_ptr as *const u8, bytes)?;
    let mut offset = 0;
    for user_buf in user_bufs {
        for (i, user_byte) in user_buf.iter_mut().enumerate() {
            let idx = offset + i;
            if idx >= bytes {
                return Ok(());
            }
            let word_idx = idx / 8;
            let byte_idx = idx % 8;
            *user_byte = (buf[word_idx] >> (byte_idx * 8)) as u8;
        }
        offset += user_buf.len();
    }
    Ok(())
}

fn check_fd_ready(process: &ProcessControlBlock, fd: usize) -> (bool, bool, bool) {
    let inner = process.inner_exclusive_access();
    let file = if fd < inner.fd_table.len() {
        inner.fd_table[fd].clone()
    } else {
        None
    };
    drop(inner);

    file.as_ref()
        .map(|file| check_file_ready(process, fd, file))
        .unwrap_or((false, false, false))
}

fn check_file_ready(
    process: &ProcessControlBlock,
    fd: usize,
    file: &Arc<dyn File + Send + Sync>,
) -> (bool, bool, bool) {
    let mut readable = false;
    let mut writable = false;
    if let Some(is_read_ready) = file.read_ready() {
        readable = file.readable() && is_read_ready;
        writable = file
            .write_ready()
            .map(|is_write_ready| file.writable() && is_write_ready)
            .unwrap_or_else(|| file.writable());
    } else if file.is_socket() {
        let pid = process.getpid();
        let manager = SOCKET_MANAGER.lock();
        if let Some(sock) = manager.get_socket(fd, pid) {
            match &sock.inner {
                crate::socket::SocketInner::Tcp(tcp) => {
                    let tcp_guard = tcp.lock();
                    readable = !tcp_guard.receive_queue.lock().is_empty()
                        || matches!(
                            tcp_guard.state,
                            crate::socket::tcp::TcpSocketState::CloseWait
                                | crate::socket::tcp::TcpSocketState::LastAck
                                | crate::socket::tcp::TcpSocketState::Closed
                                | crate::socket::tcp::TcpSocketState::FinWait1
                                | crate::socket::tcp::TcpSocketState::FinWait2
                        )
                        || (matches!(
                            tcp_guard.state,
                            crate::socket::tcp::TcpSocketState::Listening
                        ) && !tcp_guard.accept_queue.lock().is_empty());
                    writable =
                        !matches!(tcp_guard.state, crate::socket::tcp::TcpSocketState::Closed);
                }
                crate::socket::SocketInner::Udp(udp) => {
                    let udp_guard = udp.lock();
                    readable = !udp_guard.receive_queue.lock().is_empty();
                    writable = true;
                }
                crate::socket::SocketInner::Raw(_) => {
                    readable = true;
                    writable = true;
                }
                crate::socket::SocketInner::Unix(_) => {
                    readable = false;
                    writable = true;
                }
            }
        }
    } else if file.is_pipe() {
        if file.readable() {
            readable = file.pipe_has_data() || file.pipe_all_write_ends_closed();
        }
        if file.writable() {
            writable = file.pipe_has_space() && !file.pipe_all_read_ends_closed();
        }
    } else {
        if file.readable() {
            readable = true;
        }
        if file.writable() {
            writable = true;
        }
    }
    (readable, writable, false)
}

fn snapshot_fd_table(
    process: &ProcessControlBlock,
    nfds: usize,
) -> Vec<Option<Arc<dyn File + Send + Sync>>> {
    let inner = process.inner_exclusive_access();
    let len = nfds.min(inner.fd_table.len());
    let mut files = Vec::with_capacity(nfds);
    for fd in 0..len {
        files.push(inner.fd_table[fd].clone());
    }
    files.resize_with(nfds, || None);
    files
}

pub fn sys_pselect6(
    nfds: usize,
    readfds: *mut u64,
    writefds: *mut u64,
    exceptfds: *mut u64,
    timeout: *mut Timespec,
    _sigmask: *mut u8,
) -> SyscallResult {
    if nfds > FD_SETSIZE {
        return Err(SysError::EINVAL);
    }
    if nfds == 0 {
        let sequence = ZERO_FD_PSELECT_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if sequence % 1_000 == 0 && ZERO_FD_PSELECT_DUMP_COUNT.fetch_add(1, Ordering::Relaxed) < 32
        {
            crate::task::processor::dump_pselect_stall_snapshot(sequence);
        }
        if sequence >= 100_000 && sequence % 100_000 == 0 {
            crate::task::processor::dump_pselect_long_stall_snapshot(sequence);
        }
    }

    let token = current_user_token();
    let process = current_process();
    let words = fd_set_words(nfds);

    let mut input_read = vec![0u64; words];
    let mut input_write = vec![0u64; words];
    let mut input_except = vec![0u64; words];
    copy_fd_set_from_user(token, readfds, words, &mut input_read)?;
    copy_fd_set_from_user(token, writefds, words, &mut input_write)?;
    copy_fd_set_from_user(token, exceptfds, words, &mut input_except)?;

    let mut output_read = vec![0u64; words];
    let mut output_write = vec![0u64; words];
    let mut output_except = vec![0u64; words];

    let mut ready_count;

    let deadline = if !timeout.is_null() {
        let ts = *translated_ref(token, timeout)?;
        if ts.tv_sec < 0 || ts.tv_nsec < 0 {
            return Err(SysError::EINVAL);
        }
        let timeout_us = ts.tv_sec as i128 * 1_000_000 + ts.tv_nsec as i128 / 1_000;
        if timeout_us > 0 {
            Some(current_time().as_micros() as i128 + timeout_us)
        } else {
            Some(current_time().as_micros() as i128)
        }
    } else {
        None
    };

    loop {
        ready_count = 0;
        let fd_snapshot = snapshot_fd_table(&process, nfds);
        for i in 0..words {
            output_read[i] = 0;
            output_write[i] = 0;
            output_except[i] = 0;
        }

        for fd in 0..nfds {
            let (readable, writable, _exceptional) = fd_snapshot
                .get(fd)
                .and_then(|file| file.as_ref())
                .map(|file| check_file_ready(&process, fd, file))
                .unwrap_or((false, false, false));
            if readfds != core::ptr::null_mut() && fd_is_set(&input_read, fd) && readable {
                fd_set_bit(&mut output_read, fd);
                ready_count += 1;
            }
            if writefds != core::ptr::null_mut() && fd_is_set(&input_write, fd) && writable {
                fd_set_bit(&mut output_write, fd);
                ready_count += 1;
            }
            if exceptfds != core::ptr::null_mut() && fd_is_set(&input_except, fd) {
                // 简化：不报告异常
            }
        }

        if ready_count > 0 {
            break;
        }

        if let Some(d) = deadline {
            if (current_time().as_micros() as i128) >= d {
                break;
            }
        }

        let task_handle = current_task().unwrap();
        let mut requires_active_poll = false;
        for fd in 0..nfds {
            let mut should_register = false;
            if readfds != core::ptr::null_mut() && fd_is_set(&input_read, fd) {
                should_register = true;
            }
            if writefds != core::ptr::null_mut() && fd_is_set(&input_write, fd) {
                should_register = true;
            }
            if exceptfds != core::ptr::null_mut() && fd_is_set(&input_except, fd) {
                should_register = true;
            }
            if should_register {
                let inner = process.inner_exclusive_access();
                if fd < inner.fd_table.len() {
                    if let Some(file) = &inner.fd_table[fd] {
                        requires_active_poll |= file.requires_active_poll();
                        file.register_poll_waker(task_handle.clone());
                    }
                }
                drop(inner);
            }
        }

        if deadline.is_some() || requires_active_poll {
            suspend_current_and_run_next();
        } else {
            block_current_and_run_next();
        }

        let task_handle = current_task().unwrap();
        for fd in 0..nfds {
            let mut should_clear = false;
            if readfds != core::ptr::null_mut() && fd_is_set(&input_read, fd) {
                should_clear = true;
            }
            if writefds != core::ptr::null_mut() && fd_is_set(&input_write, fd) {
                should_clear = true;
            }
            if exceptfds != core::ptr::null_mut() && fd_is_set(&input_except, fd) {
                should_clear = true;
            }
            if should_clear {
                let inner = process.inner_exclusive_access();
                if fd < inner.fd_table.len() {
                    if let Some(file) = &inner.fd_table[fd] {
                        file.clear_poll_waker(&task_handle);
                    }
                }
                drop(inner);
            }
        }
        if process.inner_exclusive_access().is_zombie
            || crate::syscall::signal::should_interrupt_syscall()
        {
            return Err(SysError::EINTR);
        }
    }

    copy_fd_set_to_user(token, readfds, words, &output_read)?;
    copy_fd_set_to_user(token, writefds, words, &output_write)?;
    copy_fd_set_to_user(token, exceptfds, words, &output_except)?;

    Ok(ready_count)
}
