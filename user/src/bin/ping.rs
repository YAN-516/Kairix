#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{bind, close, read, recvfrom, sendto, sleep, socket};

const AF_INET: i32 = 2;
const SOCK_DGRAM: i32 = 2;
const SOCK_RAW: i32 = 3;
const IPPROTO_ICMP: i32 = 1;
const DNS_PORT: u16 = 53;
const DNS_SERVER_1: u32 = 0xC0A84107; // 192.168.65.7 (Docker resolver)
const DNS_SERVER_2: u32 = 0x08080808; // 8.8.8.8
const DNS_SERVER_3: u32 = 0x01010101; // 1.1.1.1
const LOCAL_IP: u32 = 0x0A00020F; // 10.0.2.15
const DNS_SRC_PORT: u16 = 5353;

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
            sin_addr: ip,
            sin_zero: [0; 8],
        }
    }
}

#[unsafe(no_mangle)]
pub fn main_with_args(argc: usize, argv: *const usize) -> i32 {
    let mut line = [0u8; 64];
    let mut dns_override: Option<u32> = None;

    if argc > 2 {
        let arg2 = unsafe { *argv.add(2) as *const u8 };
        if arg2.is_null() {
            println!("invalid dns argument");
            return -1;
        }
        let dns_s = match cstr_to_str(arg2) {
            Some(v) => v,
            None => {
                println!("invalid dns argument utf8");
                return -1;
            }
        };
        dns_override = match parse_ipv4(dns_s) {
            Some(ip) => Some(ip),
            None => {
                println!("invalid dns server ip: {}", dns_s);
                return -1;
            }
        };
    }

    let s = if argc > 1 {
        let arg1 = unsafe { *argv.add(1) as *const u8 };
        if arg1.is_null() {
            println!("usage: ping <ipv4-or-domain> [dns-ip]");
            return -1;
        }
        match cstr_to_str(arg1) {
            Some(v) => v,
            None => {
                println!("invalid argument utf8");
                return -1;
            }
        }
    } else {
        println!("usage: ping <ipv4-or-domain> [dns-ip]");
        println!("no argument detected, fallback to interactive mode");
        print!("ping> ");
        let n = read_line(&mut line);
        if n == 0 {
            println!("empty input");
            return -1;
        }
        match core::str::from_utf8(&line[..n]) {
            Ok(v) => v.trim(),
            Err(_) => {
                println!("invalid utf8 input");
                return -1;
            }
        }
    };

    let dst = match parse_ipv4(s) {
        Some(ip) => ip,
        None => {
            println!("resolving domain {} ...", s);
            match resolve_domain_ipv4(s, dns_override) {
                Some(ip) => {
                    println!(
                        "resolved {} -> {}.{}.{}.{}",
                        s,
                        (ip >> 24) & 0xFF,
                        (ip >> 16) & 0xFF,
                        (ip >> 8) & 0xFF,
                        ip & 0xFF
                    );
                    ip
                }
                None => {
                    println!("dns resolve failed: {}", s);
                    return -1;
                }
            }
        }
    };

    println!("PING {}:", s);

    let mut ok = 0;
    for seq in 1..=4u16 {
        if ping_once(dst, 0x3344, seq, s) {
            ok += 1;
        }
        sleep(100);
    }

    println!("ping summary: {}/4 replies", ok);
    if ok > 0 { 0 } else { -1 }
}

fn cstr_to_str(ptr: *const u8) -> Option<&'static str> {
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

fn ping_once(dst_ip: u32, ident: u16, seq: u16, tag: &str) -> bool {
    let fd = socket(AF_INET, SOCK_RAW, IPPROTO_ICMP);
    if fd < 0 {
        println!("[ICMP:{}] socket failed: {}", tag, fd);
        return false;
    }

    let mut packet = [0u8; 64];
    packet[0] = 8;
    packet[1] = 0;
    packet[4] = (ident >> 8) as u8;
    packet[5] = (ident & 0xFF) as u8;
    packet[6] = (seq >> 8) as u8;
    packet[7] = (seq & 0xFF) as u8;

    let mut i = 8;
    while i < packet.len() {
        packet[i] = (i - 8) as u8;
        i += 1;
    }

    let csum = icmp_csum(&packet);
    packet[2] = (csum >> 8) as u8;
    packet[3] = (csum & 0xFF) as u8;

    let dst = SockAddrIn::new(dst_ip, 0);
    let send_ret = sendto(
        fd as usize,
        packet.as_ptr(),
        packet.len(),
        0,
        &dst as *const SockAddrIn as *const u8,
        core::mem::size_of::<SockAddrIn>(),
    );

    if send_ret < 0 {
        println!("[ICMP:{}] sendto failed: {}", tag, send_ret);
        let _ = close(fd as usize);
        return false;
    }

    let mut reply = [0u8; 128];
    let mut src_addr = SockAddrIn::new(0, 0);
    let mut src_len: usize;

    for _ in 0..200 {
        src_len = core::mem::size_of::<SockAddrIn>();
        let recv_ret = recvfrom(
            fd as usize,
            reply.as_mut_ptr(),
            reply.len(),
            0,
            &mut src_addr as *mut SockAddrIn as *mut u8,
            &mut src_len as *mut usize,
        );

        if recv_ret < 0 {
            sleep(1);
            continue;
        }

        let n = recv_ret as usize;
        if n < 8 {
            continue;
        }

        if reply[0] != 0 {
            continue;
        }

        if reply[4] == packet[4]
            && reply[5] == packet[5]
            && reply[6] == packet[6]
            && reply[7] == packet[7]
        {
            let _ = close(fd as usize);
            println!("[ICMP:{}] seq={} reply ok, {} bytes", tag, seq, recv_ret);
            return true;
        }
    }

    let _ = close(fd as usize);
    println!("[ICMP:{}] seq={} timeout", tag, seq);
    false
}

