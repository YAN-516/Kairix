#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{close, connect, recvfrom, sendto, sleep, socket};

const AF_INET: i32 = 2;
const SOCK_DGRAM: i32 = 2;
const SOCK_STREAM: i32 = 1;
const IPPROTO_TCP: i32 = 6;
const DNS_PORT: u16 = 53;
const HTTP_PORT: u16 = 80;
const DEFAULT_DNS: u32 = 0x0A000203; // 10.0.2.3, QEMU user-mode DNS
const TXID: u16 = 0x4854; // "HT"

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
    if argc < 2 {
        println!("usage: httpget <host-or-url> [path] [dns-ip]");
        println!("example: httpget example.com /");
        println!("no argv detected, fallback to: httpget example.com /");
    }

    let target = if argc < 2 {
        "example.com"
    } else {
        match cstr_to_str(unsafe { *argv.add(1) as *const u8 }) {
            Some(v) => v,
            None => {
                println!("invalid target");
                return -1;
            }
        }
    };

    let mut host = target;
    let mut path = "/";
    if let Some(rest) = strip_prefix(host, "http://") {
        host = rest;
    }
    if let Some(pos) = find_byte(host, b'/') {
        path = &host[pos..];
        host = &host[..pos];
    }

    if argc > 2 {
        path = match cstr_to_str(unsafe { *argv.add(2) as *const u8 }) {
            Some(v) if !v.is_empty() => v,
            _ => {
                println!("invalid path");
                return -1;
            }
        };
    }

    let dns = if argc > 3 {
        match cstr_to_str(unsafe { *argv.add(3) as *const u8 }).and_then(parse_ipv4) {
            Some(v) => v,
            None => {
                println!("invalid dns server");
                return -1;
            }
        }
    } else {
        DEFAULT_DNS
    };

    if host.is_empty() {
        println!("empty host");
        return -1;
    }

    let ip = match parse_ipv4(host) {
        Some(ip) => ip,
        None => {
            println!("resolving {} ...", host);
            match resolve(host, dns) {
                Some(ip) => ip,
                None => {
                    println!("dns lookup failed");
                    return -1;
                }
            }
        }
    };

    println!(
        "connecting {} ({}.{}.{}.{}){}",
        host,
        (ip >> 24) & 0xff,
        (ip >> 16) & 0xff,
        (ip >> 8) & 0xff,
        ip & 0xff,
        path
    );

    http_get(host, path, ip)
}

fn http_get(host: &str, path: &str, ip: u32) -> i32 {
    let fd = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if fd < 0 {
        println!("socket failed: {}", fd);
        return -1;
    }
    let fd = fd as usize;

    let addr = SockAddrIn::new(ip, HTTP_PORT);
    let ret = connect(
        fd,
        &addr as *const SockAddrIn as *const u8,
        core::mem::size_of::<SockAddrIn>(),
    );
    if ret < 0 {
        println!("connect failed: {}", ret);
        let _ = close(fd);
        return -1;
    }

    let mut req = [0u8; 512];
    let mut n = 0usize;
    if !append(&mut req, &mut n, b"GET ")
        || !append(&mut req, &mut n, path.as_bytes())
        || !append(&mut req, &mut n, b" HTTP/1.0\r\nHost: ")
        || !append(&mut req, &mut n, host.as_bytes())
        || !append(
            &mut req,
            &mut n,
            b"\r\nConnection: close\r\nUser-Agent: kairix-httpget/0.1\r\n\r\n",
        )
    {
        println!("request too long");
        let _ = close(fd);
        return -1;
    }

    let ret = sendto(fd, req.as_ptr(), n, 0, core::ptr::null(), 0);
    if ret < 0 {
        println!("send failed: {}", ret);
        let _ = close(fd);
        return -1;
    }

    println!("--- response begin ---");
    let mut buf = [0u8; 1024];
    loop {
        let got = recvfrom(
            fd,
            buf.as_mut_ptr(),
            buf.len(),
            0,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        );
        if got < 0 {
            println!("\nrecv failed: {}", got);
            let _ = close(fd);
            return -1;
        }
        if got == 0 {
            break;
        }
        print_bytes(&buf[..got as usize]);
    }
    println!("\n--- response end ---");
    let _ = close(fd);
    0
}

