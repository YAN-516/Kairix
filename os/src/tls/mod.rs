//! Kernel-side TLS service.
//!
//! The user ABI is intentionally small: userspace owns the TCP socket and asks
//! the kernel to bind a TLS session to that fd, then reads and writes plaintext
//! through a TLS handle.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::convert::TryFrom;
use core::fmt;
use core::hash::Hasher;

use lazy_static::lazy_static;
use rustls::client::danger::{
    HandshakeSignatureValid, PeerVerified, ServerIdentity, ServerVerifier,
    SignatureVerificationInput,
};
use rustls::crypto::SignatureScheme;
use rustls::pki_types::ServerName;
use rustls::std::io::{self, Read, Write};
use rustls::{ClientConfig, ClientConnection, Connection};
use rustls_ring::DEFAULT_PROVIDER;
use spin::Mutex;

use crate::error::{SysError, SyscallResult};
use crate::socket::tcp::{self, TcpSocket, TcpSocketState};
use crate::socket::{SOCKET_MANAGER, SocketInner};
use crate::task::{current_process, suspend_current_and_run_next};

const TLS_IO_TIMEOUT_US: usize = 10_000_000;

lazy_static! {
    static ref TLS_MANAGER: Mutex<TlsManager> = Mutex::new(TlsManager::new());
}

fn kairix_getrandom(dest: &mut [u8]) -> core::result::Result<(), getrandom::Error> {
    crate::fs::devfs::urandom::fill_random(dest);
    Ok(())
}

getrandom::register_custom_getrandom!(kairix_getrandom);

#[unsafe(no_mangle)]
extern "C" fn __bswapsi2(value: u32) -> u32 {
    value.swap_bytes()
}

struct TlsManager {
    next_id: usize,
    sessions: BTreeMap<usize, TlsSession>,
}

impl TlsManager {
    fn new() -> Self {
        Self {
            next_id: 1,
            sessions: BTreeMap::new(),
        }
    }

    fn insert(&mut self, session: TlsSession) -> usize {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.sessions.insert(id, session);
        id
    }
}

struct TlsSession {
    owner_pid: usize,
    fd: usize,
    tcp: Arc<Mutex<TcpSocket>>,
    conn: ClientConnection,
}

struct TcpIo {
    tcp: Arc<Mutex<TcpSocket>>,
}

impl TcpIo {
    fn new(tcp: Arc<Mutex<TcpSocket>>) -> Self {
        Self { tcp }
    }

    fn is_closed(&self) -> bool {
        matches!(self.tcp.lock().state, TcpSocketState::Closed)
    }
}

impl Read for TcpIo {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        crate::net::poll_rx_all();
        match self.tcp.lock().recv_from(buf) {
            Ok((n, _, _)) => Ok(n),
            Err(SysError::EAGAIN) => Err(io::ErrorKind::WouldBlock.into()),
            Err(SysError::ENOTCONN) => Ok(0),
            Err(_) => Err(io::ErrorKind::Other.into()),
        }
    }
}

impl Write for TcpIo {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        tcp::send_tracked(self.tcp.clone(), buf).map_err(|err| match err {
            SysError::EAGAIN => io::ErrorKind::WouldBlock.into(),
            SysError::ENOTCONN => io::ErrorKind::BrokenPipe.into(),
            _ => io::ErrorKind::Other.into(),
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Debug)]
struct InsecureServerVerifier;

impl ServerVerifier for InsecureServerVerifier {
    fn verify_identity(
        &self,
        _identity: &ServerIdentity<'_>,
    ) -> Result<PeerVerified, rustls::Error> {
        Ok(PeerVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _input: &SignatureVerificationInput<'_>,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _input: &SignatureVerificationInput<'_>,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::ED25519,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
        ]
    }

    fn request_ocsp_response(&self) -> bool {
        false
    }

    fn hash_config(&self, h: &mut dyn Hasher) {
        h.write_u8(0);
    }
}

fn map_tls_error<E: fmt::Debug>(err: E) -> SysError {
    log::warn!("kernel tls error: {:?}", err);
    SysError::EIO
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

fn config() -> Result<ClientConfig, SysError> {
    ClientConfig::builder(Arc::new(DEFAULT_PROVIDER.clone()))
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(InsecureServerVerifier))
        .with_no_client_auth()
        .map_err(map_tls_error)
}

fn drive_tls(conn: &mut ClientConnection, io: &mut TcpIo) -> Result<(), SysError> {
    while conn.wants_write() {
        match conn.write_tls(io) {
            Ok(0) => return Err(SysError::EIO),
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
            Err(err) => return Err(map_tls_error(err)),
        }
    }
    Ok(())
}

fn wait_for_tls_input(conn: &mut ClientConnection, io: &mut TcpIo) -> Result<(), SysError> {
    let deadline = crate::timer::get_time_us().saturating_add(TLS_IO_TIMEOUT_US);
    loop {
        crate::net::poll_rx_all();
        match conn.read_tls(io) {
            Ok(0) => return Err(SysError::ENOTCONN),
            Ok(_) => return Ok(()),
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                if io.is_closed() {
                    return Err(SysError::ENOTCONN);
                }
                if crate::timer::get_time_us() >= deadline {
                    return Err(SysError::ETIMEDOUT);
                }
                suspend_current_and_run_next();
            }
            Err(err) => return Err(map_tls_error(err)),
        }
    }
}

