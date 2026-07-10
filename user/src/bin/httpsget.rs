#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    close, connect, get_time, recvfrom, sendto, sleep, socket, tls_close, tls_connect, tls_read,
    tls_write,
};

const AF_INET: i32 = 2;
const SOCK_DGRAM: i32 = 2;
const SOCK_STREAM: i32 = 1;
const IPPROTO_TCP: i32 = 6;
const DNS_PORT: u16 = 53;
const HTTPS_PORT: u16 = 443;
const DEFAULT_DNS: u32 = 0x0A000203; // 10.0.2.3, QEMU user-mode DNS
const TXID: u16 = 0x4853; // "HS"
const DEFAULT_RESPONSE_PREVIEW_LIMIT: usize = 1024;
const REQUEST_BUF_SIZE: usize = 2048;
const READ_BUF_SIZE: usize = 1024;
const MAX_HEADERS: usize = 8;
const MAX_POSITIONALS: usize = 3;
const MAX_ARG_LEN: usize = 512;

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

struct Config {
    target: Option<&'static str>,
    path: Option<&'static str>,
    dns: u32,
    ip_override: Option<u32>,
    port: Option<u16>,
    connect_host: Option<&'static str>,
    host_header: Option<&'static str>,
    method: &'static str,
    http11: bool,
    headers_only: bool,
    quiet: bool,
    verbose: bool,
    fail_on_http_error: bool,
    max_preview: usize,
    headers: [&'static str; MAX_HEADERS],
    header_count: usize,
}

impl Config {
    fn new() -> Self {
        Self {
            target: None,
            path: None,
            dns: DEFAULT_DNS,
            ip_override: None,
            port: None,
            connect_host: None,
            host_header: None,
            method: "GET",
            http11: true,
            headers_only: false,
            quiet: false,
            verbose: false,
            fail_on_http_error: false,
            max_preview: DEFAULT_RESPONSE_PREVIEW_LIMIT,
            headers: [""; MAX_HEADERS],
            header_count: 0,
        }
    }

