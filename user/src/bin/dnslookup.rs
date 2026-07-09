#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{bind, close, recvfrom, sendto, sleep, socket};

const AF_INET: i32 = 2;
const SOCK_DGRAM: i32 = 2;
const DNS_PORT: u16 = 53;
const DEFAULT_DNS: u32 = 0x0A000203; // 10.0.2.3
const LOCAL_IP: u32 = 0x0A00020F; // 10.0.2.15
const DNS_SRC_PORT: u16 = 5353;
const TXID: u16 = 0x3344;

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
        println!("usage: dnslookup <domain> [dns-ip]");
        return -1;
    }
    let domain = match cstr_to_str(unsafe { *argv.add(1) as *const u8 }) {
        Some(v) => v,
        None => {
            println!("invalid domain");
            return -1;
        }
    };
    let dns = if argc > 2 {
        match cstr_to_str(unsafe { *argv.add(2) as *const u8 }).and_then(parse_ipv4) {
            Some(v) => v,
            None => {
                println!("invalid dns server");
                return -1;
            }
        }
    } else {
        DEFAULT_DNS
    };

    match resolve(domain, dns) {
        Some(ip) => {
            println!(
                "{} -> {}.{}.{}.{}",
                domain,
                (ip >> 24) & 0xff,
                (ip >> 16) & 0xff,
                (ip >> 8) & 0xff,
                ip & 0xff
            );
            0
        }
        None => {
            println!("dns lookup failed");
            -1
        }
    }
}

fn resolve(domain: &str, dns: u32) -> Option<u32> {
    let mut query = [0u8; 512];
    let qlen = build_query(domain, &mut query)?;
    let fd = socket(AF_INET, SOCK_DGRAM, 0);
    if fd < 0 {
        println!("socket failed: {}", fd);
        return None;
    }

    let local = SockAddrIn::new(LOCAL_IP, DNS_SRC_PORT);
    let ret = bind(
        fd as usize,
        &local as *const SockAddrIn as *const u8,
        core::mem::size_of::<SockAddrIn>(),
    );
    if ret < 0 {
        println!("bind failed: {}", ret);
        let _ = close(fd as usize);
        return None;
    }

    let remote = SockAddrIn::new(dns, DNS_PORT);
    println!(
        "query {} via {}.{}.{}.{}",
        domain,
        (dns >> 24) & 0xff,
        (dns >> 16) & 0xff,
        (dns >> 8) & 0xff,
        dns & 0xff
    );
    let ret = sendto(
        fd as usize,
        query.as_ptr(),
        qlen,
        0,
        &remote as *const SockAddrIn as *const u8,
        core::mem::size_of::<SockAddrIn>(),
    );
    if ret < 0 {
        println!("sendto failed: {}", ret);
        let _ = close(fd as usize);
        return None;
    }

    let mut resp = [0u8; 512];
    let mut src = SockAddrIn::new(0, 0);
    for _ in 0..20 {
        let mut src_len = core::mem::size_of::<SockAddrIn>();
        let n = recvfrom(
            fd as usize,
            resp.as_mut_ptr(),
            resp.len(),
            0,
            &mut src as *mut SockAddrIn as *mut u8,
            &mut src_len as *mut usize,
        );
        if n > 0 {
            let out = parse_response(&resp, n as usize);
            let _ = close(fd as usize);
            return out;
        }
        sleep(10);
    }

    println!("recv timeout");
    let _ = close(fd as usize);
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
    out[p + 1] = 1;
    out[p + 2] = 0;
    out[p + 3] = 1;
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

fn parse_ipv4(s: &str) -> Option<u32> {
    let mut out = 0u32;
    let mut cnt = 0usize;
    for part in s.split('.') {
        if cnt >= 4 || part.is_empty() {
            return None;
        }
        let v = part.parse::<u8>().ok()? as u32;
        out = (out << 8) | v;
        cnt += 1;
    }
    if cnt == 4 { Some(out) } else { None }
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
    core::str::from_utf8(unsafe { core::slice::from_raw_parts(ptr, len) }).ok()
}
