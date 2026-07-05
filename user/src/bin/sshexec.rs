#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    close, connect, socket, ssh_auth_password, ssh_channel_close, ssh_channel_read,
    ssh_channel_status, ssh_close, ssh_connect, ssh_exec,
};

const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const IPPROTO_TCP: i32 = 6;
const DEFAULT_IDENT: &str = "SSH-2.0-kairix-sshexec_0.1";
const EAGAIN_RET: isize = -11;

#[repr(C)]
#[derive(Clone, Copy)]
struct SockAddrIn {
    sin_family: u16,
    sin_port: u16,
    sin_addr: u32,
    sin_zero: [u8; 8],
}

impl SockAddrIn {
    fn new(ip: u32, port: u16) -> Self {
        Self {
            sin_family: AF_INET as u16,
            sin_port: port.to_be(),
            sin_addr: ip.to_be(),
            sin_zero: [0; 8],
        }
    }
}

#[unsafe(no_mangle)]
pub fn main_with_args(argc: usize, argv: *const usize) -> i32 {
    if argc < 6 {
        println!("usage: sshexec <ipv4> <port> <username> <password> <command>");
        println!("example: sshexec 10.0.2.2 22 user password \"uname -a\"");
        return -1;
    }

    let host = match argv_str(argv, 1).and_then(parse_ipv4) {
        Some(ip) => ip,
        None => {
            println!("invalid ipv4 address");
            return -1;
        }
    };
    let port = match argv_str(argv, 2).and_then(parse_u16) {
        Some(port) => port,
        None => {
            println!("invalid port");
            return -1;
        }
    };
    let username = match argv_str(argv, 3) {
        Some(v) => v,
        None => {
            println!("invalid username");
            return -1;
        }
    };
    let password = match argv_str(argv, 4) {
        Some(v) => v,
        None => {
            println!("invalid password");
            return -1;
        }
    };
    let command = match argv_str(argv, 5) {
        Some(v) if !v.is_empty() => v,
        _ => {
            println!("invalid command");
            return -1;
        }
    };

    run(host, port, username, password, command)
}

fn run(host: u32, port: u16, username: &str, password: &str, command: &str) -> i32 {
    let fd = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if fd < 0 {
        println!("socket failed: {}", fd);
        return -1;
    }
    let fd = fd as usize;

    let addr = SockAddrIn::new(host, port);
    let ret = connect(
        fd,
        &addr as *const SockAddrIn as *const u8,
        core::mem::size_of::<SockAddrIn>(),
    );
    if ret < 0 {
        println!("tcp connect failed: {}", ret);
        let _ = close(fd);
        return -1;
    }
    println!("[ok] tcp connect");

    let ssh_id = ssh_connect(fd, DEFAULT_IDENT);
    if ssh_id < 0 {
        println!("ssh connect failed: {}", ssh_id);
        let _ = close(fd);
        return -1;
    }
    let ssh_id = ssh_id as usize;
    println!("[ok] ssh connect");

    let ret = ssh_auth_password(ssh_id, username, password);
    if ret < 0 {
        println!("ssh password auth failed: {}", ret);
        let _ = ssh_close(ssh_id);
        let _ = close(fd);
        return -1;
    }
    println!("[ok] password auth");

    let channel_id = ssh_exec(ssh_id, command);
    if channel_id < 0 {
        println!("ssh exec failed: {}", channel_id);
        let _ = ssh_close(ssh_id);
        let _ = close(fd);
        return -1;
    }
    let channel_id = channel_id as usize;
    println!("[ok] exec channel {}", channel_id);

    let mut buf = [0u8; 1024];
    loop {
        let n = ssh_channel_read(ssh_id, channel_id, &mut buf);
        if n < 0 {
            println!("\nssh channel read failed: {}", n);
            let _ = ssh_channel_close(ssh_id, channel_id);
            let _ = ssh_close(ssh_id);
            let _ = close(fd);
            return -1;
        }
        if n == 0 {
            break;
        }
        print_bytes(&buf[..n as usize]);
    }

    let mut status = ssh_channel_status(ssh_id, channel_id);
    while status == EAGAIN_RET {
        status = ssh_channel_status(ssh_id, channel_id);
    }
    if status < 0 {
        println!("\nssh channel status failed: {}", status);
        let _ = ssh_channel_close(ssh_id, channel_id);
        let _ = ssh_close(ssh_id);
        let _ = close(fd);
        return -1;
    }
    println!("\n[exit] {}", status);

    let _ = ssh_channel_close(ssh_id, channel_id);
    let _ = ssh_close(ssh_id);
    let _ = close(fd);
    (status & 0xff) as i32
}

fn argv_str(argv: *const usize, idx: usize) -> Option<&'static str> {
    cstr_to_str(unsafe { *argv.add(idx) as *const u8 })
}

fn cstr_to_str(ptr: *const u8) -> Option<&'static str> {
    if ptr.is_null() {
        return None;
    }
    let mut len = 0usize;
    loop {
        let b = unsafe { *ptr.add(len) };
        if b == 0 {
            break;
        }
        len += 1;
        if len > 512 {
            return None;
        }
    }
    let bytes = unsafe { core::slice::from_raw_parts(ptr, len) };
    core::str::from_utf8(bytes).ok()
}

fn parse_ipv4(s: &str) -> Option<u32> {
    let mut out = 0u32;
    let mut cnt = 0usize;
    for part in s.split('.') {
        if cnt >= 4 || part.is_empty() {
            return None;
        }
        let mut val = 0u32;
        for b in part.bytes() {
            if !b.is_ascii_digit() {
                return None;
            }
            val = val.checked_mul(10)?.checked_add((b - b'0') as u32)?;
            if val > 255 {
                return None;
            }
        }
        out = (out << 8) | val;
        cnt += 1;
    }
    if cnt == 4 { Some(out) } else { None }
}

fn parse_u16(s: &str) -> Option<u16> {
    if s.is_empty() {
        return None;
    }
    let mut out = 0u32;
    for b in s.bytes() {
        if !b.is_ascii_digit() {
            return None;
        }
        out = out.checked_mul(10)?.checked_add((b - b'0') as u32)?;
        if out > 65535 {
            return None;
        }
    }
    if out == 0 { None } else { Some(out as u16) }
}

fn print_bytes(bytes: &[u8]) {
    for &b in bytes {
        match b {
            b'\r' => {}
            b'\n' => println!(""),
            0x20..=0x7e | b'\t' => print!("{}", b as char),
            _ => print!("."),
        }
    }
}