    fn add_header(&mut self, value: &'static str) -> bool {
        if self.header_count >= MAX_HEADERS || !valid_header(value) {
            return false;
        }
        self.headers[self.header_count] = value;
        self.header_count += 1;
        true
    }
}

struct Request {
    host: &'static str,
    tls_host: &'static str,
    host_header: &'static str,
    path: &'static str,
    port: u16,
    ip: u32,
}

enum ArgResult {
    Ok,
    Help,
    Error,
}

#[unsafe(no_mangle)]
pub fn main_with_args(argc: usize, argv: *const usize) -> i32 {
    let mut cfg = Config::new();
    match parse_args(argc, argv, &mut cfg) {
        ArgResult::Help => {
            print_usage();
            return 0;
        }
        ArgResult::Error => return -1,
        ArgResult::Ok => {}
    }

    if argc < 2 {
        print_usage();
        println!("no argv detected, fallback to: httpsget example.com /");
    }

    if cfg.headers_only && cfg.method == "GET" {
        cfg.method = "HEAD";
    }

    let request = match prepare_request(&cfg) {
        Some(v) => v,
        None => return -1,
    };

    if !cfg.quiet {
        println!(
            "connecting {} ({}.{}.{}.{}){}",
            request.host,
            (request.ip >> 24) & 0xff,
            (request.ip >> 16) & 0xff,
            (request.ip >> 8) & 0xff,
            request.ip & 0xff,
            request.path
        );
    }

    https_get(&cfg, &request)
}

fn parse_args(argc: usize, argv: *const usize, cfg: &mut Config) -> ArgResult {
    let mut positionals = [""; MAX_POSITIONALS];
    let mut positional_count = 0usize;
    let mut i = 1usize;

    while i < argc {
        let arg = match argv_str(argv, i) {
            Some(v) => v,
            None => {
                println!("invalid argument");
                return ArgResult::Error;
            }
        };

        if arg == "-h" || arg == "--help" {
            return ArgResult::Help;
        } else if arg == "-I" || arg == "--head" || arg == "--headers-only" {
            cfg.headers_only = true;
        } else if arg == "-q" || arg == "--quiet" {
            cfg.quiet = true;
        } else if arg == "-v" || arg == "--verbose" {
            cfg.verbose = true;
        } else if arg == "-f" || arg == "--fail" {
            cfg.fail_on_http_error = true;
        } else if arg == "--http10" {
            cfg.http11 = false;
        } else if arg == "--http11" {
            cfg.http11 = true;
        } else if arg == "-X" || arg == "--method" {
            cfg.method = match next_arg(argc, argv, &mut i, "method") {
                Some(v) if valid_token(v) => v,
                _ => {
                    println!("invalid method");
                    return ArgResult::Error;
                }
            };
        } else if let Some(v) = strip_prefix(arg, "--method=") {
            if !valid_token(v) {
                println!("invalid method");
                return ArgResult::Error;
            }
            cfg.method = v;
        } else if arg == "-H" || arg == "--header" {
            let header = match next_arg(argc, argv, &mut i, "header") {
                Some(v) => v,
                None => {
                    println!("missing header");
                    return ArgResult::Error;
                }
            };
            if !cfg.add_header(header) {
                println!("invalid header or too many headers");
                return ArgResult::Error;
            }
        } else if let Some(v) = strip_prefix(arg, "--header=") {
            if !cfg.add_header(v) {
                println!("invalid header or too many headers");
                return ArgResult::Error;
            }
        } else if arg == "-d" || arg == "--dns" {
            cfg.dns = match next_arg(argc, argv, &mut i, "dns").and_then(parse_ipv4) {
                Some(v) => v,
                None => {
                    println!("invalid dns server");
                    return ArgResult::Error;
                }
            };
        } else if let Some(v) = strip_prefix(arg, "--dns=") {
            cfg.dns = match parse_ipv4(v) {
                Some(v) => v,
                None => {
                    println!("invalid dns server");
                    return ArgResult::Error;
                }
            };
        } else if arg == "--ip" {
            cfg.ip_override = match next_arg(argc, argv, &mut i, "ip").and_then(parse_ipv4) {
                Some(v) => Some(v),
                None => {
                    println!("invalid ip");
                    return ArgResult::Error;
                }
            };
        } else if let Some(v) = strip_prefix(arg, "--ip=") {
            cfg.ip_override = match parse_ipv4(v) {
                Some(v) => Some(v),
                None => {
                    println!("invalid ip");
                    return ArgResult::Error;
                }
            };
        } else if arg == "-p" || arg == "--port" {
            cfg.port = match next_arg(argc, argv, &mut i, "port").and_then(parse_port) {
                Some(v) => Some(v),
                None => {
                    println!("invalid port");
                    return ArgResult::Error;
                }
            };
        } else if let Some(v) = strip_prefix(arg, "--port=") {
            cfg.port = match parse_port(v) {
                Some(v) => Some(v),
                None => {
                    println!("invalid port");
                    return ArgResult::Error;
                }
            };
        } else if arg == "--path" {
            cfg.path = match next_arg(argc, argv, &mut i, "path") {
                Some(v) if !v.is_empty() => Some(v),
                _ => {
                    println!("invalid path");
                    return ArgResult::Error;
                }
            };
        } else if let Some(v) = strip_prefix(arg, "--path=") {
            if v.is_empty() {
                println!("invalid path");
                return ArgResult::Error;
            }
            cfg.path = Some(v);
        } else if arg == "--host" {
            cfg.host_header = match next_arg(argc, argv, &mut i, "host") {
                Some(v) if valid_host_value(v) => Some(v),
                _ => {
                    println!("invalid host header");
                    return ArgResult::Error;
                }
            };
        } else if let Some(v) = strip_prefix(arg, "--host=") {
            if !valid_host_value(v) {
                println!("invalid host header");
                return ArgResult::Error;
            }
            cfg.host_header = Some(v);
        } else if arg == "--sni" {
            cfg.connect_host = match next_arg(argc, argv, &mut i, "sni") {
                Some(v) if valid_host_value(v) => Some(v),
                _ => {
                    println!("invalid sni host");
                    return ArgResult::Error;
                }
            };
        } else if let Some(v) = strip_prefix(arg, "--sni=") {
            if !valid_host_value(v) {
                println!("invalid sni host");
                return ArgResult::Error;
            }
            cfg.connect_host = Some(v);
        } else if arg == "-n" || arg == "--max-preview" {
            cfg.max_preview =
                match next_arg(argc, argv, &mut i, "max-preview").and_then(parse_usize) {
                    Some(v) => v,
                    None => {
                        println!("invalid preview limit");
                        return ArgResult::Error;
                    }
                };
        } else if let Some(v) = strip_prefix(arg, "--max-preview=") {
            cfg.max_preview = match parse_usize(v) {
                Some(v) => v,
                None => {
                    println!("invalid preview limit");
                    return ArgResult::Error;
                }
            };
        } else if starts_with(arg, "-") {
            println!("unknown option: {}", arg);
            return ArgResult::Error;
        } else {
            if positional_count >= MAX_POSITIONALS {
                println!("too many positional arguments");
                return ArgResult::Error;
            }
            positionals[positional_count] = arg;
            positional_count += 1;
        }

        i += 1;
    }

    if positional_count > 0 {
        cfg.target = Some(positionals[0]);
    }
    if positional_count > 1 && cfg.path.is_none() {
        cfg.path = Some(positionals[1]);
    }
    if positional_count > 2 {
        cfg.dns = match parse_ipv4(positionals[2]) {
            Some(v) => v,
            None => {
                println!("invalid dns server");
                return ArgResult::Error;
            }
        };
    }

    ArgResult::Ok
}

fn prepare_request(cfg: &Config) -> Option<Request> {
    let target = cfg.target.unwrap_or("example.com");
    let mut host = target;
    let mut path = "/";
    let mut port = cfg.port.unwrap_or(HTTPS_PORT);

    if let Some(rest) = strip_prefix(host, "https://") {
        host = rest;
    } else if let Some(rest) = strip_prefix(host, "http://") {
        if !cfg.quiet {
            println!("warning: http:// scheme ignored; httpsget still uses TLS");
        }
        host = rest;
    }

    if let Some(pos) = find_byte(host, b'/') {
        path = &host[pos..];
        host = &host[..pos];
    }

    if let Some(pos) = find_byte(host, b':') {
        let parsed_port = match parse_port(&host[pos + 1..]) {
            Some(v) => v,
            None => {
                println!("invalid port in target");
                return None;
            }
        };
        if cfg.port.is_none() {
            port = parsed_port;
        }
        host = &host[..pos];
    }

    if let Some(v) = cfg.path {
        path = v;
    }

    if host.is_empty() {
        println!("empty host");
        return None;
    }
    if path.is_empty() {
        println!("empty path");
        return None;
    }

    let tls_host = cfg.connect_host.or(cfg.host_header).unwrap_or(host);
    let host_header = cfg.host_header.unwrap_or(host);

    let ip = match cfg.ip_override {
        Some(ip) => ip,
        None => match parse_ipv4(host) {
            Some(ip) => ip,
            None => {
                if !cfg.quiet {
                    println!("resolving {} ...", host);
                }
                match resolve(host, cfg.dns) {
                    Some(ip) => ip,
                    None => {
                        println!("dns lookup failed");
                        return None;
                    }
                }
            }
        },
    };

    Some(Request {
        host,
        tls_host,
        host_header,
        path,
        port,
        ip,
    })
}

fn https_get(cfg: &Config, req_info: &Request) -> i32 {
    let start = get_time();
    let fd = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if fd < 0 {
        println!("socket failed: {}", fd);
        return -1;
    }
    let fd = fd as usize;

    let addr = SockAddrIn::new(req_info.ip, req_info.port);
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

    let tls = tls_connect(fd, req_info.tls_host);
    if tls < 0 {
        println!("tls connect failed: {}", tls);
        let _ = close(fd);
        return -1;
    }
    let tls = tls as usize;

    let mut req = [0u8; REQUEST_BUF_SIZE];
    let n = match build_request(cfg, req_info, &mut req) {
        Some(v) => v,
        None => {
            println!("request too long");
            let _ = tls_close(tls);
            let _ = close(fd);
            return -1;
        }
    };

    if cfg.verbose {
        println!("--- request begin ---");
        print_bytes(&req[..n]);
        println!("--- request end ---");
    }

    if !write_all_tls(tls, &req[..n]) {
        let _ = tls_close(tls);
        let _ = close(fd);
        return -1;
    }

    let status = match read_response(tls, cfg, start) {
        Some(v) => v,
        None => {
            let _ = tls_close(tls);
            let _ = close(fd);
            return -1;
        }
    };

    let _ = tls_close(tls);
    let _ = close(fd);

    if cfg.fail_on_http_error && status >= 400 {
        -1
    } else {
        0
    }
}

fn build_request(cfg: &Config, req_info: &Request, out: &mut [u8]) -> Option<usize> {
    let mut n = 0usize;
    if !append(out, &mut n, cfg.method.as_bytes())
        || !append(out, &mut n, b" ")
        || !append(out, &mut n, req_info.path.as_bytes())
        || !append(
            out,
            &mut n,
            if cfg.http11 {
                b" HTTP/1.1\r\nHost: "
            } else {
                b" HTTP/1.0\r\nHost: "
            },
        )
        || !append(out, &mut n, req_info.host_header.as_bytes())
    {
        return None;
    }
    if req_info.port != HTTPS_PORT && find_byte(req_info.host_header, b':').is_none() {
        if !append(out, &mut n, b":") || !append_u16(out, &mut n, req_info.port) {
            return None;
        }
    }
    if !append(
        out,
        &mut n,
        b"\r\nConnection: close\r\nUser-Agent: kairix-httpsget/0.2\r\nAccept: */*\r\n",
    ) {
        return None;
    }
    for i in 0..cfg.header_count {
        if !append(out, &mut n, cfg.headers[i].as_bytes()) || !append(out, &mut n, b"\r\n") {
            return None;
        }
    }
    if !append(out, &mut n, b"\r\n") {
        return None;
    }
    Some(n)
}

fn write_all_tls(tls: usize, mut buf: &[u8]) -> bool {
    while !buf.is_empty() {
        let ret = tls_write(tls, buf);
        if ret < 0 {
            println!("tls write failed: {}", ret);
            return false;
        }
        if ret == 0 {
            println!("tls write returned 0");
            return false;
        }
        let n = ret as usize;
        if n > buf.len() {
            println!("tls write returned invalid length");
            return false;
        }
        buf = &buf[n..];
    }
    true
}

fn read_response(tls: usize, cfg: &Config, start: isize) -> Option<u16> {
    if !cfg.quiet {
        if cfg.headers_only {
            println!("--- response headers begin ---");
        } else if cfg.max_preview > 0 {
            println!("--- response preview begin ---");
        }
    }

    let mut buf = [0u8; READ_BUF_SIZE];
    let mut total = 0usize;
    let mut printed = 0usize;
    let mut truncated = false;
    let mut header_match = 0usize;
    let mut header_done = false;
    let mut status_buf = [0u8; 96];
    let mut status_len = 0usize;
    let mut status_done = false;
    let mut status = None;

    loop {
        let ret = tls_read(tls, &mut buf);
        if ret < 0 {
            println!("\ntls read failed: {}", ret);
            return None;
        }
        if ret == 0 {
            break;
        }
        let got = ret as usize;
        total += got;
        for &b in &buf[..got] {
            if !status_done {
                if b == b'\n' {
                    status_done = true;
                    status = parse_status_code(&status_buf[..status_len]);
                } else if b != b'\r' && status_len < status_buf.len() {
                    status_buf[status_len] = b;
                    status_len += 1;
                }
            }

            if cfg.headers_only && !header_done {
                if !cfg.quiet {
                    print_response_byte(b);
                }
                header_match = update_header_match(header_match, b);
                if header_match == 4 {
                    header_done = true;
                    break;
                }
                continue;
            }

            if !cfg.headers_only && !cfg.quiet && cfg.max_preview > 0 {
                if printed < cfg.max_preview {
                    print_response_byte(b);
                    printed += 1;
                } else {
                    truncated = true;
                }
            } else if !cfg.headers_only && cfg.max_preview == 0 {
                truncated = true;
            }
        }

        if cfg.headers_only && header_done {
            break;
        }
    }

    if !cfg.quiet {
        if cfg.headers_only {
            println!("--- response headers end: {} bytes read ---", total);
        } else {
            if truncated {
                println!("\n--- response preview truncated at {} bytes ---", printed);
            }
            println!("--- response end: {} bytes read ---", total);
        }
        if let Some(code) = status {
            println!("status: {}", code);
        }
        let end = get_time();
        if start >= 0 && end >= start {
            println!("elapsed: {} ms", end - start);
        }
    }

    status.or(Some(0))
}

fn update_header_match(state: usize, b: u8) -> usize {
    const NEEDLE: [u8; 4] = [b'\r', b'\n', b'\r', b'\n'];
    if b == NEEDLE[state] {
        return state + 1;
    }
    if b == b'\r' { 1 } else { 0 }
}

fn parse_status_code(line: &[u8]) -> Option<u16> {
    if line.len() < 12 {
        return None;
    }
    if line[0] != b'H' || line[1] != b'T' || line[2] != b'T' || line[3] != b'P' {
        return None;
    }
    let mut i = 0usize;
    while i < line.len() && line[i] != b' ' {
        i += 1;
    }
    while i < line.len() && line[i] == b' ' {
        i += 1;
    }
    if i + 3 > line.len() {
        return None;
    }
    let mut code = 0u16;
    for _ in 0..3 {
        let b = line[i];
        if !b.is_ascii_digit() {
            return None;
        }
        code = code * 10 + (b - b'0') as u16;
        i += 1;
    }
    Some(code)
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

fn argv_str(argv: *const usize, idx: usize) -> Option<&'static str> {
    cstr_to_str(unsafe { *argv.add(idx) as *const u8 })
}

fn next_arg(argc: usize, argv: *const usize, idx: &mut usize, name: &str) -> Option<&'static str> {
    if *idx + 1 >= argc {
        println!("missing {}", name);
        return None;
    }
    *idx += 1;
    argv_str(argv, *idx)
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
        if len > MAX_ARG_LEN {
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

fn parse_port(s: &str) -> Option<u16> {
    let value = parse_usize(s)?;
    if value == 0 || value > 65535 {
        None
    } else {
        Some(value as u16)
    }
}

fn parse_usize(s: &str) -> Option<usize> {
    if s.is_empty() {
        return None;
    }
    let mut out = 0usize;
    for b in s.bytes() {
        if !b.is_ascii_digit() {
            return None;
        }
        out = out.checked_mul(10)?.checked_add((b - b'0') as usize)?;
    }
    Some(out)
}

fn append(dst: &mut [u8], pos: &mut usize, src: &[u8]) -> bool {
    if *pos + src.len() > dst.len() {
        return false;
    }
    dst[*pos..*pos + src.len()].copy_from_slice(src);
    *pos += src.len();
    true
}

fn append_u16(dst: &mut [u8], pos: &mut usize, mut value: u16) -> bool {
    let mut tmp = [0u8; 5];
    let mut n = 0usize;
    if value == 0 {
        tmp[0] = b'0';
        n = 1;
    } else {
        while value > 0 {
            tmp[n] = b'0' + (value % 10) as u8;
            value /= 10;
            n += 1;
        }
    }
    while n > 0 {
        n -= 1;
        if !append(dst, pos, &tmp[n..n + 1]) {
            return false;
        }
    }
    true
}

fn print_bytes(bytes: &[u8]) {
    for &b in bytes {
        print_response_byte(b);
    }
}

fn print_response_byte(b: u8) {
    match b {
        b'\r' => {}
        b'\n' => println!(""),
        0x20..=0x7e | b'\t' => print!("{}", b as char),
        _ => print!("."),
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

fn starts_with(s: &str, prefix: &str) -> bool {
    strip_prefix(s, prefix).is_some()
}

fn find_byte(s: &str, needle: u8) -> Option<usize> {
    for (idx, b) in s.bytes().enumerate() {
        if b == needle {
            return Some(idx);
        }
    }
    None
}

fn valid_token(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    for b in s.bytes() {
        if !matches!(b, b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'-') {
            return false;
        }
    }
    true
}

fn valid_host_value(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    for b in s.bytes() {
        if b == b'\r' || b == b'\n' || b == b' ' || b == b'\t' {
            return false;
        }
    }
    true
}

fn valid_header(s: &str) -> bool {
    let mut colon = false;
    if s.is_empty() {
        return false;
    }
    for b in s.bytes() {
        if b == b'\r' || b == b'\n' {
            return false;
        }
        if b == b':' {
            colon = true;
        }
    }
    colon
}

fn print_usage() {
    println!("usage: httpsget [options] <host-or-url> [path] [dns-ip]");
    println!("examples:");
    println!("  httpsget example.com /");
    println!("  httpsget https://example.com/index.html -I");
    println!("  httpsget --ip 93.184.216.34 --host example.com example.com /");
    println!("options:");
    println!("  -h, --help                 show this help");
    println!("  -X, --method METHOD        request method, default GET");
    println!("  -H, --header 'K: V'        add request header, up to 8");
    println!("  -I, --head, --headers-only send HEAD and print response headers");
    println!("  -d, --dns IP               DNS server, default 10.0.2.3");
    println!("      --ip IP                skip DNS and connect to this IPv4");
    println!("  -p, --port PORT            TCP port, default 443");
    println!("      --path PATH            override URL path");
    println!("      --host HOST            override Host header and default SNI");
    println!("      --sni HOST             override TLS SNI only");
    println!("  -n, --max-preview N        print at most N response bytes, default 1024");
    println!("      --http10|--http11      HTTP version, default HTTP/1.1");
    println!("  -q, --quiet                suppress normal output");
    println!("  -v, --verbose              print request bytes before sending");
    println!("  -f, --fail                 return error for HTTP status >= 400");
}
