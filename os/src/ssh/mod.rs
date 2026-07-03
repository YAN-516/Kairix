//! Kernel-side SSH transport service.
//!
//! This module provides a small SSH-oriented syscall backend on top of the
//! existing TCP stack. It drives Sunset's client transport state machine through
//! identification exchange and the first key exchange, then keeps a per-process
//! handle for subsequent SSH operations.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::mem;

use lazy_static::lazy_static;
use spin::Mutex;
use sunset::{CliEvent, Event, Runner};

use crate::error::{SysError, SyscallResult};
use crate::socket::tcp::{self, TcpSocket, TcpSocketState};
use crate::socket::{SOCKET_MANAGER, SocketInner};
use crate::task::{current_process, suspend_current_and_run_next};

const SSH_IO_TIMEOUT_US: usize = 10_000_000;
const SSH_IDENT_MAX: usize = 255;
const SSH_PRE_BANNER_MAX: usize = 4096;
const SSH_PACKET_BUF_SIZE: usize = 35_000;
const SSH_RX_CHUNK: usize = 2048;

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
    runner: Option<Runner<'static, sunset::Client>>,
    authenticated: bool,
    closed: bool,
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

fn recv_some(tcp: &Arc<Mutex<TcpSocket>>, buf: &mut [u8]) -> Result<usize, SysError> {
    crate::net::poll_rx_all();
    match tcp.lock().recv_from(buf) {
        Ok((n, _, _)) => Ok(n),
        Err(SysError::EAGAIN) => Err(SysError::EAGAIN),
        Err(SysError::ENOTCONN) => Ok(0),
        Err(err) => Err(err),
    }
}

struct IdentCapture {
    line: Vec<u8>,
    consumed: usize,
    peer_ident: Option<Vec<u8>>,
}

impl IdentCapture {
    fn new() -> Self {
        Self {
            line: Vec::new(),
            consumed: 0,
            peer_ident: None,
        }
    }