fn read_line(buf: &mut [u8]) -> usize {
    let mut n = 0usize;
    while n + 1 < buf.len() {
        let mut ch = [0u8; 1];
        let ret = read(0, &mut ch);
        if ret <= 0 {
            break;
        }
        print!("{}", ch[0] as char);
        if ch[0] == b'\n' || ch[0] == b'\r' {
            break;
        }
        buf[n] = ch[0];
        n += 1;
    }
    n
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

fn resolve_domain_ipv4(domain: &str, dns_override: Option<u32>) -> Option<u32> {
    let mut query = [0u8; 512];
    let tx_len = build_dns_query(domain, 0x3344, &mut query)?;

    if let Some(server) = dns_override {
        println!(
            "[DNS] using override server {}.{}.{}.{}",
            (server >> 24) & 0xFF,
            (server >> 16) & 0xFF,
            (server >> 8) & 0xFF,
            server & 0xFF
        );
        return resolve_via_dns_server(server, &query[..tx_len], 0x3344);
    }

    resolve_via_dns_server(DNS_SERVER_1, &query[..tx_len], 0x3344)
        .or_else(|| resolve_via_dns_server(DNS_SERVER_2, &query[..tx_len], 0x3344))
        .or_else(|| resolve_via_dns_server(DNS_SERVER_3, &query[..tx_len], 0x3344))
}

fn resolve_via_dns_server(server_ip: u32, query: &[u8], txid: u16) -> Option<u32> {
    let fd = socket(AF_INET, SOCK_DGRAM, 0);
    if fd < 0 {
        println!("[DNS] socket failed: {}", fd);
        return None;
    }

    let local = SockAddrIn::new(LOCAL_IP, DNS_SRC_PORT);
    let bind_ret = bind(
        fd as usize,
        &local as *const SockAddrIn as *const u8,
        core::mem::size_of::<SockAddrIn>(),
    );
    if bind_ret < 0 {
        println!("[DNS] bind failed: {}", bind_ret);
        let _ = close(fd as usize);
        return None;
    }

    let remote = SockAddrIn::new(server_ip, DNS_PORT);

    let mut send_ret = -1;
    for attempt in 0..50 {
        send_ret = sendto(
            fd as usize,
            query.as_ptr(),
            query.len(),
            0,
            &remote as *const SockAddrIn as *const u8,
            core::mem::size_of::<SockAddrIn>(),
        );
        if send_ret >= 0 {
            break;
        }
        if attempt == 0 {
            println!(
                "[DNS] send pending to {}.{}.{}.{}:{}, waiting for ARP",
                (server_ip >> 24) & 0xFF,
                (server_ip >> 16) & 0xFF,
                (server_ip >> 8) & 0xFF,
                server_ip & 0xFF,
                DNS_PORT
            );
        }
        sleep(1);
    }

    if send_ret < 0 {
        println!(
            "[DNS] sendto failed to {}.{}.{}.{}: {}",
            (server_ip >> 24) & 0xFF,
            (server_ip >> 16) & 0xFF,
            (server_ip >> 8) & 0xFF,
            server_ip & 0xFF,
            send_ret
        );
        let _ = close(fd as usize);
        return None;
    }

    let mut resp = [0u8; 512];
    let mut src = SockAddrIn::new(0, 0);

    println!(
        "[DNS] waiting response from {}.{}.{}.{}",
        (server_ip >> 24) & 0xFF,
        (server_ip >> 16) & 0xFF,
        (server_ip >> 8) & 0xFF,
        server_ip & 0xFF
    );

    for _ in 0..5 {
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
            let out = parse_dns_a_response(&resp, n as usize, txid);
            let _ = close(fd as usize);
            if out.is_some() {
                println!(
                    "[DNS] got answer from {}.{}.{}.{}",
                    (server_ip >> 24) & 0xFF,
                    (server_ip >> 16) & 0xFF,
                    (server_ip >> 8) & 0xFF,
                    server_ip & 0xFF
                );
            }
            return out;
        }

        sleep(1);
    }

    println!(
        "[DNS] timeout waiting for response from {}.{}.{}.{}",
        (server_ip >> 24) & 0xFF,
        (server_ip >> 16) & 0xFF,
        (server_ip >> 8) & 0xFF,
        server_ip & 0xFF
    );
    let _ = close(fd as usize);
    None
}

