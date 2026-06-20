#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{close, connect, recvfrom, sendto, socket};

const AF_INET: i32 = 2;
const SOCK_DGRAM: i32 = 2;
const SOCK_STREAM: i32 = 1;
const IPPROTO_TCP: i32 = 6;
const DNS_PORT: u16 = 53;
const HTTP_PORT: u16 = 80;
const DEFAULT_DNS: u32 = 0x0A000203;
const TXID_BASE: u16 = 0x4e54; // "NT"
const READ_LIMIT: usize = 128 * 1024;
const MAX_DNS_A: usize = 4;
const HEADER_LIMIT: usize = 4096;

struct HttpCase {
    host: &'static str,
    path: &'static str,
    min_bytes: usize,
    repeats: usize,
}

const CASES: &[HttpCase] = &[
    HttpCase {
        host: "httpforever.com",
        path: "/",
        min_bytes: 3000,
        repeats: 2,
    },
    HttpCase {
        host: "example.com",
        path: "/",
        min_bytes: 200,
        repeats: 1,
    },
    HttpCase {
        host: "www.example.com",
        path: "/",
        min_bytes: 200,
        repeats: 1,
    },
];

#[derive(Clone, Copy)]
struct IpList {
    ips: [u32; MAX_DNS_A],
    len: usize,
}

impl IpList {
    fn new() -> Self {
        Self {
            ips: [0; MAX_DNS_A],
            len: 0,
        }
    }

    fn push(&mut self, ip: u32) {
        if self.len >= self.ips.len() {
            return;
        }
        if self.ips[..self.len].contains(&ip) {
            return;
        }
        self.ips[self.len] = ip;
        self.len += 1;
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }
}

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
pub fn main() -> i32 {
    println!("[HTTP-NETTEST] start");
    let mut passed = 0usize;
    let mut total = 0usize;
    let mut txid = TXID_BASE;

    for case in CASES {
        let ips = match resolve(case.host, DEFAULT_DNS, txid) {
            Some(ips) => ips,
            None => {
                println!("[HTTP-NETTEST] resolve failed: {}", case.host);
                total += case.repeats;
                txid = txid.wrapping_add(1);
                continue;
            }
        };
        txid = txid.wrapping_add(1);

        for run in 0..case.repeats {
            total += 1;
            let ok = fetch_with_retries(case.host, case.path, &ips, run, case.min_bytes);
            if ok {
                passed += 1;
            }
            println!(
                "[HTTP-NETTEST] {}{} run {} {}",
                case.host,
                case.path,
                run + 1,
                if ok { "PASS" } else { "FAIL" }
            );
        }
    }

    println!("[HTTP-NETTEST] summary: {}/{} passed", passed, total);
    if passed == total { 0 } else { -1 }
}

fn fetch_with_retries(host: &str, path: &str, ips: &IpList, run: usize, min_bytes: usize) -> bool {
    let mut tried = 0usize;
    while tried < ips.len {
        let idx = (run + tried) % ips.len;
        let ip = ips.ips[idx];
        println!(
            "[HTTP-NETTEST] trying {} via {}.{}.{}.{}",
            host,
            (ip >> 24) & 0xff,
            (ip >> 16) & 0xff,
            (ip >> 8) & 0xff,
            ip & 0xff
        );
        if fetch_once(host, path, ip, min_bytes) {
            return true;
        }
        tried += 1;
    }
    false
}

