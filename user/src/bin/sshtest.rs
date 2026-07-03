#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    close, connect, socket, ssh_auth_password, ssh_close, ssh_connect, ssh_connect_raw,
    ssh_peer_ident, ssh_peer_ident_raw, ssh_read, ssh_read_raw, ssh_write, ssh_write_raw,
};

const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const IPPROTO_TCP: i32 = 6;
const SSH_PORT: u16 = 22;
const DEFAULT_HOST: u32 = 0x0A000202; // 10.0.2.2, QEMU host
const DEFAULT_IDENT: &str = "SSH-2.0-kairix-sshtest_0.1";
const EBADF_RET: isize = -9;
const ENOTSOCK_RET: isize = -88;
const ENOTCONN_RET: isize = -107;
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
        println!("usage: sshtest <ipv4> [port] [client-ident] [username] [password]");
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

    let auth = if argc > 5 {
        let username = match cstr_to_str(unsafe { *argv.add(4) as *const u8 }) {
            Some(username) => username,
            None => {
                println!("invalid username");
                return -1;
            }
        };
        let password = match cstr_to_str(unsafe { *argv.add(5) as *const u8 }) {
            Some(password) => password,
            None => {
                println!("invalid password");
                return -1;
            }
        };
        Some((username, password))
    } else {
        None
    };

    println!(
        "connecting to {}.{}.{}.{}:{}",
        (host >> 24) & 0xff,
        (host >> 16) & 0xff,
        (host >> 8) & 0xff,
        host & 0xff,
        port
    );

    test_ssh(host, port, ident, auth)
}

fn test_ssh(host: u32, port: u16, ident: &str, auth: Option<(&str, &str)>) -> i32 {
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
    if !check_connected_zero_io(ssh_id) {
        let _ = ssh_close(ssh_id);
        let _ = close(fd);
        return -1;
    }
    if let Some((username, password)) = auth {
        let auth_ret = ssh_auth_password(ssh_id, username, password);
        if !expect("ssh password auth", auth_ret == 0) {
            println!("ssh password auth failed: {}", auth_ret);
            let _ = ssh_close(ssh_id);
            let _ = close(fd);
            return -1;
        }
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
        let tcp_fd = tcp_fd as usize;
        record(
            "ssh_connect open TCP fd returns ENOTCONN",
            ssh_connect(tcp_fd, DEFAULT_IDENT) == ENOTCONN_RET,
            &mut passed,
            &mut total,
        );
        record(
            "ssh_connect bad ident returns EINVAL",
            ssh_connect(tcp_fd, "bad-ident") == EINVAL_RET,
            &mut passed,
            &mut total,
        );
        record(
            "ssh_connect zero ident returns EINVAL",
            ssh_connect_raw(tcp_fd, core::ptr::null(), 0) == EINVAL_RET,
            &mut passed,
            &mut total,
        );

        let ident_with_cr = b"SSH-2.0-bad\rident";
        record(
            "ssh_connect ident with CR returns EINVAL",
            ssh_connect_raw(tcp_fd, ident_with_cr.as_ptr(), ident_with_cr.len()) == EINVAL_RET,
            &mut passed,
            &mut total,
        );
        let ident_with_lf = b"SSH-2.0-bad\nident";
        record(
            "ssh_connect ident with LF returns EINVAL",
            ssh_connect_raw(tcp_fd, ident_with_lf.as_ptr(), ident_with_lf.len()) == EINVAL_RET,
            &mut passed,
            &mut total,
        );
        let ident_with_nul = b"SSH-2.0-bad\0ident";
        record(
            "ssh_connect ident with NUL returns EINVAL",
            ssh_connect_raw(tcp_fd, ident_with_nul.as_ptr(), ident_with_nul.len()) == EINVAL_RET,
            &mut passed,
            &mut total,
        );

        let mut max_ident = [b'a'; 253];
        max_ident[..4].copy_from_slice(b"SSH-");
        record(
            "ssh_connect max length ident reaches TCP state",
            ssh_connect_raw(tcp_fd, max_ident.as_ptr(), max_ident.len()) == ENOTCONN_RET,
            &mut passed,
            &mut total,
        );

        let mut too_long_ident = [b'a'; 254];
        too_long_ident[..4].copy_from_slice(b"SSH-");
        record(
            "ssh_connect overlong ident returns EINVAL",
            ssh_connect_raw(tcp_fd, too_long_ident.as_ptr(), too_long_ident.len()) == EINVAL_RET,
            &mut passed,
            &mut total,
        );
        let _ = close(tcp_fd);
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
    let payload = b"ignored";
    record(
        "ssh_write stale handle with payload returns EBADF",
        ssh_write_raw(0xfeed, payload.as_ptr(), payload.len()) == EBADF_RET,
        &mut passed,
        &mut total,
    );
    record(
        "ssh_read stale handle returns EBADF",
        ssh_read_raw(0xfeed, core::ptr::null_mut(), 0) == EBADF_RET,
        &mut passed,
        &mut total,
    );
    let mut read_buf = [0u8; 8];
    record(
        "ssh_read stale handle with buffer returns EBADF",
        ssh_read_raw(0xfeed, read_buf.as_mut_ptr(), read_buf.len()) == EBADF_RET,
        &mut passed,
        &mut total,
    );
    record(
        "ssh_peer_ident stale handle returns EBADF",
        ssh_peer_ident_raw(0xfeed, core::ptr::null_mut(), 0) == EBADF_RET,
        &mut passed,
        &mut total,
    );
    let mut ident_buf = [0u8; 8];
    record(
        "ssh_peer_ident stale handle with buffer returns EBADF",
        ssh_peer_ident_raw(0xfeed, ident_buf.as_mut_ptr(), ident_buf.len()) == EBADF_RET,
        &mut passed,
        &mut total,
    );
    record(
        "ssh_close stale handle returns EBADF",
        ssh_close(0xfeed) == EBADF_RET,
        &mut passed,
        &mut total,
    );
    record(
        "ssh_auth_password stale handle returns EBADF",
        ssh_auth_password(0xfeed, "user", "pass") == EBADF_RET,
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
    expect(
        "ssh peer ident small buffer truncates",
        small_n as usize == small.len().min(full_len),
    );
    true
}

fn check_connected_zero_io(ssh_id: usize) -> bool {
    let mut ok = true;
    ok &= expect("ssh zero-length write", ssh_write(ssh_id, &[]) == 0);

    let mut empty = [0u8; 0];
    ok &= expect("ssh zero-length read", ssh_read(ssh_id, &mut empty) == 0);

    ok &= expect(
        "ssh raw write blocked after sunset kex",
        ssh_write(ssh_id, b"raw") == ENOTCONN_RET,
    );
    let mut raw_read = [0u8; 1];
    ok &= expect(
        "ssh raw read blocked after sunset kex",
        ssh_read(ssh_id, &mut raw_read) == ENOTCONN_RET,
    );

    let close_probe = ssh_close(ssh_id.wrapping_add(0x1000));
    ok &= expect(
        "ssh unrelated close returns EBADF",
        close_probe == EBADF_RET,
    );
    ok
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
