//! Kernel-side SSH transport service.
//!
//! This module provides a small SSH-oriented syscall backend on top of the
//! existing TCP stack. It drives Sunset's client transport state machine through
//! identification exchange and the first key exchange, then keeps a per-process
//! handle for subsequent SSH operations.

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::mem;

use lazy_static::lazy_static;
use spin::Mutex;
use sunset::{ChanData, ChanHandle, CliEvent, Event, Runner, SignKey};

use crate::error::{SysError, SyscallResult};
use crate::socket::tcp::{self, TcpSocket, TcpSocketState};
use crate::socket::{SOCKET_MANAGER, SocketInner};
use crate::task::{current_process, suspend_current_and_run_next};

const SSH_IO_TIMEOUT_US: usize = 10_000_000;
const SSH_IDENT_MAX: usize = 255;
const SSH_PRE_BANNER_MAX: usize = 4096;
const SSH_PACKET_BUF_SIZE: usize = 35_000;
const SSH_RX_CHUNK: usize = 2048;
const SSH_SLOT_BITS: usize = usize::BITS as usize / 2;
const SSH_SLOT_MASK: usize = (1usize << SSH_SLOT_BITS) - 1;
const OPENSSH_KEY_MAGIC: &[u8] = b"openssh-key-v1\0";
const OPENSSH_BEGIN: &[u8] = b"-----BEGIN OPENSSH PRIVATE KEY-----";
const OPENSSH_END: &[u8] = b"-----END OPENSSH PRIVATE KEY-----";
const SSH_ED25519_NAME: &[u8] = b"ssh-ed25519";

lazy_static! {
    static ref SSH_MANAGER: Mutex<SshManager> = Mutex::new(SshManager::new());
}

struct SshManager {
    next_generation: usize,
    slots: Vec<SshSlot>,
}

struct SshSlot {
    generation: usize,
    session: Option<SshSession>,
}

impl SshManager {
    fn new() -> Self {
        Self {
            next_generation: 1,
            slots: Vec::new(),
        }
    }

    fn insert(&mut self, session: SshSession) -> usize {
        let generation = self.alloc_generation();
        if let Some(index) = self.slots.iter().position(|slot| slot.session.is_none()) {
            self.slots[index].generation = generation;
            self.slots[index].session = Some(session);
            return Self::encode_id(index, generation);
        }

        let index = self.slots.len();
        self.slots.push(SshSlot {
            generation,
            session: Some(session),
        });
        Self::encode_id(index, generation)
    }

    fn get(&self, id: usize) -> Option<&SshSession> {
        let (index, generation) = Self::decode_id(id)?;
        let slot = self.slots.get(index)?;
        if slot.generation != generation {
            return None;
        }
        slot.session.as_ref()
    }

    fn get_mut(&mut self, id: usize) -> Option<&mut SshSession> {
        let (index, generation) = Self::decode_id(id)?;
        let slot = self.slots.get_mut(index)?;
        if slot.generation != generation {
            return None;
        }
        slot.session.as_mut()
    }

    fn remove(&mut self, id: usize) -> Option<SshSession> {
        let (index, generation) = Self::decode_id(id)?;
        let slot = self.slots.get_mut(index)?;
        if slot.generation != generation {
            return None;
        }
        slot.session.take()
    }

    fn alloc_generation(&mut self) -> usize {
        let generation = self.next_generation.max(1);
        self.next_generation = self.next_generation.wrapping_add(1).max(1);
        generation
    }

    fn encode_id(index: usize, generation: usize) -> usize {
        (generation << SSH_SLOT_BITS) | (index + 1)
    }

    fn decode_id(id: usize) -> Option<(usize, usize)> {
        let slot_id = id & SSH_SLOT_MASK;
        let generation = id >> SSH_SLOT_BITS;
        if slot_id == 0 || generation == 0 {
            return None;
        }
        Some((slot_id - 1, generation))
    }
}

struct SshSession {
    owner_pid: usize,
    fd: usize,
    tcp: Option<Arc<Mutex<TcpSocket>>>,
    peer_ident: Vec<u8>,
    runner: Option<Runner<'static, sunset::Client>>,
    pending_rx: Vec<u8>,
    channels: Vec<SshChannel>,
    authenticated: bool,
    closed: bool,
}

struct SshChannel {
    handle: Option<ChanHandle>,
    exit_status: Option<i32>,
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

struct ByteReader<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> ByteReader<'a> {
    fn new(input: &'a [u8]) -> Self {
        Self { input, pos: 0 }
    }