    fn consume(&mut self, buf: &[u8]) -> Result<(), SysError> {
        if self.peer_ident.is_some() {
            return Ok(());
        }

        for &b in buf {
            self.consumed += 1;
            if self.consumed > SSH_PRE_BANNER_MAX {
                return Err(SysError::EINVAL);
            }

            if b == b'\n' {
                if self.line.ends_with(b"\r") {
                    self.line.pop();
                }
                if self.line.starts_with(b"SSH-") {
                    if self.line.len() > SSH_IDENT_MAX {
                        return Err(SysError::EINVAL);
                    }
                    self.peer_ident = Some(core::mem::take(&mut self.line));
                    return Ok(());
                }
                self.line.clear();
            } else if self.line.len() >= SSH_IDENT_MAX {
                return Err(SysError::EINVAL);
            } else {
                self.line.push(b);
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<Vec<u8>, SysError> {
        self.peer_ident.ok_or(SysError::EIO)
    }
}

fn sunset_error(err: sunset::Error) -> SysError {
    log::debug!("sunset ssh error: {:?}", err);
    SysError::EIO
}

fn new_client_runner() -> Runner<'static, sunset::Client> {
    let mut inbuf = Vec::new();
    inbuf.resize(SSH_PACKET_BUF_SIZE, 0);
    let mut outbuf = Vec::new();
    outbuf.resize(SSH_PACKET_BUF_SIZE, 0);
    Runner::new_client(
        Box::leak(inbuf.into_boxed_slice()),
        Box::leak(outbuf.into_boxed_slice()),
    )
}

fn flush_sunset_output(
    runner: &mut Runner<'static, sunset::Client>,
    tcp: &Arc<Mutex<TcpSocket>>,
    deadline: usize,
) -> Result<(), SysError> {
    loop {
        let sent = {
            let out = runner.output_buf();
            if out.is_empty() {
                return Ok(());
            }
            match tcp::send_tracked(tcp.clone(), out) {
                Ok(0) => return Err(SysError::EIO),
                Ok(n) => {
                    log::info!("ssh sunset tx {} of {} bytes", n, out.len());
                    n
                }
                Err(SysError::EAGAIN) => {
                    if crate::timer::get_time_us() >= deadline {
                        return Err(SysError::ETIMEDOUT);
                    }
                    suspend_current_and_run_next();
                    0
                }
                Err(err) => return Err(err),
            }
        };

        if sent > 0 {
            runner.consume_output(sent);
        }
    }
}

fn feed_sunset_input(
    runner: &mut Runner<'static, sunset::Client>,
    buf: &[u8],
) -> Result<usize, SysError> {
    let mut total = 0usize;
    while total < buf.len() {
        let n = runner.input(&buf[total..]).map_err(sunset_error)?;
        if n == 0 {
            break;
        }
        total += n;
    }
    Ok(total)
}

fn feed_pending_sunset_input(
    runner: &mut Runner<'static, sunset::Client>,
    pending: &mut Vec<u8>,
) -> Result<bool, SysError> {
    if pending.is_empty() || !runner.is_input_ready() {
        return Ok(false);
    }

    let consumed = feed_sunset_input(runner, pending)?;
    log::info!(
        "ssh sunset input consumed {} of {} pending bytes",
        consumed,
        pending.len()
    );
    if consumed == 0 {
        return Ok(false);
    }
    pending.drain(..consumed);
    Ok(true)
}

fn drive_sunset_kex(
    runner: &mut Runner<'static, sunset::Client>,
    tcp: &Arc<Mutex<TcpSocket>>,
) -> Result<Vec<u8>, SysError> {
    let deadline = crate::timer::get_time_us().saturating_add(SSH_IO_TIMEOUT_US);
    let mut rxbuf = [0u8; SSH_RX_CHUNK];
    let mut pending_rx = Vec::new();
    let mut ident = IdentCapture::new();
    let mut saw_hostkey = false;

    loop {
        let mut progressed = false;
        let mut kex_complete = false;
        let mut check_kex_done = false;

        {
            let event = runner.progress().map_err(sunset_error)?;
            match event {
                Event::Cli(CliEvent::Hostkey(hostkey)) => {
                    log::info!("ssh kex event: hostkey");
                    hostkey.accept().map_err(sunset_error)?;
                    log::info!("ssh kex hostkey accepted");
                    saw_hostkey = true;
                    progressed = true;
                }
                Event::Cli(CliEvent::Banner(banner)) => {
                    if let Ok(text) = banner.banner() {
                        log::info!("ssh server banner: {}", text);
                    }
                    progressed = true;
                }
                Event::Cli(other) => {
                    log::info!("unexpected ssh client event during kex: {:?}", other);
                    return Err(SysError::EIO);
                }
                Event::Serv(_) => {
                    log::info!("unexpected ssh server event during client kex");
                    return Err(SysError::EIO);
                }
                Event::Progressed => {
                    log::info!("ssh kex event: progressed");
                    progressed = true;
                    if saw_hostkey {
                        check_kex_done = true;
                    }
                }
                Event::None => {}
            }
        }

        if check_kex_done && runner.is_initial_kex_done() {
            kex_complete = true;
        }

        flush_sunset_output(runner, tcp, deadline)?;
        if kex_complete {
            return ident.finish();
        }

        if feed_pending_sunset_input(runner, &mut pending_rx)? {
            progressed = true;
        }

        if pending_rx.is_empty() && runner.is_input_ready() {
            match recv_some(tcp, &mut rxbuf) {
                Ok(0) => return Err(SysError::ENOTCONN),
                Ok(n) => {
                    log::info!("ssh kex rx {} bytes", n);
                    ident.consume(&rxbuf[..n])?;
                    pending_rx.extend_from_slice(&rxbuf[..n]);
                    progressed = true;
                    let _ = feed_pending_sunset_input(runner, &mut pending_rx)?;
                }
                Err(SysError::EAGAIN) => {}
                Err(err) => return Err(err),
            }
        }

        if !progressed {
            if tcp_is_closed(tcp) {
                return Err(SysError::ENOTCONN);
            }
            if crate::timer::get_time_us() >= deadline {
                return Err(SysError::ETIMEDOUT);
            }
            suspend_current_and_run_next();
        }
    }
}

fn drive_sunset_password_auth(
    runner: &mut Runner<'static, sunset::Client>,
    tcp: &Arc<Mutex<TcpSocket>>,
    username: &str,
    password: &str,
) -> Result<(), SysError> {
    let deadline = crate::timer::get_time_us().saturating_add(SSH_IO_TIMEOUT_US);
    let mut rxbuf = [0u8; SSH_RX_CHUNK];
    let mut pending_rx = Vec::new();
    let mut password_sent = false;

    loop {
        let mut progressed = false;

        {
            let event = runner.progress().map_err(sunset_error)?;
            match event {
                Event::Cli(CliEvent::Username(request)) => {
                    request.username(username).map_err(sunset_error)?;
                    progressed = true;
                }
                Event::Cli(CliEvent::Pubkey(request)) => {
                    request.skip().map_err(sunset_error)?;
                    progressed = true;
                }
                Event::Cli(CliEvent::Password(request)) => {
                    if password_sent {
                        return Err(SysError::EACCES);
                    }
                    request.password(password).map_err(sunset_error)?;
                    password_sent = true;
                    progressed = true;
                }
                Event::Cli(CliEvent::Hostkey(hostkey)) => {
                    hostkey.accept().map_err(sunset_error)?;
                    progressed = true;
                }
                Event::Cli(CliEvent::Banner(banner)) => {
                    if let Ok(text) = banner.banner() {
                        log::info!("ssh auth banner: {}", text);
                    }
                    progressed = true;
                }
                Event::Cli(CliEvent::Authenticated) => return Ok(()),
                Event::Cli(other) => {
                    log::debug!("unexpected ssh client event during auth: {:?}", other);
                    return Err(SysError::EIO);
                }
                Event::Serv(_) => return Err(SysError::EIO),
                Event::Progressed => progressed = true,
                Event::None => {}
            }
        }

        flush_sunset_output(runner, tcp, deadline)?;

        if feed_pending_sunset_input(runner, &mut pending_rx)? {
            progressed = true;
        }

        if pending_rx.is_empty() && runner.is_input_ready() {
            match recv_some(tcp, &mut rxbuf) {
                Ok(0) => return Err(SysError::ENOTCONN),
                Ok(n) => {
                    pending_rx.extend_from_slice(&rxbuf[..n]);
                    progressed = true;
                    let _ = feed_pending_sunset_input(runner, &mut pending_rx)?;
                }
                Err(SysError::EAGAIN) => {}
                Err(err) => return Err(err),
            }
        }

        if !progressed {
            if tcp_is_closed(tcp) {
                return Err(SysError::ENOTCONN);
            }
            if crate::timer::get_time_us() >= deadline {
                return Err(SysError::ETIMEDOUT);
            }
            suspend_current_and_run_next();
        }
    }
}

/// Start an SSH transport session on an already-connected TCP socket.
///
/// The supplied client identification string is validated for API compatibility,
/// while Sunset emits its own protocol identification and drives the first KEX.
pub fn connect(fd: usize, client_ident: &[u8]) -> SyscallResult {
    check_client_ident(client_ident)?;
    let tcp = tcp_from_fd(fd)?;
    if !matches!(
        tcp.lock().state,
        TcpSocketState::Established | TcpSocketState::CloseWait
    ) {
        return Err(SysError::ENOTCONN);
    }

    let mut runner = new_client_runner();
    let peer_ident = drive_sunset_kex(&mut runner, &tcp)?;

    let owner_pid = current_process().getpid();
    let id = SSH_MANAGER.lock().insert(SshSession {
        owner_pid,
        fd,
        tcp,
        peer_ident,
        runner: Some(runner),
        authenticated: false,
        closed: false,
    });
    Ok(id)
}

/// Authenticate the SSH session with a username and password.
pub fn auth_password(ssh_id: usize, username: &str, password: &str) -> SyscallResult {
    if username.is_empty() {
        return Err(SysError::EINVAL);
    }

    let pid = current_process().getpid();
    let (mut runner, tcp, already_authenticated) = {
        let mut manager = SSH_MANAGER.lock();
        let session = manager.sessions.get_mut(&ssh_id).ok_or(SysError::EBADF)?;
        if session.closed || session.owner_pid != pid {
            return Err(SysError::EBADF);
        }
        (
            session.runner.take().ok_or(SysError::EIO)?,
            session.tcp.clone(),
            session.authenticated,
        )
    };

    let result = if already_authenticated {
        Ok(())
    } else {
        drive_sunset_password_auth(&mut runner, &tcp, username, password)
    };

    let mut manager = SSH_MANAGER.lock();
    if let Some(session) = manager.sessions.get_mut(&ssh_id) {
        if result.is_ok() {
            session.authenticated = true;
        }
        if session.closed {
            mem::forget(runner);
        } else {
            session.runner = Some(runner);
        }
    } else {
        mem::forget(runner);
    }

    result.map(|_| 0)
}

/// Return the peer SSH identification string.
pub fn peer_ident(ssh_id: usize, out: &mut [u8]) -> SyscallResult {
    let pid = current_process().getpid();
    let manager = SSH_MANAGER.lock();
    let session = manager.sessions.get(&ssh_id).ok_or(SysError::EBADF)?;
    if session.closed || session.owner_pid != pid {
        return Err(SysError::EBADF);
    }
    if out.is_empty() {
        return Ok(session.peer_ident.len());
    }
    let n = out.len().min(session.peer_ident.len());
    out[..n].copy_from_slice(&session.peer_ident[..n]);
    Ok(n)
}

/// Write raw SSH transport bytes.
///
/// Non-empty raw writes are not valid after Sunset owns the transport state.
/// Channel/authentication syscalls should be layered on top of the stored runner.
pub fn write(ssh_id: usize, buf: &[u8]) -> SyscallResult {
    let pid = current_process().getpid();
    let manager = SSH_MANAGER.lock();
    let session = manager.sessions.get(&ssh_id).ok_or(SysError::EBADF)?;
    if session.closed || session.owner_pid != pid {
        return Err(SysError::EBADF);
    }
    if buf.is_empty() {
        return Ok(0);
    }
    Err(SysError::ENOTCONN)
}

/// Read raw SSH transport bytes.
///
/// Non-empty raw reads are not valid after Sunset owns the transport state.
/// Channel/authentication syscalls should be layered on top of the stored runner.
pub fn read(ssh_id: usize, buf: &mut [u8]) -> SyscallResult {
    let pid = current_process().getpid();
    let manager = SSH_MANAGER.lock();
    let session = manager.sessions.get(&ssh_id).ok_or(SysError::EBADF)?;
    if session.closed || session.owner_pid != pid {
        return Err(SysError::EBADF);
    }
    if buf.is_empty() {
        return Ok(0);
    }
    Err(SysError::ENOTCONN)
}

/// Close an SSH transport session.
pub fn close(ssh_id: usize) -> SyscallResult {
    let pid = current_process().getpid();
    let mut manager = SSH_MANAGER.lock();
    let session = manager.sessions.get_mut(&ssh_id).ok_or(SysError::EBADF)?;
    if session.closed || session.owner_pid != pid {
        return Err(SysError::EBADF);
    }
    log::debug!("closing ssh session {} for fd {}", ssh_id, session.fd);
    session.closed = true;
    // Sunset owns leaked packet buffers here; dropping the runner can stall close.
    if let Some(runner) = session.runner.take() {
        mem::forget(runner);
    }
    Ok(0)
}