fn resolve(domain: &str, dns: u32) -> Option<u32> {
    let mut query = [0u8; 512];
    let qlen = build_query(domain, &mut query)?;
    let fd = socket(AF_INET, SOCK_DGRAM, 0);
    if fd < 0 {
        println!("dns socket failed: {}", fd);
        return None;
    }
    let fd = fd as usize;

    let remote = SockAddrIn::new(dns, DNS_PORT);
    let ret = sendto(
        fd,
        query.as_ptr(),
        qlen,
        0,
        &remote as *const SockAddrIn as *const u8,
        core::mem::size_of::<SockAddrIn>(),
    );
    if ret < 0 {
        println!("dns send failed: {}", ret);
        let _ = close(fd);
        return None;
    }

    let mut resp = [0u8; 512];
    for _ in 0..30 {
        let n = recvfrom(
            fd,
            resp.as_mut_ptr(),
            resp.len(),
            0,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        );
        if n > 0 {
            let out = parse_response(&resp, n as usize);
            let _ = close(fd);
            return out;
        }
        sleep(10);
    }

    println!("dns recv timeout");
    let _ = close(fd);
    None
}

fn build_query(domain: &str, out: &mut [u8]) -> Option<usize> {
    out[0] = (TXID >> 8) as u8;
    out[1] = TXID as u8;
    out[2] = 0x01;
    out[3] = 0x00;
    out[4] = 0x00;
    out[5] = 0x01;
    out[6] = 0x00;
    out[7] = 0x00;
    out[8] = 0x00;
    out[9] = 0x00;
    out[10] = 0x00;
    out[11] = 0x00;

    let mut p = 12usize;
    for label in domain.trim_end_matches('.').split('.') {
        if label.is_empty() || label.len() > 63 || p + 1 + label.len() >= out.len() {
            return None;
        }
        out[p] = label.len() as u8;
        p += 1;
        for b in label.bytes() {
            out[p] = b;
            p += 1;
        }
    }
    if p + 5 > out.len() {
        return None;
    }
    out[p] = 0;
    p += 1;
    out[p] = 0;
    out[p + 1] = 1; // A
    out[p + 2] = 0;
    out[p + 3] = 1; // IN
    Some(p + 4)
}

fn parse_response(buf: &[u8], len: usize) -> Option<u32> {
    if len < 12 || buf[0] != (TXID >> 8) as u8 || buf[1] != TXID as u8 {
        return None;
    }
    let flags = ((buf[2] as u16) << 8) | buf[3] as u16;
    if flags & 0x8000 == 0 || flags & 0x000f != 0 {
        return None;
    }
    let qd = ((buf[4] as u16) << 8) | buf[5] as u16;
    let an = ((buf[6] as u16) << 8) | buf[7] as u16;
    let mut p = 12usize;
    for _ in 0..qd {
        p = skip_name(buf, len, p)?;
        if p + 4 > len {
            return None;
        }
        p += 4;
    }
    for _ in 0..an {
        p = skip_name(buf, len, p)?;
        if p + 10 > len {
            return None;
        }
        let typ = ((buf[p] as u16) << 8) | buf[p + 1] as u16;
        let class = ((buf[p + 2] as u16) << 8) | buf[p + 3] as u16;
        let rdlen = ((buf[p + 8] as u16) << 8) | buf[p + 9] as u16;
        p += 10;
        if p + rdlen as usize > len {
            return None;
        }
        if typ == 1 && class == 1 && rdlen == 4 {
            return Some(
                ((buf[p] as u32) << 24)
                    | ((buf[p + 1] as u32) << 16)
                    | ((buf[p + 2] as u32) << 8)
                    | buf[p + 3] as u32,
            );
        }
        p += rdlen as usize;
    }
    None
}

fn skip_name(buf: &[u8], len: usize, mut p: usize) -> Option<usize> {
    loop {
        if p >= len {
            return None;
        }
        let b = buf[p];
        if b & 0xc0 == 0xc0 {
            return if p + 1 < len { Some(p + 2) } else { None };
        }
        p += 1;
        if b == 0 {
            return Some(p);
        }
        if b & 0xc0 != 0 {
            return None;
        }
        p = p.checked_add(b as usize)?;
        if p > len {
            return None;
        }
    }
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
        if len > 255 {
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

fn append(dst: &mut [u8], pos: &mut usize, src: &[u8]) -> bool {
    if *pos + src.len() > dst.len() {
        return false;
    }
    dst[*pos..*pos + src.len()].copy_from_slice(src);
    *pos += src.len();
    true
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

fn strip_prefix<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let bytes = s.as_bytes();
    let prefix = prefix.as_bytes();
    if bytes.len() < prefix.len() {
        return None;
    }
    if &bytes[..prefix.len()] == prefix {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

fn find_byte(s: &str, needle: u8) -> Option<usize> {
    for (idx, b) in s.bytes().enumerate() {
        if b == needle {
            return Some(idx);
        }
    }
    None
}
