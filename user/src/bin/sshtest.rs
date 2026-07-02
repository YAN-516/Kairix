#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    close, connect, socket, ssh_close, ssh_connect, ssh_connect_raw, ssh_peer_ident,
    ssh_peer_ident_raw, ssh_read_raw, ssh_write_raw,
};

const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const IPPROTO_TCP: i32 = 6;
const SSH_PORT: u16 = 22;
const DEFAULT_HOST: u32 = 0x0A000202; // 10.0.2.2, QEMU host
const DEFAULT_IDENT: &str = "SSH-2.0-kairix-sshtest_0.1";
const EBADF_RET: isize = -9;
const ENOTSOCK_RET: isize = -88;
const EINVAL_RET: isize = -22;

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
    if argc > 1 {
        if let Some(arg) = cstr_to_str(unsafe { *argv.add(1) as *const u8 }) {
            if arg == "--selftest" {
                return run_selftests();
            }
        }
    }

    if argc < 2 {
        println!("usage: sshtest <ipv4> [port] [client-ident]");
        println!("       sshtest --selftest");
        println!("no argv detected, fallback to: sshtest 10.0.2.2 22");
    }

    let host = if argc > 1 {
        match cstr_to_str(unsafe { *argv.add(1) as *const u8 }).and_then(parse_ipv4) {
            Some(ip) => ip,
            None => {
                println!("invalid ipv4 address");
                return -1;
            }
        }
    } else {
        DEFAULT_HOST
    };

    let port = if argc > 2 {
        match cstr_to_str(unsafe { *argv.add(2) as *const u8 }).and_then(parse_u16) {
            Some(port) => port,
            None => {
                println!("invalid port");
                return -1;
            }
        }
    } else {
        SSH_PORT
    };

    let ident = if argc > 3 {
        match cstr_to_str(unsafe { *argv.add(3) as *const u8 }) {
            Some(ident) => ident,
            None => {
                println!("invalid client ident");
                return -1;
            }
        }
    } else {
        DEFAULT_IDENT
    };

    println!(
        "connecting to {}.{}.{}.{}:{}",
        (host >> 24) & 0xff,
        (host >> 16) & 0xff,
        (host >> 8) & 0xff,
        host & 0xff,
        port
    );

    test_ssh(host, port, ident)
}

fn test_ssh(host: u32, port: u16, ident: &str) -> i32 {
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

    let ssh_id = ssh_connect(fd, ident);
    if ssh_id < 0 {
        println!("ssh connect failed: {}", ssh_id);
        let _ = close(fd);
        return -1;
    }
    let ssh_id = ssh_id as usize;

    if !check_peer_ident(ssh_id) {
        let _ = ssh_close(ssh_id);
        let _ = close(fd);
        return -1;
    }

    let close_ret = ssh_close(ssh_id);
    expect("ssh close", close_ret == 0);
    let second_close = ssh_close(ssh_id);
    expect("ssh double close returns EBADF", second_close == EBADF_RET);
    let _ = close(fd);
    0
}

fn run_selftests() -> i32 {
    let mut passed = 0usize;
    let mut total = 0usize;

    record(
        "ssh_connect invalid fd returns EBADF",
        ssh_connect(usize::MAX, DEFAULT_IDENT) == EBADF_RET,
        &mut passed,
        &mut total,
    );

    let udp_fd = socket(AF_INET, 2, 0);
    if udp_fd >= 0 {
        record(
            "ssh_connect non-TCP fd returns ENOTSOCK",
            ssh_connect(udp_fd as usize, DEFAULT_IDENT) == ENOTSOCK_RET,
            &mut passed,
            &mut total,
        );
        let _ = close(udp_fd as usize);
    } else {
        record(
            "create UDP socket for non-TCP check",
            false,
            &mut passed,
            &mut total,
        );
    }

    let tcp_fd = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if tcp_fd >= 0 {
        record(
            "ssh_connect open TCP fd returns ENOTCONN",
            ssh_connect(tcp_fd as usize, DEFAULT_IDENT) == -107,
            &mut passed,
            &mut total,
        );
        record(
            "ssh_connect bad ident returns EINVAL",
            ssh_connect(tcp_fd as usize, "bad-ident") == EINVAL_RET,
            &mut passed,
            &mut total,
        );
        record(
            "ssh_connect zero ident returns EINVAL",
            ssh_connect_raw(tcp_fd as usize, core::ptr::null(), 0) == EINVAL_RET,
            &mut passed,
            &mut total,
        );
        let _ = close(tcp_fd as usize);
    } else {
        record(
            "create TCP socket for connect checks",
            false,
            &mut passed,
            &mut total,
        );
    }

    record(
        "ssh_write stale handle returns EBADF",
        ssh_write_raw(0xfeed, core::ptr::null(), 0) == EBADF_RET,
        &mut passed,
        &mut total,
    );
    record(
        "ssh_read stale handle returns EBADF",
        ssh_read_raw(0xfeed, core::ptr::null_mut(), 0) == EBADF_RET,
        &mut passed,
        &mut total,
    );
    record(
        "ssh_peer_ident stale handle returns EBADF",
        ssh_peer_ident_raw(0xfeed, core::ptr::null_mut(), 0) == EBADF_RET,
        &mut passed,
        &mut total,
    );
    record(
        "ssh_close stale handle returns EBADF",
        ssh_close(0xfeed) == EBADF_RET,
        &mut passed,
        &mut total,
    );

    println!("selftest result: {}/{} passed", passed, total);
    if passed == total { 0 } else { -1 }
}

