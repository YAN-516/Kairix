#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    close, connect, fcntl, read, socket, ssh_auth_password, ssh_channel_close,
    ssh_channel_try_read, ssh_channel_write, ssh_close, ssh_connect, ssh_shell, write, yield_,
};

const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const IPPROTO_TCP: i32 = 6;
const DEFAULT_IDENT: &str = "SSH-2.0-kairix-sshshell_0.1";
const STDIN: usize = 0;
const STDOUT: usize = 1;
const EAGAIN_RET: isize = -11;
const F_GETFL: usize = 3;
const F_SETFL: usize = 4;
const O_NONBLOCK: usize = 0o4000;
const PROMPT_HOST_MAX: usize = 15;

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
    if argc < 5 {
        println!("usage: sshshell <ipv4> <port> <username> <password>");
        println!("example: sshshell 10.0.2.2 22 user password");
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

    run(host, port, username, password)
}

fn run(host: u32, port: u16, username: &str, password: &str) -> i32 {
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

    let channel_id = ssh_shell(ssh_id);
    if channel_id < 0 {
        println!("ssh shell failed: {}", channel_id);
        let _ = ssh_close(ssh_id);
        let _ = close(fd);
        return -1;
    }
    let channel_id = channel_id as usize;
    println!("[ok] shell channel {}", channel_id);
    println!("[info] no PTY yet; type commands and use `exit` to close the shell");

    let old_stdin_flags = set_stdin_nonblock();
    let code = shell_loop(ssh_id, channel_id, username, host);
    restore_stdin_flags(old_stdin_flags);

    let _ = ssh_channel_close(ssh_id, channel_id);
    let _ = ssh_close(ssh_id);
    let _ = close(fd);
    code
}

fn shell_loop(ssh_id: usize, channel_id: usize, username: &str, host: u32) -> i32 {
    let mut out = [0u8; 1024];
    let mut input = [0u8; 128];
    let mut prompt_needed = true;

    loop {
        loop {
            let n = ssh_channel_try_read(ssh_id, channel_id, &mut out);
            if n == EAGAIN_RET {
                break;
            }
            if n < 0 {
                println!("\nssh channel read failed: {}", n);
                return -1;
            }
            if n == 0 {
                println!("\n[closed]");
                return 0;
            }
            write_all(STDOUT, &out[..n as usize]);
            prompt_needed = out[n as usize - 1] == b'\n';
        }

        if prompt_needed {
            print_prompt(username, host);
            prompt_needed = false;
        }

        loop {
            let n = read(STDIN, &mut input);
            if n == EAGAIN_RET {
                break;
            }
            if n < 0 {
                println!("\nstdin read failed: {}", n);
                return -1;
            }
            if n == 0 {
                break;
            }
            if input[..n as usize].contains(&4) {
                println!("\n[ctrl-d]");
                return 0;
            }
            write_all(STDOUT, &input[..n as usize]);
            if !write_channel_all(ssh_id, channel_id, &input[..n as usize]) {
                return -1;
            }
        }

        yield_();
    }
}

fn set_stdin_nonblock() -> Option<usize> {
    let flags = fcntl(STDIN, F_GETFL, 0);
    if flags >= 0 {
        let _ = fcntl(STDIN, F_SETFL, flags as usize | O_NONBLOCK);
        Some(flags as usize)
    } else {
        None
    }
}

fn restore_stdin_flags(flags: Option<usize>) {
    if let Some(flags) = flags {
        let _ = fcntl(STDIN, F_SETFL, flags);
    }
}

fn print_prompt(username: &str, host: u32) {
    print!("{}", username);
    print!("@");
    print_ipv4(host);
    print!("$ ");
}

fn print_ipv4(ip: u32) {
    let mut buf = [0u8; PROMPT_HOST_MAX];
    let mut pos = 0usize;
    for shift in [24u32, 16, 8, 0] {
        if pos > 0 {
            buf[pos] = b'.';
            pos += 1;
        }
        pos += write_decimal((ip >> shift) & 0xff, &mut buf[pos..]);
    }
    write_all(STDOUT, &buf[..pos]);
}

fn write_decimal(mut value: u32, out: &mut [u8]) -> usize {
    if value == 0 {
        out[0] = b'0';
        return 1;
    }
    let mut tmp = [0u8; 3];
    let mut len = 0usize;
    while value > 0 {
        tmp[len] = b'0' + (value % 10) as u8;
        value /= 10;
        len += 1;
    }
    for i in 0..len {
        out[i] = tmp[len - 1 - i];
    }
    len
}

fn write_channel_all(ssh_id: usize, channel_id: usize, mut buf: &[u8]) -> bool {
    while !buf.is_empty() {
        let n = ssh_channel_write(ssh_id, channel_id, buf);
        if n < 0 {
            println!("\nssh channel write failed: {}", n);
            return false;
        }
        if n == 0 {
            yield_();
            continue;
        }
        buf = &buf[n as usize..];
    }
    true
}

fn write_all(fd: usize, mut buf: &[u8]) {
    while !buf.is_empty() {
        let n = write(fd, buf);
        if n <= 0 {
            break;
        }
        buf = &buf[n as usize..];
    }
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
