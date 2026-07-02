//! Kernel-side SSH transport service.
//!
//! This module provides a small SSH-oriented syscall backend on top of the
//! existing TCP stack. It performs the SSH identification string exchange and
//! keeps a per-process handle for subsequent SSH traffic. The full Sunset SSH
//! packet/authentication state machine can be layered here once its dependency
//! set is vendored for the kernel build.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;

use lazy_static::lazy_static;
use spin::Mutex;

use crate::error::{SysError, SyscallResult};
use crate::socket::tcp::{self, TcpSocket, TcpSocketState};
use crate::socket::{SOCKET_MANAGER, SocketInner};
use crate::task::{current_process, suspend_current_and_run_next};

const SSH_IO_TIMEOUT_US: usize = 10_000_000;
const SSH_IDENT_MAX: usize = 255;
const SSH_PRE_BANNER_MAX: usize = 4096;

lazy_static! {
    static ref SSH_MANAGER: Mutex<SshManager> = Mutex::new(SshManager::new());
}

struct SshManager {
    next_id: usize,
    sessions: BTreeMap<usize, SshSession>,
}

impl SshManager {
    fn new() -> Self {
        Self {
            next_id: 1,
            sessions: BTreeMap::new(),
        }
    }

    fn insert(&mut self, session: SshSession) -> usize {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.sessions.insert(id, session);
        id
    }
}

struct SshSession {
    owner_pid: usize,
    fd: usize,
    tcp: Arc<Mutex<TcpSocket>>,
    peer_ident: Vec<u8>,
}

fn tcp_from_fd(fd: usize) -> Result<Arc<Mutex<TcpSocket>>, SysError> {
    let pid = current_process().getpid();
    let manager = SOCKET_MANAGER.lock();
    let sock = manager.get_socket(fd, pid).ok_or(SysError::EBADF)?;
    match &sock.inner {
        SocketInner::Tcp(tcp) => Ok(tcp.clone()),
        _ => Err(SysError::ENOTSOCK),
    }
}

fn tcp_is_closed(tcp: &Arc<Mutex<TcpSocket>>) -> bool {
    matches!(tcp.lock().state, TcpSocketState::Closed)
}

fn check_client_ident(ident: &[u8]) -> Result<(), SysError> {
    if ident.is_empty() || ident.len() > SSH_IDENT_MAX - 2 {
        return Err(SysError::EINVAL);
    }
    if !ident.starts_with(b"SSH-") || ident.iter().any(|b| matches!(*b, b'\r' | b'\n' | 0)) {
        return Err(SysError::EINVAL);
    }
    Ok(())
}