    fn read_exact(&mut self, len: usize) -> Result<&'a [u8], SysError> {
        let end = self.pos.checked_add(len).ok_or(SysError::EINVAL)?;
        if end > self.input.len() {
            return Err(SysError::EINVAL);
        }
        let out = &self.input[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    fn read_u32(&mut self) -> Result<u32, SysError> {
        let b = self.read_exact(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_string(&mut self) -> Result<&'a [u8], SysError> {
        let len = self.read_u32()? as usize;
        self.read_exact(len)
    }
}

fn parse_openssh_ed25519_key(input: &[u8]) -> Result<SignKey, SysError> {
    let decoded = if input.starts_with(OPENSSH_KEY_MAGIC) {
        input.to_vec()
    } else {
        let body = collect_openssh_pem_body(input)?;
        base64_decode(&body)?
    };

    let mut r = ByteReader::new(&decoded);
    if r.read_exact(OPENSSH_KEY_MAGIC.len())? != OPENSSH_KEY_MAGIC {
        return Err(SysError::EINVAL);
    }

    let cipher = r.read_string()?;
    let kdf = r.read_string()?;
    let _kdf_options = r.read_string()?;
    if cipher != b"none" || kdf != b"none" {
        return Err(SysError::EACCES);
    }

    if r.read_u32()? != 1 {
        return Err(SysError::EINVAL);
    }
    let _public_key = r.read_string()?;
    let private_blob = r.read_string()?;

    let mut p = ByteReader::new(private_blob);
    let check1 = p.read_u32()?;
    let check2 = p.read_u32()?;
    if check1 != check2 {
        return Err(SysError::EINVAL);
    }

    if p.read_string()? != SSH_ED25519_NAME {
        return Err(SysError::EINVAL);
    }
    let public_key = p.read_string()?;
    let private_key = p.read_string()?;
    let _comment = p.read_string()?;
    if public_key.len() != 32 || private_key.len() != 64 {
        return Err(SysError::EINVAL);
    }

    let mut seed = [0u8; 32];
    seed.copy_from_slice(&private_key[..32]);
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
    if signing_key.verifying_key().to_bytes() != public_key {
        return Err(SysError::EINVAL);
    }

    Ok(SignKey::Ed25519(signing_key))
}

fn collect_openssh_pem_body(input: &[u8]) -> Result<Vec<u8>, SysError> {
    let mut out = Vec::new();
    let mut in_body = false;
    let mut saw_end = false;
    let mut start = 0usize;

    while start <= input.len() {
        let mut end = start;
        while end < input.len() && input[end] != b'\n' {
            end += 1;
        }
        let mut line = &input[start..end];
        while matches!(line.last(), Some(b'\r' | b' ' | b'\t')) {
            line = &line[..line.len() - 1];
        }
        while matches!(line.first(), Some(b' ' | b'\t')) {
            line = &line[1..];
        }

        if line == OPENSSH_BEGIN {
            in_body = true;
        } else if line == OPENSSH_END {
            saw_end = true;
            break;
        } else if in_body {
            for &b in line {
                if !b.is_ascii_whitespace() {
                    out.push(b);
                }
            }
        }

        if end == input.len() {
            break;
        }
        start = end + 1;
    }

    if !saw_end || out.is_empty() {
        return Err(SysError::EINVAL);
    }
    Ok(out)
}

fn base64_decode(input: &[u8]) -> Result<Vec<u8>, SysError> {
    if input.len() % 4 != 0 {
        return Err(SysError::EINVAL);
    }

    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < input.len() {
        let mut vals = [0u8; 4];
        let mut pad = 0usize;
        for i in 0..4 {
            let b = input[pos + i];
            if b == b'=' {
                vals[i] = 0;
                pad += 1;
            } else {
                if pad != 0 {
                    return Err(SysError::EINVAL);
                }
                vals[i] = base64_value(b).ok_or(SysError::EINVAL)?;
            }
        }
        if pad > 2 || (pad != 0 && pos + 4 != input.len()) {
            return Err(SysError::EINVAL);
        }

        out.push((vals[0] << 2) | (vals[1] >> 4));
        if pad < 2 {
            out.push((vals[1] << 4) | (vals[2] >> 2));
        }
        if pad == 0 {
            out.push((vals[2] << 6) | vals[3]);
        }
        pos += 4;
    }

    Ok(out)
}

fn base64_value(b: u8) -> Option<u8> {
    match b {
        b'A'..=b'Z' => Some(b - b'A'),
        b'a'..=b'z' => Some(b - b'a' + 26),
        b'0'..=b'9' => Some(b - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
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
    let sys_err = match &err {
        sunset::Error::NoAuthMethods => SysError::EACCES,
        sunset::Error::BadUsage { .. } => SysError::EINVAL,
        sunset::Error::SessionEOF | sunset::Error::ChannelEOF => SysError::ENOTCONN,
        _ => SysError::EIO,
    };
    log::info!("sunset ssh error: {:?} -> {:?}", err, sys_err);
    sys_err
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
) -> Result<(Vec<u8>, Vec<u8>), SysError> {
    let deadline = crate::timer::get_time_us().saturating_add(SSH_IO_TIMEOUT_US);
    let mut rxbuf = [0u8; SSH_RX_CHUNK];
    let mut pending_rx = Vec::new();
    let mut ident = IdentCapture::new();
    let mut saw_hostkey = false;

    loop {
        if crate::timer::get_time_us() >= deadline {
            log::info!(
                "ssh kex timeout: saw_hostkey={} input_ready={} pending_rx={}",
                saw_hostkey,
                runner.is_input_ready(),
                pending_rx.len()
            );
            return Err(SysError::ETIMEDOUT);
        }

        let mut progressed = false;

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
                }
                Event::None => {}
            }
        }

        flush_sunset_output(runner, tcp, deadline)?;
        if saw_hostkey && runner.is_initial_kex_done() {
            let peer_ident = ident.finish()?;
            log::info!(
                "ssh kex complete: peer_ident_len={} pending_rx={}",
                peer_ident.len(),
                pending_rx.len()
            );
            return Ok((peer_ident, pending_rx));
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
            suspend_current_and_run_next();
        }
    }
}

fn drive_sunset_password_auth(
    runner: &mut Runner<'static, sunset::Client>,
    tcp: &Arc<Mutex<TcpSocket>>,
    pending_rx: &mut Vec<u8>,
    username: &str,
    password: &str,
) -> Result<(), SysError> {
    let deadline = crate::timer::get_time_us().saturating_add(SSH_IO_TIMEOUT_US);
    let mut rxbuf = [0u8; SSH_RX_CHUNK];
    let mut password_sent = false;

    loop {
        let mut progressed = false;

        {
            let event = runner.progress().map_err(sunset_error)?;
            match event {
                Event::Cli(CliEvent::Username(request)) => {
                    log::info!("ssh auth event: username");
                    request.username(username).map_err(sunset_error)?;
                    progressed = true;
                }
                Event::Cli(CliEvent::Pubkey(request)) => {
                    log::info!("ssh auth event: skip pubkey");
                    request.skip().map_err(sunset_error)?;
                    progressed = true;
                }
                Event::Cli(CliEvent::Password(request)) => {
                    if password_sent {
                        return Err(SysError::EACCES);
                    }
                    log::info!("ssh auth event: password");
                    request.password(password).map_err(sunset_error)?;
                    password_sent = true;
                    progressed = true;
                }
                Event::Cli(CliEvent::Hostkey(hostkey)) => {
                    log::info!("ssh auth event: hostkey");
                    hostkey.accept().map_err(sunset_error)?;
                    progressed = true;
                }
                Event::Cli(CliEvent::Banner(banner)) => {
                    if let Ok(text) = banner.banner() {
                        log::info!("ssh auth banner: {}", text);
                    }
                    progressed = true;
                }
                Event::Cli(CliEvent::Authenticated) => {
                    log::info!("ssh auth event: authenticated");
                    return Ok(());
                }
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

        if feed_pending_sunset_input(runner, pending_rx)? {
            continue;
        }

        if pending_rx.is_empty() && runner.is_input_ready() {
            match recv_some(tcp, &mut rxbuf) {
                Ok(0) => return Err(SysError::ENOTCONN),
                Ok(n) => {
                    log::info!("ssh auth rx {} bytes", n);
                    pending_rx.extend_from_slice(&rxbuf[..n]);
                    progressed = true;
                    let _ = feed_pending_sunset_input(runner, pending_rx)?;
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

fn drive_sunset_publickey_auth(
    runner: &mut Runner<'static, sunset::Client>,
    tcp: &Arc<Mutex<TcpSocket>>,
    pending_rx: &mut Vec<u8>,
    username: &str,
    key: SignKey,
) -> Result<(), SysError> {
    let deadline = crate::timer::get_time_us().saturating_add(SSH_IO_TIMEOUT_US);
    let mut rxbuf = [0u8; SSH_RX_CHUNK];
    let mut key = Some(key);

    loop {
        let mut progressed = false;

        {
            let event = runner.progress().map_err(sunset_error)?;
            match event {
                Event::Cli(CliEvent::Username(request)) => {
                    log::info!("ssh auth event: username");
                    request.username(username).map_err(sunset_error)?;
                    progressed = true;
                }
                Event::Cli(CliEvent::Pubkey(request)) => {
                    let Some(signkey) = key.take() else {
                        log::info!("ssh auth event: publickey rejected");
                        return Err(SysError::EACCES);
                    };
                    log::info!("ssh auth event: publickey");
                    request.pubkey(signkey).map_err(sunset_error)?;
                    progressed = true;
                }
                Event::Cli(CliEvent::Password(_)) => {
                    log::info!("ssh auth event: publickey auth unavailable");
                    return Err(SysError::EACCES);
                }
                Event::Cli(CliEvent::Hostkey(hostkey)) => {
                    log::info!("ssh auth event: hostkey");
                    hostkey.accept().map_err(sunset_error)?;
                    progressed = true;
                }
                Event::Cli(CliEvent::Banner(banner)) => {
                    if let Ok(text) = banner.banner() {
                        log::info!("ssh auth banner: {}", text);
                    }
                    progressed = true;
                }
                Event::Cli(CliEvent::Authenticated) => {
                    log::info!("ssh auth event: authenticated");
                    return Ok(());
                }
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

        if feed_pending_sunset_input(runner, pending_rx)? {
            continue;
        }

        if pending_rx.is_empty() && runner.is_input_ready() {
            match recv_some(tcp, &mut rxbuf) {
                Ok(0) => return Err(SysError::ENOTCONN),
                Ok(n) => {
                    log::info!("ssh auth rx {} bytes", n);
                    pending_rx.extend_from_slice(&rxbuf[..n]);
                    progressed = true;
                    let _ = feed_pending_sunset_input(runner, pending_rx)?;
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

enum SessionRequest<'a> {
    Exec(&'a str),
    Shell,
}

impl SessionRequest<'_> {
    fn name(&self) -> &'static str {
        match self {
            Self::Exec(_) => "exec",
            Self::Shell => "shell",
        }
    }
}

fn drive_sunset_session_request(
    runner: &mut Runner<'static, sunset::Client>,
    tcp: &Arc<Mutex<TcpSocket>>,
    pending_rx: &mut Vec<u8>,
    request: SessionRequest<'_>,
) -> Result<ChanHandle, SysError> {
    let mut channel = Some(runner.open_client_session().map_err(sunset_error)?);
    let channel_num = channel.as_ref().unwrap().num();
    let deadline = crate::timer::get_time_us().saturating_add(SSH_IO_TIMEOUT_US);
    let mut rxbuf = [0u8; SSH_RX_CHUNK];

    loop {
        let mut progressed = false;
        let mut opened = None;

        {
            let event = runner.progress().map_err(sunset_error)?;
            match event {
                Event::Cli(CliEvent::SessionOpened(mut opener)) => {
                    if opener.channel() == channel_num {
                        log::info!(
                            "ssh {} session opened: channel={}",
                            request.name(),
                            channel_num
                        );
                        match &request {
                            SessionRequest::Exec(command) => {
                                opener.exec(*command).map_err(sunset_error)?;
                            }
                            SessionRequest::Shell => {
                                opener.shell().map_err(sunset_error)?;
                            }
                        }
                        opened = Some(channel.take().ok_or(SysError::EIO)?);
                    } else {
                        progressed = true;
                    }
                }
                Event::Cli(CliEvent::Banner(banner)) => {
                    if let Ok(text) = banner.banner() {
                        log::info!("ssh exec banner: {}", text);
                    }
                    progressed = true;
                }
                Event::Cli(CliEvent::Hostkey(hostkey)) => {
                    hostkey.accept().map_err(sunset_error)?;
                    progressed = true;
                }
                Event::Cli(CliEvent::PollAgain) | Event::Progressed => progressed = true,
                Event::Cli(CliEvent::Defunct) => return Err(SysError::ENOTCONN),
                Event::Cli(other) => {
                    log::info!(
                        "unexpected ssh client event during {} open: {:?}",
                        request.name(),
                        other
                    );
                    return Err(SysError::EIO);
                }
                Event::Serv(_) => return Err(SysError::EIO),
                Event::None => {}
            }
        }

        flush_sunset_output(runner, tcp, deadline)?;
        if let Some(opened) = opened {
            return Ok(opened);
        }

        if feed_pending_sunset_input(runner, pending_rx)? {
            continue;
        }

        if pending_rx.is_empty() && runner.is_input_ready() {
            match recv_some(tcp, &mut rxbuf) {
                Ok(0) => return Err(SysError::ENOTCONN),
                Ok(n) => {
                    log::info!("ssh {} open rx {} bytes", request.name(), n);
                    pending_rx.extend_from_slice(&rxbuf[..n]);
                    progressed = true;
                    let _ = feed_pending_sunset_input(runner, pending_rx)?;
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

fn drive_sunset_channel(
    runner: &mut Runner<'static, sunset::Client>,
    tcp: &Arc<Mutex<TcpSocket>>,
    pending_rx: &mut Vec<u8>,
    channel: &mut SshChannel,
    read_buf: Option<&mut [u8]>,
    wait: bool,
) -> Result<Option<usize>, SysError> {
    let deadline = crate::timer::get_time_us().saturating_add(SSH_IO_TIMEOUT_US);
    let mut rxbuf = [0u8; SSH_RX_CHUNK];
    let mut read_buf = read_buf;

    loop {
        let mut progressed = false;

        {
            let event = runner.progress().map_err(sunset_error)?;
            match event {
                Event::Cli(CliEvent::SessionExit(exit)) => {
                    match exit {
                        sunset::CliSessionExit::Status(code) => {
                            channel.exit_status = Some((code & 0xff) as i32);
                        }
                        sunset::CliSessionExit::Signal(_) => {
                            channel.exit_status = Some(128);
                        }
                    }
                    progressed = true;
                }
                Event::Cli(CliEvent::Banner(banner)) => {
                    if let Ok(text) = banner.banner() {
                        log::info!("ssh channel banner: {}", text);
                    }
                    progressed = true;
                }
                Event::Cli(CliEvent::Hostkey(hostkey)) => {
                    hostkey.accept().map_err(sunset_error)?;
                    progressed = true;
                }
                Event::Cli(CliEvent::PollAgain) | Event::Progressed => progressed = true,
                Event::Cli(CliEvent::Defunct) => {
                    channel.closed = true;
                    return Ok(Some(0));
                }
                Event::Cli(other) => {
                    log::debug!("unexpected ssh client event during channel io: {:?}", other);
                    progressed = true;
                }
                Event::Serv(_) => return Err(SysError::EIO),
                Event::None => {}
            }
        }

        flush_sunset_output(runner, tcp, deadline)?;

        if let (Some(handle), Some(out)) = (channel.handle.as_ref(), read_buf.as_deref_mut()) {
            if !out.is_empty() {
                if let Some((ready_ch, _dt, _len)) = runner.read_channel_ready() {
                    if ready_ch == handle.num() {
                        let (n, _dt) = runner
                            .read_channel_either(handle, out)
                            .map_err(sunset_error)?;
                        flush_sunset_output(runner, tcp, deadline)?;
                        return Ok(Some(n));
                    }
                }
            } else {
                return Ok(Some(0));
            }
        }

        if feed_pending_sunset_input(runner, pending_rx)? {
            progressed = true;
        }

        if pending_rx.is_empty() && runner.is_input_ready() {
            match recv_some(tcp, &mut rxbuf) {
                Ok(0) => {
                    channel.closed = true;
                    return Ok(Some(0));
                }
                Ok(n) => {
                    log::info!("ssh channel rx {} bytes", n);
                    pending_rx.extend_from_slice(&rxbuf[..n]);
                    progressed = true;
                    if feed_pending_sunset_input(runner, pending_rx)? {
                        continue;
                    }
                }
                Err(SysError::EAGAIN) => {}
                Err(err) => return Err(err),
            }
        }

        if let Some(handle) = channel.handle.as_ref() {
            if runner.is_channel_closed(handle) || runner.is_channel_eof(handle) {
                channel.closed = true;
                return Ok(Some(0));
            }
        } else {
            channel.closed = true;
            return Ok(Some(0));
        }

        if channel.exit_status.is_some() && read_buf.is_none() {
            return Ok(None);
        }

        if !progressed {
            if tcp_is_closed(tcp) {
                channel.closed = true;
                return Ok(Some(0));
            }
            if !wait {
                return Err(SysError::EAGAIN);
            }
            if crate::timer::get_time_us() >= deadline {
                if read_buf.is_some() {
                    return Err(SysError::ETIMEDOUT);
                }
                return Ok(None);
            }
            suspend_current_and_run_next();
        }
    }
}

fn drive_sunset_channel_write(
    runner: &mut Runner<'static, sunset::Client>,
    tcp: &Arc<Mutex<TcpSocket>>,
    pending_rx: &mut Vec<u8>,
    channel: &mut SshChannel,
    input: &[u8],
) -> Result<usize, SysError> {
    if input.is_empty() {
        return Ok(0);
    }

    let deadline = crate::timer::get_time_us().saturating_add(SSH_IO_TIMEOUT_US);
    let mut rxbuf = [0u8; SSH_RX_CHUNK];

    loop {
        let mut progressed = false;

        {
            let event = runner.progress().map_err(sunset_error)?;
            match event {
                Event::Cli(CliEvent::SessionExit(exit)) => {
                    match exit {
                        sunset::CliSessionExit::Status(code) => {
                            channel.exit_status = Some((code & 0xff) as i32);
                        }
                        sunset::CliSessionExit::Signal(_) => {
                            channel.exit_status = Some(128);
                        }
                    }
                    progressed = true;
                }
                Event::Cli(CliEvent::Banner(banner)) => {
                    if let Ok(text) = banner.banner() {
                        log::info!("ssh channel write banner: {}", text);
                    }
                    progressed = true;
                }
                Event::Cli(CliEvent::Hostkey(hostkey)) => {
                    hostkey.accept().map_err(sunset_error)?;
                    progressed = true;
                }
                Event::Cli(CliEvent::PollAgain) | Event::Progressed => progressed = true,
                Event::Cli(CliEvent::Defunct) => {
                    channel.closed = true;
                    return Err(SysError::ENOTCONN);
                }
                Event::Cli(other) => {
                    log::debug!(
                        "unexpected ssh client event during channel write: {:?}",
                        other
                    );
                    progressed = true;
                }
                Event::Serv(_) => return Err(SysError::EIO),
                Event::None => {}
            }
        }

        if let Some(handle) = channel.handle.as_ref() {
            let n = runner
                .write_channel(handle, ChanData::Normal, input)
                .map_err(sunset_error)?;
            if n > 0 {
                flush_sunset_output(runner, tcp, deadline)?;
                return Ok(n);
            }
        } else {
            channel.closed = true;
            return Err(SysError::EBADF);
        }

        flush_sunset_output(runner, tcp, deadline)?;

        if feed_pending_sunset_input(runner, pending_rx)? {
            progressed = true;
        }

        if pending_rx.is_empty() && runner.is_input_ready() {
            match recv_some(tcp, &mut rxbuf) {
                Ok(0) => {
                    channel.closed = true;
                    return Err(SysError::ENOTCONN);
                }
                Ok(n) => {
                    log::info!("ssh channel write rx {} bytes", n);
                    pending_rx.extend_from_slice(&rxbuf[..n]);
                    progressed = true;
                    let _ = feed_pending_sunset_input(runner, pending_rx)?;
                }
                Err(SysError::EAGAIN) => {}
                Err(err) => return Err(err),
            }
        }

        if let Some(handle) = channel.handle.as_ref() {
            if runner.is_channel_closed(handle) || runner.is_channel_eof(handle) {
                channel.closed = true;
                return Err(SysError::ENOTCONN);
            }
        }

        if !progressed {
            if tcp_is_closed(tcp) {
                channel.closed = true;
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
    let (peer_ident, pending_rx) = drive_sunset_kex(&mut runner, &tcp)?;

    let owner_pid = current_process().getpid();
    log::info!(
        "ssh connect kex returned: fd={} peer_ident_len={} pending_rx={}",
        fd,
        peer_ident.len(),
        pending_rx.len()
    );
    log::info!("ssh connect manager lock begin");
    let mut manager = SSH_MANAGER.lock();
    log::info!("ssh connect manager lock acquired");
    let session = SshSession {
        owner_pid,
        fd,
        tcp: Some(tcp),
        peer_ident,
        runner: Some(runner),
        pending_rx,
        channels: Vec::new(),
        authenticated: false,
        closed: false,
    };
    log::info!("ssh connect session prepared");
    let id = manager.insert(session);
    log::info!("ssh connect session inserted: id={} fd={}", id, fd);
    Ok(id)
}

/// Authenticate the SSH session with a username and password.
pub fn auth_password(ssh_id: usize, username: &str, password: &str) -> SyscallResult {
    if username.is_empty() {
        return Err(SysError::EINVAL);
    }

    let pid = current_process().getpid();
    let (mut runner, tcp, mut pending_rx, already_authenticated) = {
        let mut manager = SSH_MANAGER.lock();
        let session = manager.get_mut(ssh_id).ok_or(SysError::EBADF)?;
        if session.closed || session.owner_pid != pid {
            return Err(SysError::EBADF);
        }
        (
            session.runner.take().ok_or(SysError::EIO)?,
            session.tcp.as_ref().ok_or(SysError::EIO)?.clone(),
            core::mem::take(&mut session.pending_rx),
            session.authenticated,
        )
    };

    let result = if already_authenticated {
        Ok(())
    } else {
        drive_sunset_password_auth(&mut runner, &tcp, &mut pending_rx, username, password)
    };

    let mut manager = SSH_MANAGER.lock();
    if let Some(session) = manager.get_mut(ssh_id) {
        if result.is_ok() {
            session.authenticated = true;
        }
        if session.closed {
            mem::forget(runner);
        } else {
            session.pending_rx = pending_rx;
            session.runner = Some(runner);
        }
    } else {
        mem::forget(runner);
    }

    result.map(|_| 0)
}

/// Authenticate the SSH session with an OpenSSH private key.
pub fn auth_publickey(ssh_id: usize, username: &str, private_key: &[u8]) -> SyscallResult {
    if username.is_empty() || private_key.is_empty() {
        return Err(SysError::EINVAL);
    }

    let key = parse_openssh_ed25519_key(private_key)?;

    let pid = current_process().getpid();
    let (mut runner, tcp, mut pending_rx, already_authenticated) = {
        let mut manager = SSH_MANAGER.lock();
        let session = manager.get_mut(ssh_id).ok_or(SysError::EBADF)?;
        if session.closed || session.owner_pid != pid {
            return Err(SysError::EBADF);
        }
        (
            session.runner.take().ok_or(SysError::EIO)?,
            session.tcp.as_ref().ok_or(SysError::EIO)?.clone(),
            core::mem::take(&mut session.pending_rx),
            session.authenticated,
        )
    };

    let result = if already_authenticated {
        Ok(())
    } else {
        drive_sunset_publickey_auth(&mut runner, &tcp, &mut pending_rx, username, key)
    };

    let mut manager = SSH_MANAGER.lock();
    if let Some(session) = manager.get_mut(ssh_id) {
        if result.is_ok() {
            session.authenticated = true;
        }
        if session.closed {
            mem::forget(runner);
        } else {
            session.pending_rx = pending_rx;
            session.runner = Some(runner);
        }
    } else {
        mem::forget(runner);
    }

    result.map(|_| 0)
}

fn open_session_channel(ssh_id: usize, request: SessionRequest<'_>) -> SyscallResult {
    let pid = current_process().getpid();
    let (mut runner, tcp, mut pending_rx) = {
        let mut manager = SSH_MANAGER.lock();
        let session = manager.get_mut(ssh_id).ok_or(SysError::EBADF)?;
        if session.closed || session.owner_pid != pid {
            return Err(SysError::EBADF);
        }
        if !session.authenticated {
            return Err(SysError::EACCES);
        }
        if session
            .channels
            .iter()
            .any(|channel| !channel.closed && channel.handle.is_some())
        {
            return Err(SysError::EBUSY);
        }
        (
            session.runner.take().ok_or(SysError::EIO)?,
            session.tcp.as_ref().ok_or(SysError::EIO)?.clone(),
            core::mem::take(&mut session.pending_rx),
        )
    };

    let result = drive_sunset_session_request(&mut runner, &tcp, &mut pending_rx, request);

    let mut manager = SSH_MANAGER.lock();
    if let Some(session) = manager.get_mut(ssh_id) {
        let id = match result {
            Ok(handle) => {
                session.channels.push(SshChannel {
                    handle: Some(handle),
                    exit_status: None,
                    closed: false,
                });
                Ok(session.channels.len())
            }
            Err(err) => Err(err),
        };
        if session.closed {
            mem::forget(runner);
        } else {
            session.pending_rx = pending_rx;
            session.runner = Some(runner);
        }
        id
    } else {
        mem::forget(runner);
        Err(SysError::EBADF)
    }
}

/// Open a no-PTY session channel and request remote command execution.
pub fn exec(ssh_id: usize, command: &str) -> SyscallResult {
    if command.is_empty() {
        return Err(SysError::EINVAL);
    }
    open_session_channel(ssh_id, SessionRequest::Exec(command))
}

/// Open a no-PTY session channel and request an interactive remote shell.
pub fn shell(ssh_id: usize) -> SyscallResult {
    open_session_channel(ssh_id, SessionRequest::Shell)
}

/// Read stdout/stderr data from an exec channel. Returns 0 on EOF.
pub fn channel_read(ssh_id: usize, channel_id: usize, out: &mut [u8]) -> SyscallResult {
    if out.is_empty() {
        return Ok(0);
    }

    let pid = current_process().getpid();
    let (mut runner, tcp, mut pending_rx, mut channel) = {
        let mut manager = SSH_MANAGER.lock();
        let session = manager.get_mut(ssh_id).ok_or(SysError::EBADF)?;
        if session.closed || session.owner_pid != pid {
            return Err(SysError::EBADF);
        }
        let index = channel_id.checked_sub(1).ok_or(SysError::EBADF)?;
        if index >= session.channels.len() {
            return Err(SysError::EBADF);
        }
        (
            session.runner.take().ok_or(SysError::EIO)?,
            session.tcp.as_ref().ok_or(SysError::EIO)?.clone(),
            core::mem::take(&mut session.pending_rx),
            mem::replace(&mut session.channels[index], SshChannel {
                handle: None,
                exit_status: None,
                closed: true,
            }),
        )
    };

    let result = drive_sunset_channel(
        &mut runner,
        &tcp,
        &mut pending_rx,
        &mut channel,
        Some(out),
        true,
    );

    let mut manager = SSH_MANAGER.lock();
    if let Some(session) = manager.get_mut(ssh_id) {
        if let Some(slot) = channel_id
            .checked_sub(1)
            .and_then(|index| session.channels.get_mut(index))
        {
            *slot = channel;
        }
        if session.closed {
            mem::forget(runner);
        } else {
            session.pending_rx = pending_rx;
            session.runner = Some(runner);
        }
    } else {
        mem::forget(runner);
    }

    result.map(|n| n.unwrap_or(0))
}

/// Try to read stdout/stderr data from a channel without waiting.
pub fn channel_try_read(ssh_id: usize, channel_id: usize, out: &mut [u8]) -> SyscallResult {
    if out.is_empty() {
        return Ok(0);
    }

    let pid = current_process().getpid();
    let (mut runner, tcp, mut pending_rx, mut channel) = {
        let mut manager = SSH_MANAGER.lock();
        let session = manager.get_mut(ssh_id).ok_or(SysError::EBADF)?;
        if session.closed || session.owner_pid != pid {
            return Err(SysError::EBADF);
        }
        let index = channel_id.checked_sub(1).ok_or(SysError::EBADF)?;
        if index >= session.channels.len() {
            return Err(SysError::EBADF);
        }
        (
            session.runner.take().ok_or(SysError::EIO)?,
            session.tcp.as_ref().ok_or(SysError::EIO)?.clone(),
            core::mem::take(&mut session.pending_rx),
            mem::replace(&mut session.channels[index], SshChannel {
                handle: None,
                exit_status: None,
                closed: true,
            }),
        )
    };

    let result = drive_sunset_channel(
        &mut runner,
        &tcp,
        &mut pending_rx,
        &mut channel,
        Some(out),
        false,
    );

    let mut manager = SSH_MANAGER.lock();
    if let Some(session) = manager.get_mut(ssh_id) {
        if let Some(slot) = channel_id
            .checked_sub(1)
            .and_then(|index| session.channels.get_mut(index))
        {
            *slot = channel;
        }
        if session.closed {
            mem::forget(runner);
        } else {
            session.pending_rx = pending_rx;
            session.runner = Some(runner);
        }
    } else {
        mem::forget(runner);
    }

    result.map(|n| n.unwrap_or(0))
}

/// Write stdin data to an open session channel.
pub fn channel_write(ssh_id: usize, channel_id: usize, input: &[u8]) -> SyscallResult {
    if input.is_empty() {
        return Ok(0);
    }

    let pid = current_process().getpid();
    let (mut runner, tcp, mut pending_rx, mut channel) = {
        let mut manager = SSH_MANAGER.lock();
        let session = manager.get_mut(ssh_id).ok_or(SysError::EBADF)?;
        if session.closed || session.owner_pid != pid {
            return Err(SysError::EBADF);
        }
        let index = channel_id.checked_sub(1).ok_or(SysError::EBADF)?;
        if index >= session.channels.len() {
            return Err(SysError::EBADF);
        }
        (
            session.runner.take().ok_or(SysError::EIO)?,
            session.tcp.as_ref().ok_or(SysError::EIO)?.clone(),
            core::mem::take(&mut session.pending_rx),
            mem::replace(&mut session.channels[index], SshChannel {
                handle: None,
                exit_status: None,
                closed: true,
            }),
        )
    };

    let result =
        drive_sunset_channel_write(&mut runner, &tcp, &mut pending_rx, &mut channel, input);

    let mut manager = SSH_MANAGER.lock();
    if let Some(session) = manager.get_mut(ssh_id) {
        if let Some(slot) = channel_id
            .checked_sub(1)
            .and_then(|index| session.channels.get_mut(index))
        {
            *slot = channel;
        }
        if session.closed {
            mem::forget(runner);
        } else {
            session.pending_rx = pending_rx;
            session.runner = Some(runner);
        }
    } else {
        mem::forget(runner);
    }

    result
}

/// Return the remote exit status, or EAGAIN while the command is still running.
pub fn channel_status(ssh_id: usize, channel_id: usize) -> SyscallResult {
    let pid = current_process().getpid();
    let (mut runner, tcp, mut pending_rx, mut channel) = {
        let mut manager = SSH_MANAGER.lock();
        let session = manager.get_mut(ssh_id).ok_or(SysError::EBADF)?;
        if session.closed || session.owner_pid != pid {
            return Err(SysError::EBADF);
        }
        let index = channel_id.checked_sub(1).ok_or(SysError::EBADF)?;
        if index >= session.channels.len() {
            return Err(SysError::EBADF);
        }
        if let Some(status) = session.channels[index].exit_status {
            return Ok(status as usize);
        }
        (
            session.runner.take().ok_or(SysError::EIO)?,
            session.tcp.as_ref().ok_or(SysError::EIO)?.clone(),
            core::mem::take(&mut session.pending_rx),
            mem::replace(&mut session.channels[index], SshChannel {
                handle: None,
                exit_status: None,
                closed: true,
            }),
        )
    };

    let result = drive_sunset_channel(&mut runner, &tcp, &mut pending_rx, &mut channel, None, true);
    let status = channel.exit_status;

    let mut manager = SSH_MANAGER.lock();
    if let Some(session) = manager.get_mut(ssh_id) {
        if let Some(slot) = channel_id
            .checked_sub(1)
            .and_then(|index| session.channels.get_mut(index))
        {
            *slot = channel;
        }
        if session.closed {
            mem::forget(runner);
        } else {
            session.pending_rx = pending_rx;
            session.runner = Some(runner);
        }
    } else {
        mem::forget(runner);
    }

    result?;
    match status {
        Some(code) => Ok(code as usize),
        None => Err(SysError::EAGAIN),
    }
}

/// Mark a channel done in Sunset and invalidate its local handle.
pub fn channel_close(ssh_id: usize, channel_id: usize) -> SyscallResult {
    let pid = current_process().getpid();
    let (mut runner, mut channel) = {
        let mut manager = SSH_MANAGER.lock();
        let session = manager.get_mut(ssh_id).ok_or(SysError::EBADF)?;
        if session.closed || session.owner_pid != pid {
            return Err(SysError::EBADF);
        }
        let index = channel_id.checked_sub(1).ok_or(SysError::EBADF)?;
        if index >= session.channels.len() {
            return Err(SysError::EBADF);
        }
        (
            session.runner.take().ok_or(SysError::EIO)?,
            mem::replace(&mut session.channels[index], SshChannel {
                handle: None,
                exit_status: None,
                closed: true,
            }),
        )
    };

    let result = match channel.handle.take() {
        Some(handle) => runner.channel_done(handle).map_err(sunset_error).map(|_| 0),
        None if channel.closed => Ok(0),
        None => Err(SysError::EBADF),
    };
    channel.closed = true;

    let mut manager = SSH_MANAGER.lock();
    if let Some(session) = manager.get_mut(ssh_id) {
        if let Some(slot) = channel_id
            .checked_sub(1)
            .and_then(|index| session.channels.get_mut(index))
        {
            *slot = channel;
        }
        if session.closed {
            mem::forget(runner);
        } else {
            session.runner = Some(runner);
        }
    } else {
        mem::forget(runner);
    }

    result
}

/// Return the peer SSH identification string.
pub fn peer_ident(ssh_id: usize, out: &mut [u8]) -> SyscallResult {
    let pid = current_process().getpid();
    let manager = SSH_MANAGER.lock();
    let session = manager.get(ssh_id).ok_or(SysError::EBADF)?;
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
    let session = manager.get(ssh_id).ok_or(SysError::EBADF)?;
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
    let session = manager.get(ssh_id).ok_or(SysError::EBADF)?;
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
    {
        let session = manager.get(ssh_id).ok_or(SysError::EBADF)?;
        if session.closed || session.owner_pid != pid {
            return Err(SysError::EBADF);
        }
    }

    let mut session = manager.remove(ssh_id).ok_or(SysError::EBADF)?;
    log::debug!("closing ssh session {} for fd {}", ssh_id, session.fd);
    session.closed = true;
    let tcp = session.tcp.take();
    session.peer_ident.clear();
    session.pending_rx.clear();
    // Sunset owns leaked packet buffers here; dropping the runner can stall close.
    if let Some(runner) = session.runner.take() {
        mem::forget(runner);
    }
    drop(manager);
    drop(tcp);
    Ok(0)
}