fn fetch_once(host: &str, path: &str, ip: u32, min_bytes: usize) -> bool {
    let fd = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if fd < 0 {
        println!("[HTTP-NETTEST] socket failed: {}", fd);
        return false;
    }
    let fd = fd as usize;

    let addr = SockAddrIn::new(ip, HTTP_PORT);
    let ret = connect(
        fd,
        &addr as *const SockAddrIn as *const u8,
        core::mem::size_of::<SockAddrIn>(),
    );
    if ret < 0 {
        println!(
            "[HTTP-NETTEST] connect failed {} via {}.{}.{}.{}: {}",
            host,
            (ip >> 24) & 0xff,
            (ip >> 16) & 0xff,
            (ip >> 8) & 0xff,
            ip & 0xff,
            ret
        );
        let _ = close(fd);
        return false;
    }

    let mut req = [0u8; 768];
    let mut req_len = 0usize;
    if !append(&mut req, &mut req_len, b"GET ")
        || !append(&mut req, &mut req_len, path.as_bytes())
        || !append(&mut req, &mut req_len, b" HTTP/1.1\r\nHost: ")
        || !append(&mut req, &mut req_len, host.as_bytes())
        || !append(&mut req, &mut req_len, b"\r\nConnection: close\r\nAccept: */*\r\nUser-Agent: kairix-http-nettest/0.1\r\n\r\n")
    {
        println!("[HTTP-NETTEST] request too long");
        let _ = close(fd);
        return false;
    }

    let ret = sendto(fd, req.as_ptr(), req_len, 0, core::ptr::null(), 0);
    if ret < 0 {
        println!("[HTTP-NETTEST] send failed {}: {}", host, ret);
        let _ = close(fd);
        return false;
    }

    let mut buf = [0u8; 1460];
    let mut header = [0u8; HEADER_LIMIT];
    let mut header_len = 0usize;
    let mut total = 0usize;
    let mut body = 0usize;
    let mut header_done = false;
    let mut status_ok = false;
    let mut content_length = None;

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
            println!("[HTTP-NETTEST] recv failed {}: {}", host, got);
            let _ = close(fd);
            return false;
        }
        if got == 0 {
            break;
        }
        let n = got as usize;

        if header_done {
            body += n;
        } else {
            let copy = core::cmp::min(n, header.len().saturating_sub(header_len));
            if copy > 0 {
                header[header_len..header_len + copy].copy_from_slice(&buf[..copy]);
                header_len += copy;
            }
            if let Some(end) = find_header_end(&header[..header_len]) {
                header_done = true;
                status_ok = is_http_success(&header[..end]);
                content_length = parse_content_length(&header[..end]);
                body = header_len.saturating_sub(end + 4);
            } else if header_len == header.len() {
                println!("[HTTP-NETTEST] header too large {}", host);
                let _ = close(fd);
                return false;
            }
        }

        total += n;
        if let Some(want) = content_length {
            if body >= want {
                break;
            }
        }
        if total > READ_LIMIT {
            break;
        }
    }

    let _ = close(fd);
    println!(
        "[HTTP-NETTEST] {}{} bytes={} body={} status_ok={} complete={}",
        host, path, total, body, status_ok, content_complete(content_length, body)
    );
    status_ok && total >= min_bytes && content_complete(content_length, body)
}

fn resolve(domain: &str, dns: u32, txid: u16) -> Option<IpList> {
    let mut query = [0u8; 512];
    let qlen = build_query(domain, txid, &mut query)?;
    let fd = socket(AF_INET, SOCK_DGRAM, 0);
    if fd < 0 {
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
        let _ = close(fd);
        return None;
    }

    let mut resp = [0u8; 512];
    let n = recvfrom(
        fd,
        resp.as_mut_ptr(),
        resp.len(),
        0,
        core::ptr::null_mut(),
        core::ptr::null_mut(),
    );
    let _ = close(fd);
    if n <= 0 {
        return None;
    }
    let ips = parse_response(&resp, n as usize, txid)?;
    if ips.is_empty() {
        None
    } else {
        print!("[HTTP-NETTEST] {} A:", domain);
        let mut i = 0usize;
        while i < ips.len {
            let ip = ips.ips[i];
            print!(
                " {}.{}.{}.{}",
                (ip >> 24) & 0xff,
                (ip >> 16) & 0xff,
                (ip >> 8) & 0xff,
                ip & 0xff
            );
            i += 1;
        }
        println!("");
        Some(ips)
    }
}