fn send_client_ident(tcp: Arc<Mutex<TcpSocket>>, ident: &[u8]) -> Result<(), SysError> {
    check_client_ident(ident)?;

    let mut line = Vec::with_capacity(ident.len() + 2);
    line.extend_from_slice(ident);
    line.extend_from_slice(b"\r\n");

    let mut sent = 0usize;
    let deadline = crate::timer::get_time_us().saturating_add(SSH_IO_TIMEOUT_US);
    while sent < line.len() {
        match tcp::send_tracked(tcp.clone(), &line[sent..]) {
            Ok(0) => return Err(SysError::EIO),
            Ok(n) => sent += n,
            Err(SysError::EAGAIN) => {
                if crate::timer::get_time_us() >= deadline {
                    return Err(SysError::ETIMEDOUT);
                }
                suspend_current_and_run_next();
            }
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

fn recv_some(tcp: &Arc<Mutex<TcpSocket>>, buf: &mut [u8]) -> Result<usize, SysError> {
    crate::net::poll_rx_all();
    match tcp.lock().recv_from(buf) {
        Ok((n, _, _)) => Ok(n),
        Err(SysError::EAGAIN) => Err(SysError::EAGAIN),
        Err(SysError::ENOTCONN) => Ok(0),
        Err(err) => Err(err),
    }
}

fn read_peer_ident(tcp: Arc<Mutex<TcpSocket>>) -> Result<Vec<u8>, SysError> {
    let deadline = crate::timer::get_time_us().saturating_add(SSH_IO_TIMEOUT_US);
    let mut line = Vec::new();
    let mut consumed = 0usize;
    let mut one = [0u8; 1];

    loop {
        match recv_some(&tcp, &mut one) {
            Ok(0) => return Err(SysError::ENOTCONN),
            Ok(_) => {
                consumed += 1;
                if consumed > SSH_PRE_BANNER_MAX {
                    return Err(SysError::EINVAL);
                }
                if one[0] == b'\n' {
                    if line.ends_with(b"\r") {
                        line.pop();
                    }
                    if line.starts_with(b"SSH-") {
                        if line.len() > SSH_IDENT_MAX {
                            return Err(SysError::EINVAL);
                        }
                        return Ok(line);
                    }
                    line.clear();
                } else if line.len() >= SSH_IDENT_MAX {
                    return Err(SysError::EINVAL);
                } else {
                    line.push(one[0]);
                }
            }
            Err(SysError::EAGAIN) => {
                if tcp_is_closed(&tcp) {
                    return Err(SysError::ENOTCONN);
                }
                if crate::timer::get_time_us() >= deadline {
                    return Err(SysError::ETIMEDOUT);
                }
                suspend_current_and_run_next();
            }
            Err(err) => return Err(err),
        }
    }
}

/// Start an SSH transport session on an already-connected TCP socket.
pub fn connect(fd: usize, client_ident: &[u8]) -> SyscallResult {
    check_client_ident(client_ident)?;
    let tcp = tcp_from_fd(fd)?;
    if !matches!(
        tcp.lock().state,
        TcpSocketState::Established | TcpSocketState::CloseWait
    ) {
        return Err(SysError::ENOTCONN);
    }

    send_client_ident(tcp.clone(), client_ident)?;
    let peer_ident = read_peer_ident(tcp.clone())?;

    let owner_pid = current_process().getpid();
    let id = SSH_MANAGER.lock().insert(SshSession {
        owner_pid,
        fd,
        tcp,
        peer_ident,
    });
    Ok(id)
}

/// Return the peer SSH identification string.
pub fn peer_ident(ssh_id: usize, out: &mut [u8]) -> SyscallResult {
    let pid = current_process().getpid();
    let manager = SSH_MANAGER.lock();
    let session = manager.sessions.get(&ssh_id).ok_or(SysError::EBADF)?;
    if session.owner_pid != pid {
        return Err(SysError::EBADF);
    }
    if out.is_empty() {
        return Ok(session.peer_ident.len());
    }
    let n = out.len().min(session.peer_ident.len());
    out[..n].copy_from_slice(&session.peer_ident[..n]);
    Ok(n)
}

/// Write raw SSH transport bytes after the identification exchange.
pub fn write(ssh_id: usize, buf: &[u8]) -> SyscallResult {
    let pid = current_process().getpid();
    let tcp = {
        let manager = SSH_MANAGER.lock();
        let session = manager.sessions.get(&ssh_id).ok_or(SysError::EBADF)?;
        if session.owner_pid != pid {
            return Err(SysError::EBADF);
        }
        session.tcp.clone()
    };
    if buf.is_empty() {
        return Ok(0);
    }
    tcp::send_tracked(tcp, buf)
}

/// Read raw SSH transport bytes after the identification exchange.
pub fn read(ssh_id: usize, buf: &mut [u8]) -> SyscallResult {
    let pid = current_process().getpid();
    let tcp = {
        let manager = SSH_MANAGER.lock();
        let session = manager.sessions.get(&ssh_id).ok_or(SysError::EBADF)?;
        if session.owner_pid != pid {
            return Err(SysError::EBADF);
        }
        session.tcp.clone()
    };
    if buf.is_empty() {
        return Ok(0);
    }

    let deadline = crate::timer::get_time_us().saturating_add(SSH_IO_TIMEOUT_US);
    loop {
        match recv_some(&tcp, buf) {
            Ok(n) => return Ok(n),
            Err(SysError::EAGAIN) => {
                if tcp_is_closed(&tcp) {
                    return Ok(0);
                }
                if crate::timer::get_time_us() >= deadline {
                    return Err(SysError::ETIMEDOUT);
                }
                suspend_current_and_run_next();
            }
            Err(err) => return Err(err),
        }
    }
}

/// Close and remove an SSH transport session.
pub fn close(ssh_id: usize) -> SyscallResult {
    let pid = current_process().getpid();
    let mut manager = SSH_MANAGER.lock();
    let session = manager.sessions.remove(&ssh_id).ok_or(SysError::EBADF)?;
    if session.owner_pid != pid {
        manager.sessions.insert(ssh_id, session);
        return Err(SysError::EBADF);
    }
    log::debug!("closing ssh session {} for fd {}", ssh_id, session.fd);
    Ok(0)
}