fn build_dns_query(domain: &str, txid: u16, out: &mut [u8]) -> Option<usize> {
    if out.len() < 18 {
        return None;
    }

    out[0] = (txid >> 8) as u8;
    out[1] = (txid & 0xFF) as u8;
    out[2] = 0x01; // recursion desired
    out[3] = 0x00;
    out[4] = 0x00;
    out[5] = 0x01; // qdcount=1
    out[6] = 0x00;
    out[7] = 0x00;
    out[8] = 0x00;
    out[9] = 0x00;
    out[10] = 0x00;
    out[11] = 0x00;

    let mut p = 12usize;
    for label in domain.split('.') {
        let len = label.len();
        if len == 0 || len > 63 || p + 1 + len >= out.len() {
            return None;
        }
        out[p] = len as u8;
        p += 1;
        for &b in label.as_bytes() {
            out[p] = b;
            p += 1;
        }
    }

    if p + 5 > out.len() {
        return None;
    }
    out[p] = 0;
    p += 1;

    out[p] = 0x00;
    out[p + 1] = 0x01; // QTYPE=A
    out[p + 2] = 0x00;
    out[p + 3] = 0x01; // QCLASS=IN
    p += 4;
    Some(p)
}

fn parse_dns_a_response(buf: &[u8], len: usize, txid: u16) -> Option<u32> {
    if len < 12 {
        return None;
    }

    let id = ((buf[0] as u16) << 8) | (buf[1] as u16);
    if id != txid {
        return None;
    }

    let flags = ((buf[2] as u16) << 8) | (buf[3] as u16);
    let qr = (flags & 0x8000) != 0;
    let rcode = flags & 0x000F;
    if !qr || rcode != 0 {
        return None;
    }

    let qdcount = ((buf[4] as u16) << 8) | (buf[5] as u16);
    let ancount = ((buf[6] as u16) << 8) | (buf[7] as u16);
    if qdcount == 0 || ancount == 0 {
        return None;
    }

    let mut p = 12usize;

    for _ in 0..qdcount {
        p = skip_dns_name(buf, len, p)?;
        if p + 4 > len {
            return None;
        }
        p += 4;
    }

    for _ in 0..ancount {
        p = skip_dns_name(buf, len, p)?;
        if p + 10 > len {
            return None;
        }

        let typ = ((buf[p] as u16) << 8) | (buf[p + 1] as u16);
        let class = ((buf[p + 2] as u16) << 8) | (buf[p + 3] as u16);
        let rdlen = ((buf[p + 8] as u16) << 8) | (buf[p + 9] as u16);
        p += 10;

        if p + rdlen as usize > len {
            return None;
        }

        if typ == 1 && class == 1 && rdlen == 4 {
            let ip = ((buf[p] as u32) << 24)
                | ((buf[p + 1] as u32) << 16)
                | ((buf[p + 2] as u32) << 8)
                | (buf[p + 3] as u32);
            return Some(ip);
        }

        p += rdlen as usize;
    }

    None
}

fn skip_dns_name(buf: &[u8], len: usize, mut p: usize) -> Option<usize> {
    if p >= len {
        return None;
    }

    loop {
        if p >= len {
            return None;
        }
        let b = buf[p];
        if b & 0xC0 == 0xC0 {
            if p + 1 >= len {
                return None;
            }
            return Some(p + 2);
        }
        if b == 0 {
            return Some(p + 1);
        }
        let l = b as usize;
        p += 1;
        if p + l > len {
            return None;
        }
        p += l;
    }
}

fn icmp_csum(data: &[u8]) -> u16 {
    let mut sum = 0u32;
    let mut i = 0;

    while i + 1 < data.len() {
        let word = ((data[i] as u16) << 8) | data[i + 1] as u16;
        sum += word as u32;
        if sum >> 16 != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
        i += 2;
    }

    if i < data.len() {
        sum += (data[i] as u32) << 8;
        if sum >> 16 != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
    }

    !(sum as u16)
}