fn build_query(domain: &str, txid: u16, out: &mut [u8]) -> Option<usize> {
    out[0] = (txid >> 8) as u8;
    out[1] = txid as u8;
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
    out[p + 1] = 1;
    out[p + 2] = 0;
    out[p + 3] = 1;
    Some(p + 4)
}

fn parse_response(buf: &[u8], len: usize, txid: u16) -> Option<IpList> {
    if len < 12 || buf[0] != (txid >> 8) as u8 || buf[1] != txid as u8 {
        return None;
    }
    let flags = ((buf[2] as u16) << 8) | buf[3] as u16;
    if flags & 0x8000 == 0 || flags & 0x000f != 0 {
        return None;
    }
    let qd = ((buf[4] as u16) << 8) | buf[5] as u16;
    let an = ((buf[6] as u16) << 8) | buf[7] as u16;
    let mut ips = IpList::new();
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
            ips.push(
                ((buf[p] as u32) << 24)
                    | ((buf[p + 1] as u32) << 16)
                    | ((buf[p + 2] as u32) << 8)
                    | buf[p + 3] as u32,
            );
        }
        p += rdlen as usize;
    }
    Some(ips)
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

fn append(dst: &mut [u8], pos: &mut usize, src: &[u8]) -> bool {
    if *pos + src.len() > dst.len() {
        return false;
    }
    dst[*pos..*pos + src.len()].copy_from_slice(src);
    *pos += src.len();
    true
}

fn is_http_success(buf: &[u8]) -> bool {
    starts_with(buf, b"HTTP/") && contains(buf, b" 200 ")
}

fn starts_with(buf: &[u8], prefix: &[u8]) -> bool {
    buf.len() >= prefix.len() && &buf[..prefix.len()] == prefix
}

fn contains(buf: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || buf.len() < needle.len() {
        return false;
    }
    let mut i = 0usize;
    while i + needle.len() <= buf.len() {
        if &buf[i..i + needle.len()] == needle {
            return true;
        }
        i += 1;
    }
    false
}

fn content_complete(content_length: Option<usize>, body: usize) -> bool {
    match content_length {
        Some(want) => body >= want,
        None => body > 0,
    }
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    let mut i = 0usize;
    while i + 4 <= buf.len() {
        if &buf[i..i + 4] == b"\r\n\r\n" {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn parse_content_length(header: &[u8]) -> Option<usize> {
    let mut line_start = 0usize;
    while line_start < header.len() {
        let mut line_end = line_start;
        while line_end + 1 < header.len()
            && !(header[line_end] == b'\r' && header[line_end + 1] == b'\n')
        {
            line_end += 1;
        }
        let line = &header[line_start..line_end];
        if starts_with_ci(line, b"content-length:") {
            let mut p = b"content-length:".len();
            while p < line.len() && (line[p] == b' ' || line[p] == b'\t') {
                p += 1;
            }
            return parse_usize(&line[p..]);
        }
        if line_end + 2 > header.len() {
            break;
        }
        line_start = line_end + 2;
    }
    None
}

fn starts_with_ci(buf: &[u8], prefix: &[u8]) -> bool {
    if buf.len() < prefix.len() {
        return false;
    }
    let mut i = 0usize;
    while i < prefix.len() {
        if lower(buf[i]) != lower(prefix[i]) {
            return false;
        }
        i += 1;
    }
    true
}

fn lower(b: u8) -> u8 {
    if b >= b'A' && b <= b'Z' {
        b + 32
    } else {
        b
    }
}

fn parse_usize(buf: &[u8]) -> Option<usize> {
    let mut n = 0usize;
    let mut seen = false;
    for &b in buf {
        if b < b'0' || b > b'9' {
            break;
        }
        seen = true;
        n = n.checked_mul(10)?.checked_add((b - b'0') as usize)?;
    }
    if seen { Some(n) } else { None }
}