fn complete_handshake(conn: &mut ClientConnection, io: &mut TcpIo) -> Result<(), SysError> {
    let deadline = crate::timer::get_time_us().saturating_add(TLS_IO_TIMEOUT_US);
    while conn.is_handshaking() {
        drive_tls(conn, io)?;
        if !conn.is_handshaking() {
            break;
        }
        match conn.read_tls(io) {
            Ok(0) => return Err(SysError::ENOTCONN),
            Ok(_) => conn
                .process_new_packets()
                .map_err(map_tls_error)
                .map(|_| ())?,
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                if crate::timer::get_time_us() >= deadline {
                    return Err(SysError::ETIMEDOUT);
                }
                crate::net::poll_rx_all();
                suspend_current_and_run_next();
            }
            Err(err) => return Err(map_tls_error(err)),
        }
    }
    drive_tls(conn, io)
}

/// Create a TLS client session on an already-connected TCP socket.
pub fn connect(fd: usize, host: &str) -> SyscallResult {
    let tcp = tcp_from_fd(fd)?;
    if !matches!(
        tcp.lock().state,
        TcpSocketState::Established | TcpSocketState::CloseWait
    ) {
        return Err(SysError::ENOTCONN);
    }

    let server_name = ServerName::try_from(host)
        .map_err(|_| SysError::EINVAL)?
        .to_owned();
    let config = Arc::new(config()?);
    let mut conn = config.connect(server_name).build().map_err(map_tls_error)?;
    let mut io = TcpIo::new(tcp.clone());
    complete_handshake(&mut conn, &mut io)?;

    let owner_pid = current_process().getpid();
    let id = TLS_MANAGER.lock().insert(TlsSession {
        owner_pid,
        fd,
        tcp,
        conn,
    });
    Ok(id)
}

/// Write plaintext into a TLS session.
pub fn write(tls_id: usize, buf: &[u8]) -> SyscallResult {
    if buf.is_empty() {
        return Ok(0);
    }
    let pid = current_process().getpid();
    let mut manager = TLS_MANAGER.lock();
    let session = manager.sessions.get_mut(&tls_id).ok_or(SysError::EBADF)?;
    if session.owner_pid != pid {
        return Err(SysError::EBADF);
    }

    let mut io = TcpIo::new(session.tcp.clone());
    let n = session.conn.writer().write(buf).map_err(map_tls_error)?;
    drive_tls(&mut session.conn, &mut io)?;
    Ok(n)
}

/// Read plaintext from a TLS session.
pub fn read(tls_id: usize, buf: &mut [u8]) -> SyscallResult {
    if buf.is_empty() {
        return Ok(0);
    }
    let pid = current_process().getpid();
    let mut manager = TLS_MANAGER.lock();
    let session = manager.sessions.get_mut(&tls_id).ok_or(SysError::EBADF)?;
    if session.owner_pid != pid {
        return Err(SysError::EBADF);
    }

    let mut io = TcpIo::new(session.tcp.clone());
    loop {
        match session.conn.reader().read(buf) {
            Ok(n) if n > 0 => return Ok(n),
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
            Err(err) => return Err(map_tls_error(err)),
        }

        match wait_for_tls_input(&mut session.conn, &mut io) {
            Ok(()) => {}
            Err(SysError::ENOTCONN) => return Ok(0),
            Err(err) => return Err(err),
        }
        session.conn.process_new_packets().map_err(map_tls_error)?;
        drive_tls(&mut session.conn, &mut io)?;
    }
}

/// Close and remove a TLS session.
pub fn close(tls_id: usize) -> SyscallResult {
    let pid = current_process().getpid();
    let mut manager = TLS_MANAGER.lock();
    let mut session = manager.sessions.remove(&tls_id).ok_or(SysError::EBADF)?;
    if session.owner_pid != pid {
        manager.sessions.insert(tls_id, session);
        return Err(SysError::EBADF);
    }
    log::debug!("closing tls session {} for fd {}", tls_id, session.fd);
    session.conn.send_close_notify();
    let mut io = TcpIo::new(session.tcp.clone());
    let _ = drive_tls(&mut session.conn, &mut io);
    Ok(0)
}