fn check_peer_ident(ssh_id: usize) -> bool {
    let full_len = ssh_peer_ident_raw(ssh_id, core::ptr::null_mut(), 0);
    if full_len < 0 {
        println!("ssh peer ident length failed: {}", full_len);
        return false;
    }
    let full_len = full_len as usize;

    let mut small = [0u8; 4];
    let small_n = ssh_peer_ident(ssh_id, &mut small);
    if small_n < 0 {
        println!("ssh peer ident small buffer failed: {}", small_n);
        return false;
    }
    if small_n as usize != small.len().min(full_len) {
        println!("ssh peer ident small buffer length mismatch");
        return false;
    }

    let mut banner = [0u8; 256];
    let n = ssh_peer_ident(ssh_id, &mut banner);
    if n < 0 {
        println!("ssh peer ident failed: {}", n);
        return false;
    }
    if n as usize != full_len.min(banner.len()) {
        println!("ssh peer ident full buffer length mismatch");
        return false;
    }
    if n as usize > 0 && !starts_with(&banner[..n as usize], b"SSH-") {
        println!("ssh peer ident missing SSH- prefix");
        return false;
    }

    print!("ssh peer ident: ");
    print_bytes(&banner[..n as usize]);
    println!("");
    expect("ssh peer ident length query", full_len >= n as usize);
    expect("ssh peer ident small buffer truncates", small_n as usize == small.len().min(full_len));
    true
}

fn expect(name: &str, ok: bool) -> bool {
    if ok {
        println!("[ok] {}", name);
    } else {
        println!("[fail] {}", name);
    }
    ok
}

fn record(name: &str, ok: bool, passed: &mut usize, total: &mut usize) {
    *total += 1;
    if expect(name, ok) {
        *passed += 1;
    }
}

fn starts_with(buf: &[u8], prefix: &[u8]) -> bool {
    buf.len() >= prefix.len() && &buf[..prefix.len()] == prefix
}

fn cstr_to_str(ptr: *const u8) -> Option<&'static str> {
    if ptr.is_null() {
        return None;
    }
    let mut len = 0usize;
    unsafe {
        while *ptr.add(len) != 0 {
            len += 1;
        }
        core::str::from_utf8(core::slice::from_raw_parts(ptr, len)).ok()
    }
}

fn parse_ipv4(s: &str) -> Option<u32> {
    let bytes = s.as_bytes();
    let mut parts = [0u8; 4];
    let mut part = 0usize;
    let mut value = 0u32;
    let mut saw_digit = false;

    for &b in bytes {
        if b == b'.' {
            if !saw_digit || value > 255 || part >= 3 {
                return None;
            }
            parts[part] = value as u8;
            part += 1;
            value = 0;
            saw_digit = false;
        } else if b.is_ascii_digit() {
            value = value.saturating_mul(10).saturating_add((b - b'0') as u32);
            saw_digit = true;
        } else {
            return None;
        }
    }

    if !saw_digit || value > 255 || part != 3 {
        return None;
    }
    parts[3] = value as u8;

    Some(
        ((parts[0] as u32) << 24)
            | ((parts[1] as u32) << 16)
            | ((parts[2] as u32) << 8)
            | parts[3] as u32,
    )
}

fn parse_u16(s: &str) -> Option<u16> {
    if s.is_empty() {
        return None;
    }
    let mut value = 0u32;
    for &b in s.as_bytes() {
        if !b.is_ascii_digit() {
            return None;
        }
        value = value.saturating_mul(10).saturating_add((b - b'0') as u32);
        if value > u16::MAX as u32 {
            return None;
        }
    }
    Some(value as u16)
}

fn print_bytes(bytes: &[u8]) {
    for &b in bytes {
        if b == b'\r' {
            print!("\\r");
        } else if b == b'\n' {
            print!("\\n");
        } else if b.is_ascii_graphic() || b == b' ' {
            print!("{}", b as char);
        } else {
            print!(".");
        }
    }
}
