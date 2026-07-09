#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use alloc::{string::String, vec, vec::Vec};
use user_lib::git::{
    GitRef, PktLine, PktLineError, encode_pkt_data, encode_pkt_flush, parse_pkt_line,
    parse_ref_advertisement,
};
use user_lib::{
    AT_FDCWD, OpenFlags, close, connect, open, read, recvfrom, sendto, sleep, socket,
    ssh_auth_password, ssh_auth_publickey, ssh_channel_close, ssh_channel_status,
    ssh_channel_try_read, ssh_channel_write, ssh_close, ssh_connect, ssh_exec, tls_close,
    tls_connect, tls_read, tls_write, write,
};

const AF_INET: i32 = 2;
const SOCK_DGRAM: i32 = 2;
const SOCK_NONBLOCK: i32 = 0o0004000;
const SOCK_STREAM: i32 = 1;
const IPPROTO_TCP: i32 = 6;
const DNS_PORT: u16 = 53;
const HTTPS_PORT: u16 = 443;
const DEFAULT_DNS: u32 = 0x0A000203;
const TXID: u16 = 0x474c; // "GL"
const REQUEST_BUF_SIZE: usize = 2048;
const READ_BUF_SIZE: usize = 1024;
const MAX_ARG_LEN: usize = 512;
const MAX_REF_FILE_LEN: usize = 256;
const MAX_PACK_LEN: usize = 1024 * 1024;
const MAX_BODY_LEN: usize = MAX_PACK_LEN + 512 * 1024;
const MAX_KEY_FILE_LEN: usize = 16 * 1024;
const INITIAL_REFS: usize = 4096;
const MAX_REFS: usize = 32768;
const MAX_CAPS: usize = 64;
const MAX_DNS_ADDRS: usize = 8;
const TCP_CONNECT_RETRIES: usize = 3;
const TLS_READ_IDLE_LIMIT: usize = 300;
const TLS_READ_IDLE_SLEEP_MS: usize = 10;
const HTTP_HEADER_LIMIT: usize = 4096;
const BODY_UNTIL_CLOSE: u8 = 0;
const BODY_CONTENT_LENGTH: u8 = 1;
const BODY_CHUNKED: u8 = 2;
const SSH_PORT: u16 = 22;
const DEFAULT_SSH_IDENT: &str = "SSH-2.0-kairix-gitfetch_0.1";
const EAGAIN_RET: isize = -11;
const SSH_IDLE_LIMIT: usize = 1000;
const SSH_IDLE_SLEEP_MS: usize = 10;
const CHUNK_SIZE: u8 = 0;
const CHUNK_DATA: u8 = 1;
const CHUNK_DATA_CR: u8 = 2;
const CHUNK_DATA_LF: u8 = 3;
const CHUNK_DONE: u8 = 4;

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
    url: Option<&'static str>,
    output: &'static str,
    meta_output: Option<&'static str>,
    dns: u32,
    ip_override: Option<u32>,
    port: Option<u16>,
    ssh_user: Option<&'static str>,
    ssh_password: Option<&'static str>,
    ssh_key_path: Option<&'static str>,
    repo_path: Option<&'static str>,
    have_oid: Option<&'static str>,
    verbose: bool,
}

impl Config {
    fn new() -> Self {
        Self {
            url: None,
            output: "/musl/gitfetch.pack",
            meta_output: None,
            dns: DEFAULT_DNS,
            ip_override: None,
            port: None,
            ssh_user: None,
            ssh_password: None,
            ssh_key_path: None,
            repo_path: None,
            have_oid: None,
            verbose: false,
        }
    }
}

struct Target<'a> {
    host: &'a str,
    path: &'a str,
    port: u16,
    ips: Vec<u32>,
}

struct SshTarget<'a> {
    host: &'a str,
    user: &'a str,
    password: Option<&'a str>,
    key_path: Option<&'a str>,
    repo: &'a str,
    port: u16,
    ips: Vec<u32>,
}

struct ChunkDecoder {
    state: u8,
    line: [u8; 32],
    line_len: usize,
    remaining: usize,
}

struct PackWriter {
    fd: usize,
    bytes: usize,
    saw_pack: bool,
}

struct SidebandStream {
    pending: Vec<u8>,
    complete: bool,
}

struct SelectedFetchRef {
    oid: String,
    name: String,
}

impl ChunkDecoder {
    fn new() -> Self {
        Self {
            state: CHUNK_SIZE,
            line: [0; 32],
            line_len: 0,
            remaining: 0,
        }
    }
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

    let url = match cfg.url {
        Some(v) => v,
        None => {
            print_usage();
            return -1;
        }
    };

    if starts_with(url, "https://") {
        let target = match prepare_target(&cfg) {
            Some(v) => v,
            None => return -1,
        };

        run_gitfetch_https(&cfg, &target)
    } else {
        let target = match prepare_ssh_target(&cfg) {
            Some(v) => v,
            None => return -1,
        };

        run_gitfetch_ssh(&cfg, &target)
    }
}

fn parse_args(argc: usize, argv: *const usize, cfg: &mut Config) -> ArgResult {
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
        } else if arg == "-v" || arg == "--verbose" {
            cfg.verbose = true;
        } else if arg == "-o" || arg == "--output" {
            cfg.output = match next_arg(argc, argv, &mut i, "output") {
                Some(v) if !v.is_empty() => v,
                _ => {
                    println!("invalid output path");
                    return ArgResult::Error;
                }
            };
        } else if let Some(v) = strip_prefix(arg, "--output=") {
            if v.is_empty() {
                println!("invalid output path");
                return ArgResult::Error;
            }
            cfg.output = v;
        } else if arg == "--meta" {
            cfg.meta_output = match next_arg(argc, argv, &mut i, "meta") {
                Some(v) if !v.is_empty() => Some(v),
                _ => {
                    println!("invalid meta path");
                    return ArgResult::Error;
                }
            };
        } else if let Some(v) = strip_prefix(arg, "--meta=") {
            if v.is_empty() {
                println!("invalid meta path");
                return ArgResult::Error;
            }
            cfg.meta_output = Some(v);
        } else if arg == "--repo" {
            cfg.repo_path = match next_arg(argc, argv, &mut i, "repo") {
                Some(v) if !v.is_empty() => Some(v),
                _ => {
                    println!("invalid repo path");
                    return ArgResult::Error;
                }
            };
        } else if let Some(v) = strip_prefix(arg, "--repo=") {
            if v.is_empty() {
                println!("invalid repo path");
                return ArgResult::Error;
            }
            cfg.repo_path = Some(v);
        } else if arg == "--have" {
            cfg.have_oid = match next_arg(argc, argv, &mut i, "have") {
                Some(v) if is_hex_oid(v) => Some(v),
                _ => {
                    println!("invalid have oid");
                    return ArgResult::Error;
                }
            };
        } else if let Some(v) = strip_prefix(arg, "--have=") {
            if !is_hex_oid(v) {
                println!("invalid have oid");
                return ArgResult::Error;
            }
            cfg.have_oid = Some(v);
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
        } else if arg == "-u" || arg == "--user" {
            cfg.ssh_user = match next_arg(argc, argv, &mut i, "user") {
                Some(v) if !v.is_empty() => Some(v),
                _ => {
                    println!("invalid user");
                    return ArgResult::Error;
                }
            };
        } else if let Some(v) = strip_prefix(arg, "--user=") {
            if v.is_empty() {
                println!("invalid user");
                return ArgResult::Error;
            }
            cfg.ssh_user = Some(v);
        } else if arg == "--password" {
            cfg.ssh_password = match next_arg(argc, argv, &mut i, "password") {
                Some(v) => Some(v),
                None => {
                    println!("invalid password");
                    return ArgResult::Error;
                }
            };
        } else if let Some(v) = strip_prefix(arg, "--password=") {
            cfg.ssh_password = Some(v);
        } else if arg == "--key" || arg == "-i" {
            cfg.ssh_key_path = match next_arg(argc, argv, &mut i, "key") {
                Some(v) if !v.is_empty() => Some(v),
                _ => {
                    println!("invalid key path");
                    return ArgResult::Error;
                }
            };
        } else if let Some(v) = strip_prefix(arg, "--key=") {
            if v.is_empty() {
                println!("invalid key path");
                return ArgResult::Error;
            }
            cfg.ssh_key_path = Some(v);
        } else if starts_with(arg, "-") {
            println!("unknown option: {}", arg);
            return ArgResult::Error;
        } else if cfg.url.is_none() {
            cfg.url = Some(arg);
        } else if let Some(dns) = parse_ipv4(arg) {
            cfg.dns = dns;
        } else {
            println!("too many arguments");
            return ArgResult::Error;
        }
        i += 1;
    }
    ArgResult::Ok
}

fn prepare_target<'a>(cfg: &'a Config) -> Option<Target<'a>> {
    let url = match cfg.url {
        Some(v) => v,
        None => {
            print_usage();
            return None;
        }
    };

    let mut rest = match strip_prefix(url, "https://") {
        Some(v) => v,
        None => {
            println!("only https:// URLs are supported for now");
            return None;
        }
    };

    let mut path = "/";
    if let Some(pos) = find_byte(rest, b'/') {
        path = &rest[pos..];
        rest = &rest[..pos];
    }

    let mut host = rest;
    let mut port = cfg.port.unwrap_or(HTTPS_PORT);
    if let Some(pos) = find_byte(host, b':') {
        let parsed_port = match parse_port(&host[pos + 1..]) {
            Some(v) => v,
            None => {
                println!("invalid port in URL");
                return None;
            }
        };
        if cfg.port.is_none() {
            port = parsed_port;
        }
        host = &host[..pos];
    }

    if host.is_empty() || path.is_empty() {
        println!("invalid URL");
        return None;
    }

    let ips = match resolve_target_ips(host, cfg) {
        Some(v) => v,
        None => return None,
    };

    Some(Target {
        host,
        path,
        port,
        ips,
    })
}

fn prepare_ssh_target<'a>(cfg: &'a Config) -> Option<SshTarget<'a>> {
    let url = match cfg.url {
        Some(v) => v,
        None => {
            print_usage();
            return None;
        }
    };

    let mut user = cfg.ssh_user;
    let mut password = cfg.ssh_password;
    let key_path = cfg.ssh_key_path;
    let mut host;
    let repo;
    let mut port = cfg.port.unwrap_or(SSH_PORT);

    if let Some(rest) = strip_prefix(url, "ssh://") {
        let slash = match find_byte(rest, b'/') {
            Some(v) => v,
            None => {
                println!("invalid ssh URL");
                return None;
            }
        };
        let mut authority = &rest[..slash];
        repo = &rest[slash..];
        if let Some(at) = find_byte(authority, b'@') {
            let info = &authority[..at];
            authority = &authority[at + 1..];
            if let Some(colon) = find_byte(info, b':') {
                if user.is_none() {
                    user = Some(&info[..colon]);
                }
                if password.is_none() {
                    password = Some(&info[colon + 1..]);
                }
            } else if user.is_none() {
                user = Some(info);
            }
        }
        host = authority;
        if let Some(colon) = find_byte(host, b':') {
            let parsed = match parse_port(&host[colon + 1..]) {
                Some(v) => v,
                None => {
                    println!("invalid ssh port");
                    return None;
                }
            };
            if cfg.port.is_none() {
                port = parsed;
            }
            host = &host[..colon];
        }
    } else if let Some(at) = find_byte(url, b'@') {
        let after_user = &url[at + 1..];
        let colon = match find_byte(after_user, b':') {
            Some(v) => v,
            None => {
                println!("unsupported URL; use https://, ssh://, or user@host:repo.git");
                return None;
            }
        };
        if user.is_none() {
            user = Some(&url[..at]);
        }
        host = &after_user[..colon];
        repo = &after_user[colon + 1..];
    } else {
        println!("unsupported URL; use https://, ssh://, or user@host:repo.git");
        return None;
    }

    let user = match user {
        Some(v) if !v.is_empty() => v,
        _ => {
            println!("missing ssh user; use ssh://user@host/repo.git or --user USER");
            return None;
        }
    };
    if password.is_none() && key_path.is_none() {
        println!("missing ssh auth; use --password PASS or --key /path/id_ed25519");
        return None;
    }

    if host.is_empty() || repo.is_empty() || !valid_ssh_repo(repo) {
        println!("invalid ssh repo");
        return None;
    }

    let ips = match resolve_target_ips(host, cfg) {
        Some(v) => v,
        None => return None,
    };

    Some(SshTarget {
        host,
        user,
        password,
        key_path,
        repo,
        port,
        ips,
    })
}

fn resolve_target_ips(host: &str, cfg: &Config) -> Option<Vec<u32>> {
    if let Some(ip) = cfg.ip_override {
        return Some(vec![ip]);
    }
    if let Some(ip) = parse_ipv4(host) {
        return Some(vec![ip]);
    }

    println!("resolving {} ...", host);
    let mut ips = Vec::new();
    resolve_append(host, cfg.dns, &mut ips);
    for dns in [0x01010101, 0x08080808, 0x09090909] {
        if ips.len() >= MAX_DNS_ADDRS {
            break;
        }
        if dns != cfg.dns {
            resolve_append(host, dns, &mut ips);
        }
    }

    if ips.is_empty() {
        println!("dns lookup failed");
        None
    } else {
        Some(ips)
    }
}

fn run_gitfetch_https(cfg: &Config, target: &Target<'_>) -> i32 {
    let mut last_status = -1;
    for (idx, &ip) in target.ips.iter().enumerate() {
        if idx > 0 {
            print!("trying next ip ");
            print_ipv4(ip);
            println!(" ...");
        }
        print_gitfetch_target(target.host, ip, target.path);
        match try_gitfetch_https_ip(cfg, target, ip) {
            HttpsAttempt::Ok(code) => return code,
            HttpsAttempt::Retry(status) => last_status = status,
        }
    }

    if last_status != -1 {
        println!(
            "https gitfetch failed after trying {} ip(s)",
            target.ips.len()
        );
    }
    -1
}

enum HttpsAttempt {
    Ok(i32),
    Retry(i32),
}

fn https_info_refs(cfg: &Config, target: &Target<'_>, ip: u32) -> Option<Vec<u8>> {
    let mut req = [0u8; REQUEST_BUF_SIZE];
    let n = match build_info_refs_request(target, &mut req) {
        Some(v) => v,
        None => {
            println!("request too long");
            return None;
        }
    };
    let (status, body) = send_https_request(cfg, target.host, target.port, ip, &req[..n])?;
    if status != 200 {
        println!("info/refs http status: {}", status);
        return None;
    }
    Some(body)
}

fn https_upload_pack_to_file(
    cfg: &Config,
    target: &Target<'_>,
    ip: u32,
    body: &[u8],
) -> Option<usize> {
    let req = build_upload_pack_request(target, body)?;
    let (status, bytes) =
        send_https_request_to_pack(cfg, target.host, target.port, ip, &req, cfg.output)?;
    if status != 200 {
        println!("git-upload-pack http status: {}", status);
        return None;
    }
    Some(bytes)
}

fn send_https_request(
    cfg: &Config,
    host: &str,
    port: u16,
    ip: u32,
    req: &[u8],
) -> Option<(u16, Vec<u8>)> {
    let fd = open_connected_socket(ip, port)?;

    println!("tls connect ...");
    let tls = tls_connect(fd, host);
    if tls < 0 {
        println!("tls connect failed: {}", tls);
        let _ = close(fd);
        return None;
    }
    let tls = tls as usize;

    if cfg.verbose {
        println!("--- request begin ---");
        print_bytes(req);
        println!("--- request end ---");
    }

    if !write_all_tls(tls, req) {
        let _ = tls_close(tls);
        let _ = close(fd);
        return None;
    }

    let mut body = Vec::new();
    let status = match read_http_body(tls, &mut body) {
        Some(v) => v,
        None => {
            let _ = tls_close(tls);
            let _ = close(fd);
            return None;
        }
    };

    let _ = tls_close(tls);
    let _ = close(fd);
    Some((status, body))
}

fn send_https_request_to_pack(
    cfg: &Config,
    host: &str,
    port: u16,
    ip: u32,
    req: &[u8],
    output: &str,
) -> Option<(u16, usize)> {
    let fd = open_connected_socket(ip, port)?;

    println!("tls connect ...");
    let tls = tls_connect(fd, host);
    if tls < 0 {
        println!("tls connect failed: {}", tls);
        let _ = close(fd);
        return None;
    }
    let tls = tls as usize;

    if cfg.verbose {
        println!("--- request begin ---");
        print_bytes(req);
        println!("--- request end ---");
    }

    if !write_all_tls(tls, req) {
        let _ = tls_close(tls);
        let _ = close(fd);
        return None;
    }

    let status = match read_http_pack_body(tls, output) {
        Some(v) => v,
        None => {
            let _ = tls_close(tls);
            let _ = close(fd);
            return None;
        }
    };

    let _ = tls_close(tls);
    let _ = close(fd);
    Some(status)
}

fn try_gitfetch_https_ip(cfg: &Config, target: &Target<'_>, ip: u32) -> HttpsAttempt {
    let refs_body = match https_info_refs(cfg, target, ip) {
        Some(v) => v,
        None => return HttpsAttempt::Retry(-1),
    };
    let selected = match choose_fetch_ref(&refs_body) {
        Some(v) => v,
        None => return HttpsAttempt::Retry(-1),
    };
    println!("want {}", selected.oid);
    let have = local_have_oid(cfg);
    if let Some(ref oid) = have {
        println!("have {}", oid);
    }
    if !write_fetch_meta(cfg, &selected) {
        return HttpsAttempt::Retry(-1);
    }

    let request_body = match build_fetch_request(&selected.oid, have.as_deref()) {
        Some(v) => v,
        None => return HttpsAttempt::Retry(-1),
    };
    let pack_bytes = match https_upload_pack_to_file(cfg, target, ip, &request_body) {
        Some(v) => v,
        None => return HttpsAttempt::Retry(-1),
    };
    println!("saved pack: {}", cfg.output);
    println!("pack bytes: {}", pack_bytes);
    HttpsAttempt::Ok(0)
}

fn run_gitfetch_ssh(cfg: &Config, target: &SshTarget<'_>) -> i32 {
    let (fd, ip) = match open_connected_socket_any(&target.ips, target.port) {
        Some(v) => v,
        None => return -1,
    };
    print_gitfetch_ssh_target(target.user, target.host, ip, target.repo);

    let ssh_id = ssh_connect(fd, DEFAULT_SSH_IDENT);
    if ssh_id < 0 {
        println!("ssh connect failed: {}", ssh_id);
        let _ = close(fd);
        return -1;
    }
    let ssh_id = ssh_id as usize;

    if !auth_ssh_target(ssh_id, target) {
        let _ = ssh_close(ssh_id);
        let _ = close(fd);
        return -1;
    }

    let command = build_upload_pack_command(target.repo);
    if cfg.verbose {
        println!("ssh exec: {}", command);
    }
    let channel_id = ssh_exec(ssh_id, &command);
    if channel_id < 0 {
        println!("ssh exec failed: {}", channel_id);
        let _ = ssh_close(ssh_id);
        let _ = close(fd);
        return -1;
    }
    let channel_id = channel_id as usize;

    let mut body = Vec::new();
    let read_ok = read_ssh_refs(ssh_id, channel_id, &mut body);

    if !read_ok {
        let _ = ssh_channel_close(ssh_id, channel_id);
        let _ = ssh_close(ssh_id);
        let _ = close(fd);
        return -1;
    }

    if !refs_advertisement_complete(&body) {
        println!("incomplete ssh git refs");
        if !body.is_empty() {
            print!("ssh git-upload-pack output: ");
            print_lossy_line(&body);
            println!("");
        }
        let _ = ssh_channel_close(ssh_id, channel_id);
        let _ = ssh_close(ssh_id);
        let _ = close(fd);
        return -1;
    }

    let selected = match choose_fetch_ref(&body) {
        Some(v) => v,
        None => {
            let _ = ssh_channel_close(ssh_id, channel_id);
            let _ = ssh_close(ssh_id);
            let _ = close(fd);
            return -1;
        }
    };
    println!("want {}", selected.oid);
    let have = local_have_oid(cfg);
    if let Some(ref oid) = have {
        println!("have {}", oid);
    }
    if !write_fetch_meta(cfg, &selected) {
        let _ = ssh_channel_close(ssh_id, channel_id);
        let _ = ssh_close(ssh_id);
        let _ = close(fd);
        return -1;
    }
    let request = match build_fetch_request(&selected.oid, have.as_deref()) {
        Some(v) => v,
        None => {
            let _ = ssh_channel_close(ssh_id, channel_id);
            let _ = ssh_close(ssh_id);
            let _ = close(fd);
            return -1;
        }
    };
    if !write_all_ssh_channel(ssh_id, channel_id, &request) {
        let _ = ssh_channel_close(ssh_id, channel_id);
        let _ = ssh_close(ssh_id);
        let _ = close(fd);
        return -1;
    }

    let pack_bytes = match read_ssh_pack_to_file(ssh_id, channel_id, cfg.output) {
        Some(v) => v,
        None => {
            let _ = ssh_channel_close(ssh_id, channel_id);
            let _ = ssh_close(ssh_id);
            let _ = close(fd);
            return -1;
        }
    };

    let _ = ssh_channel_close(ssh_id, channel_id);
    let _ = ssh_close(ssh_id);
    let _ = close(fd);

    println!("saved pack: {}", cfg.output);
    println!("pack bytes: {}", pack_bytes);
    0
}

fn auth_ssh_target(ssh_id: usize, target: &SshTarget<'_>) -> bool {
    if let Some(path) = target.key_path {
        let key = match read_key_file(path) {
            Some(v) => v,
            None => return false,
        };
        let ret = ssh_auth_publickey(ssh_id, target.user, &key);
        if ret >= 0 {
            return true;
        }
        println!("ssh publickey auth failed: {}", ret);
        if target.password.is_none() {
            return false;
        }
        println!("trying ssh password auth ...");
    }

    let Some(password) = target.password else {
        return false;
    };
    let ret = ssh_auth_password(ssh_id, target.user, password);
    if ret < 0 {
        println!("ssh password auth failed: {}", ret);
        return false;
    }
    true
}

fn read_key_file(path: &str) -> Option<Vec<u8>> {
    let fd = open(AT_FDCWD, path, OpenFlags::RDONLY, 0);
    if fd < 0 {
        println!("open key failed: {}", fd);
        return None;
    }
    let fd = fd as usize;

    let mut out = Vec::new();
    let mut buf = [0u8; 512];
    loop {
        let n = read(fd, &mut buf);
        if n < 0 {
            println!("read key failed: {}", n);
            let _ = close(fd);
            return None;
        }
        if n == 0 {
            break;
        }
        if out.len() + n as usize > MAX_KEY_FILE_LEN {
            println!("key file too large");
            let _ = close(fd);
            return None;
        }
        out.extend_from_slice(&buf[..n as usize]);
    }

    let _ = close(fd);
    if out.is_empty() {
        println!("empty key file");
        None
    } else {
        Some(out)
    }
}

fn read_ssh_refs(ssh_id: usize, channel_id: usize, body: &mut Vec<u8>) -> bool {
    let mut buf = [0u8; READ_BUF_SIZE];
    let mut idle = 0usize;

    loop {
        let n = ssh_channel_try_read(ssh_id, channel_id, &mut buf);
        if n == EAGAIN_RET {
            idle += 1;
            if idle >= SSH_IDLE_LIMIT {
                let status = ssh_channel_status(ssh_id, channel_id);
                if status >= 0 {
                    if refs_advertisement_complete(body) {
                        return true;
                    }
                    println!("ssh git-upload-pack exit: {}", status);
                    return true;
                }
                if status != EAGAIN_RET {
                    println!("ssh channel status failed: {}", status);
                    return false;
                }
                println!("ssh channel read timeout");
                return false;
            }
            sleep(SSH_IDLE_SLEEP_MS);
            continue;
        }
        idle = 0;

        if n < 0 {
            println!("ssh channel read failed: {}", n);
            return false;
        }
        if n == 0 {
            return true;
        }
        if !append_body(body, &buf[..n as usize]) {
            return false;
        }
        if refs_advertisement_complete(body) {
            return true;
        }
    }
}

fn read_ssh_pack_to_file(ssh_id: usize, channel_id: usize, output: &str) -> Option<usize> {
    let mut buf = [0u8; READ_BUF_SIZE];
    let mut idle = 0usize;
    let mut writer = PackWriter::open(output)?;
    let mut stream = SidebandStream::new();

    loop {
        let n = ssh_channel_try_read(ssh_id, channel_id, &mut buf);
        if n == EAGAIN_RET {
            idle += 1;
            if idle >= SSH_IDLE_LIMIT {
                let status = ssh_channel_status(ssh_id, channel_id);
                if status >= 0 {
                    if stream.complete {
                        return writer.finish();
                    }
                    println!("ssh git-upload-pack exit: {}", status);
                    return writer.finish();
                }
                if status != EAGAIN_RET {
                    println!("ssh channel status failed: {}", status);
                    return None;
                }
                println!("ssh pack read timeout");
                return None;
            }
            sleep(SSH_IDLE_SLEEP_MS);
            continue;
        }
        idle = 0;

        if n < 0 {
            println!("ssh pack read failed: {}", n);
            return None;
        }
        if n == 0 {
            return writer.finish();
        }
        stream.feed(&buf[..n as usize], &mut writer)?;
        if stream.complete {
            return writer.finish();
        }
    }
}

fn write_all_ssh_channel(ssh_id: usize, channel_id: usize, mut buf: &[u8]) -> bool {
    while !buf.is_empty() {
        let n = ssh_channel_write(ssh_id, channel_id, buf);
        if n < 0 {
            println!("ssh channel write failed: {}", n);
            return false;
        }
        if n == 0 {
            println!("ssh channel write returned 0");
            return false;
        }
        buf = &buf[n as usize..];
    }
    true
}

fn refs_advertisement_complete(input: &[u8]) -> bool {
    let mut pos = 0usize;
    let mut saw_data = false;
    while pos < input.len() {
        match parse_pkt_line(&input[pos..]) {
            Ok((PktLine::Flush, _)) => return saw_data,
            Ok((PktLine::Data(_), used)) => {
                saw_data = true;
                pos += used;
            }
            Err(PktLineError::Incomplete) => return false,
            Err(_) => return false,
        }
    }
    false
}

fn print_lossy_line(input: &[u8]) {
    for &b in input {
        if b == b'\r' || b == b'\n' {
            print!(" ");
        } else if b.is_ascii_graphic() || b == b' ' {
            print!("{}", b as char);
        } else {
            print!(".");
        }
    }
}

fn open_connected_socket_any(ips: &[u32], port: u16) -> Option<(usize, u32)> {
    for (idx, &ip) in ips.iter().enumerate() {
        if idx > 0 {
            print!("trying next ip ");
            print_ipv4(ip);
            println!(" ...");
        }
        if let Some(fd) = open_connected_socket(ip, port) {
            return Some((fd, ip));
        }
    }
    None
}

fn open_connected_socket(ip: u32, port: u16) -> Option<usize> {
    let addr = SockAddrIn::new(ip, port);
    let mut last = -1;
    for attempt in 0..TCP_CONNECT_RETRIES {
        let fd = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        if fd < 0 {
            println!("socket failed: {}", fd);
            return None;
        }
        let fd = fd as usize;
        let ret = connect(
            fd,
            &addr as *const SockAddrIn as *const u8,
            core::mem::size_of::<SockAddrIn>(),
        );
        if ret >= 0 {
            return Some(fd);
        }
        last = ret;
        let _ = close(fd);
        if attempt + 1 < TCP_CONNECT_RETRIES {
            println!("tcp connect failed: {}, retrying ...", ret);
            sleep(200);
        }
    }
    println!("tcp connect failed: {}", last);
    None
}

fn print_gitfetch_target(host: &str, ip: u32, path: &str) {
    print!("gitfetch: {} (", host);
    print_ipv4(ip);
    println!("){}", path);
}

fn print_gitfetch_ssh_target(user: &str, host: &str, ip: u32, repo: &str) {
    print!("gitfetch ssh: {}@{} (", user, host);
    print_ipv4(ip);
    println!(") {}", repo);
}

fn print_ipv4(ip: u32) {
    print!(
        "{}.{}.{}.{}",
        (ip >> 24) & 0xff,
        (ip >> 16) & 0xff,
        (ip >> 8) & 0xff,
        ip & 0xff
    );
}

fn build_info_refs_request(target: &Target<'_>, out: &mut [u8]) -> Option<usize> {
    let mut n = 0usize;
    if !append(out, &mut n, b"GET ")
        || !append(out, &mut n, target.path.as_bytes())
        || !append_info_refs_suffix(out, &mut n, target.path)
        || !append(out, &mut n, b" HTTP/1.1\r\nHost: ")
        || !append(out, &mut n, target.host.as_bytes())
    {
        return None;
    }
    if target.port != HTTPS_PORT && find_byte(target.host, b':').is_none() {
        if !append(out, &mut n, b":") || !append_u16(out, &mut n, target.port) {
            return None;
        }
    }
    if !append(
        out,
        &mut n,
        b"\r\nUser-Agent: kairix-gitfetch/0.1\r\nAccept: application/x-git-upload-pack-advertisement\r\nAccept-Encoding: identity\r\nConnection: close\r\n\r\n",
    ) {
        return None;
    }
    Some(n)
}

fn append_info_refs_suffix(out: &mut [u8], pos: &mut usize, path: &str) -> bool {
    if path.ends_with('/') {
        append(out, pos, b"info/refs?service=git-upload-pack")
    } else {
        append(out, pos, b"/info/refs?service=git-upload-pack")
    }
}

fn build_upload_pack_request(target: &Target<'_>, body: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(b"POST ");
    out.extend_from_slice(target.path.as_bytes());
    if target.path.ends_with('/') {
        out.extend_from_slice(b"git-upload-pack");
    } else {
        out.extend_from_slice(b"/git-upload-pack");
    }
    out.extend_from_slice(b" HTTP/1.1\r\nHost: ");
    out.extend_from_slice(target.host.as_bytes());
    if target.port != HTTPS_PORT && find_byte(target.host, b':').is_none() {
        out.push(b':');
        append_u16_vec(&mut out, target.port);
    }
    out.extend_from_slice(
        b"\r\nUser-Agent: kairix-gitfetch/0.1\r\nAccept: application/x-git-upload-pack-result\r\nContent-Type: application/x-git-upload-pack-request\r\nAccept-Encoding: identity\r\nContent-Length: ",
    );
    append_usize_vec(&mut out, body.len());
    out.extend_from_slice(b"\r\nConnection: close\r\n\r\n");
    out.extend_from_slice(body);
    Some(out)
}

fn build_fetch_request(want_oid: &str, have_oid: Option<&str>) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut want = String::new();
    want.push_str("want ");
    want.push_str(want_oid);
    want.push_str(" multi_ack_detailed side-band-64k thin-pack ofs-delta\n");
    encode_pkt_data(want.as_bytes(), &mut out).ok()?;
    encode_pkt_flush(&mut out);
    if let Some(oid) = have_oid {
        let mut have = String::new();
        have.push_str("have ");
        have.push_str(oid);
        have.push('\n');
        encode_pkt_data(have.as_bytes(), &mut out).ok()?;
    }
    encode_pkt_data(b"done\n", &mut out).ok()?;
    Some(out)
}

impl PackWriter {
    fn open(path: &str) -> Option<Self> {
        let fd = open(
            AT_FDCWD,
            path,
            OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::WRONLY,
            0o644,
        );
        if fd < 0 {
            println!("open output failed: {}", fd);
            return None;
        }
        Some(Self {
            fd: fd as usize,
            bytes: 0,
            saw_pack: false,
        })
    }

    fn write_pack(&mut self, bytes: &[u8]) -> Option<()> {
        if bytes.is_empty() {
            return Some(());
        }
        if !self.saw_pack {
            if self.bytes == 0 {
                if bytes.len() < 4 || &bytes[..4] != b"PACK" {
                    println!("missing PACK header");
                    return None;
                }
                self.saw_pack = true;
            } else {
                println!("invalid split PACK header");
                return None;
            }
        }
        let mut written = 0usize;
        while written < bytes.len() {
            let n = write(self.fd, &bytes[written..]);
            if n < 0 {
                println!("write output failed: {}", n);
                return None;
            }
            if n == 0 {
                println!("write output returned 0");
                return None;
            }
            written += n as usize;
        }
        self.bytes += bytes.len();
        Some(())
    }

    fn finish(self) -> Option<usize> {
        let bytes = self.bytes;
        let saw_pack = self.saw_pack;
        let _ = close(self.fd);
        if !saw_pack {
            println!("no pack received");
        }
        Some(bytes)
    }
}

impl SidebandStream {
    fn new() -> Self {
        Self {
            pending: Vec::new(),
            complete: false,
        }
    }

    fn feed(&mut self, bytes: &[u8], writer: &mut PackWriter) -> Option<()> {
        if self.complete {
            return Some(());
        }
        self.pending.extend_from_slice(bytes);
        loop {
            match parse_pkt_line(&self.pending) {
                Ok((PktLine::Flush, used)) => {
                    self.pending.drain(0..used);
                    self.complete = true;
                    return Some(());
                }
                Ok((PktLine::Data(data), used)) => {
                    let mut payload = Vec::new();
                    payload.extend_from_slice(data);
                    self.pending.drain(0..used);
                    self.handle_data(&payload, writer)?;
                }
                Err(PktLineError::Incomplete) => return Some(()),
                Err(err) => {
                    println!("invalid upload-pack response: {:?}", err);
                    return None;
                }
            }
        }
    }

    fn handle_data(&mut self, data: &[u8], writer: &mut PackWriter) -> Option<()> {
        if data == b"NAK\n" || starts_with_bytes(data, b"ACK ") || data.is_empty() {
            return Some(());
        }
        match data[0] {
            1 => writer.write_pack(&data[1..]),
            2 => {
                print_progress(&data[1..]);
                Some(())
            }
            3 => {
                print!("remote fatal: ");
                print_lossy_line(&data[1..]);
                println!("");
                None
            }
            band => {
                println!("invalid side-band channel: {}", band);
                None
            }
        }
    }
}

fn choose_fetch_ref(body: &[u8]) -> Option<SelectedFetchRef> {
    let mut cap = INITIAL_REFS;
    loop {
        let mut refs = vec![GitRef { oid: "", name: "" }; cap];
        let mut caps = vec![""; MAX_CAPS];
        let parsed = match parse_ref_advertisement(body, &mut refs, &mut caps) {
            Ok(v) => v,
            Err(PktLineError::OutputTooSmall) if cap < MAX_REFS => {
                cap = (cap * 2).min(MAX_REFS);
                continue;
            }
            Err(err) => {
                println!("parse git refs failed: {:?}", err);
                return None;
            }
        };

        if let Some(head) = parsed.head {
            if let Some(symref) = find_head_symref(parsed.capabilities) {
                if let Some(r) = parsed.refs.iter().find(|r| r.name == symref) {
                    return Some(SelectedFetchRef {
                        oid: String::from(r.oid),
                        name: String::from(r.name),
                    });
                }
                return Some(SelectedFetchRef {
                    oid: String::from(head.oid),
                    name: String::from(symref),
                });
            }
            return Some(SelectedFetchRef {
                oid: String::from(head.oid),
                name: String::from("HEAD"),
            });
        }
        if let Some(r) = parsed.refs.iter().find(|r| r.name == "refs/heads/main") {
            return Some(SelectedFetchRef {
                oid: String::from(r.oid),
                name: String::from(r.name),
            });
        }
        if let Some(r) = parsed.refs.iter().find(|r| r.name == "refs/heads/master") {
            return Some(SelectedFetchRef {
                oid: String::from(r.oid),
                name: String::from(r.name),
            });
        }
        if let Some(r) = parsed
            .refs
            .iter()
            .find(|r| starts_with(r.name, "refs/heads/"))
        {
            return Some(SelectedFetchRef {
                oid: String::from(r.oid),
                name: String::from(r.name),
            });
        }
        if let Some(r) = parsed.refs.first() {
            return Some(SelectedFetchRef {
                oid: String::from(r.oid),
                name: String::from(r.name),
            });
        }
        println!("no fetchable ref found");
        return None;
    }
}

fn find_head_symref<'a>(caps: &[&'a str]) -> Option<&'a str> {
    for &cap in caps {
        if let Some(v) = strip_prefix(cap, "symref=HEAD:") {
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

fn write_fetch_meta(cfg: &Config, selected: &SelectedFetchRef) -> bool {
    let Some(path) = cfg.meta_output else {
        return true;
    };

    let mut data = Vec::new();
    data.extend_from_slice(b"oid ");
    data.extend_from_slice(selected.oid.as_bytes());
    data.push(b'\n');
    data.extend_from_slice(b"ref ");
    data.extend_from_slice(selected.name.as_bytes());
    data.push(b'\n');
    if let Some(url) = cfg.url {
        data.extend_from_slice(b"url ");
        data.extend_from_slice(url.as_bytes());
        data.push(b'\n');
    }

    if !write_file(path, &data) {
        return false;
    }
    println!("saved meta: {}", path);
    true
}

fn write_file(path: &str, data: &[u8]) -> bool {
    let fd = open(
        AT_FDCWD,
        path,
        OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::WRONLY,
        0o644,
    );
    if fd < 0 {
        println!("open meta failed: {}", fd);
        return false;
    }
    let fd = fd as usize;
    let mut written = 0usize;
    while written < data.len() {
        let n = write(fd, &data[written..]);
        if n < 0 {
            println!("write meta failed: {}", n);
            let _ = close(fd);
            return false;
        }
        if n == 0 {
            println!("write meta returned 0");
            let _ = close(fd);
            return false;
        }
        written += n as usize;
    }
    let _ = close(fd);
    true
}

fn local_have_oid(cfg: &Config) -> Option<String> {
    if let Some(oid) = cfg.have_oid {
        return Some(String::from(oid));
    }
    let repo = cfg.repo_path?;
    let oid = read_repo_head_oid(repo)?;
    Some(oid)
}

fn read_repo_head_oid(repo: &str) -> Option<String> {
    let git_dir = join_path(repo, ".git")?;
    let head_path = join_path(&git_dir, "HEAD")?;
    let head_data = read_text_file(&head_path, MAX_REF_FILE_LEN)?;
    let head = trim_ascii_str(&head_data)?;
    if let Some(ref_name) = strip_prefix(head, "ref: ") {
        if !is_safe_ref_name(ref_name) {
            println!("unsafe HEAD ref");
            return None;
        }
        let ref_path = join_path(&git_dir, ref_name)?;
        let ref_data = read_text_file(&ref_path, MAX_REF_FILE_LEN)?;
        let oid = trim_ascii_str(&ref_data)?;
        if is_hex_oid(oid) {
            return Some(String::from(oid));
        }
        println!("invalid local ref oid");
        return None;
    }
    if is_hex_oid(head) {
        return Some(String::from(head));
    }
    println!("unsupported local HEAD");
    None
}

fn read_text_file(path: &str, max_len: usize) -> Option<Vec<u8>> {
    let fd = open(AT_FDCWD, path, OpenFlags::RDONLY, 0);
    if fd < 0 {
        return None;
    }
    let fd = fd as usize;
    let mut out = Vec::new();
    let mut buf = [0u8; 128];
    loop {
        let n = read(fd, &mut buf);
        if n < 0 {
            let _ = close(fd);
            return None;
        }
        if n == 0 {
            break;
        }
        if out.len() + n as usize > max_len {
            let _ = close(fd);
            return None;
        }
        out.extend_from_slice(&buf[..n as usize]);
    }
    let _ = close(fd);
    Some(out)
}

fn trim_ascii_str(input: &[u8]) -> Option<&str> {
    let mut start = 0usize;
    let mut end = input.len();
    while start < end && input[start].is_ascii_whitespace() {
        start += 1;
    }
    while end > start && input[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    core::str::from_utf8(&input[start..end]).ok()
}

fn join_path(parent: &str, name: &str) -> Option<String> {
    if parent.len() + name.len() + 2 > MAX_ARG_LEN {
        println!("path too long");
        return None;
    }
    let mut out = String::new();
    out.push_str(parent);
    if !parent.ends_with('/') {
        out.push('/');
    }
    out.push_str(name);
    Some(out)
}

fn is_hex_oid(input: &str) -> bool {
    input.len() == 40 && input.bytes().all(|b| b.is_ascii_hexdigit())
}

fn is_safe_ref_name(input: &str) -> bool {
    if !starts_with(input, "refs/") {
        return false;
    }
    let mut prev_slash = false;
    for &b in input.as_bytes() {
        if b == b'/' {
            if prev_slash {
                return false;
            }
            prev_slash = true;
            continue;
        }
        prev_slash = false;
        if b == b'.' || b == b'\\' || b == 0 || b <= b' ' {
            return false;
        }
    }
    !input.ends_with('/')
}

fn print_progress(input: &[u8]) {
    if input.is_empty() {
        return;
    }
    print!("remote: ");
    print_lossy_line(input);
    println!("");
}

fn read_http_body(tls: usize, body: &mut Vec<u8>) -> Option<u16> {
    let mut buf = [0u8; READ_BUF_SIZE];
    let mut header = Vec::new();
    let mut header_match = 0usize;
    let mut header_done = false;
    let mut status = None;
    let mut body_mode = BODY_UNTIL_CLOSE;
    let mut remaining = 0usize;
    let mut chunk_decoder = ChunkDecoder::new();

    loop {
        let ret = tls_read_with_wait(tls, &mut buf);
        if ret < 0 {
            if header_done {
                println!("tls read failed: {} after {} body bytes", ret, body.len());
            } else {
                println!("tls read failed: {} before http header", ret);
            }
            return None;
        }
        if ret == 0 {
            break;
        }

        let got = ret as usize;
        let mut payload_start = 0usize;
        if !header_done {
            let mut i = 0usize;
            while i < got {
                if header.len() >= HTTP_HEADER_LIMIT {
                    println!("http header too large");
                    return None;
                }
                header.push(buf[i]);
                header_match = update_header_match(header_match, buf[i]);
                i += 1;
                if header_match == 4 {
                    header_done = true;
                    payload_start = i;
                    let header_fields = &header[..header.len() - 4];
                    status = parse_status_code(header_fields);
                    if header_has_value(header_fields, b"transfer-encoding", b"chunked") {
                        body_mode = BODY_CHUNKED;
                    } else if let Some(len) = header_usize(header_fields, b"content-length") {
                        if len > MAX_BODY_LEN {
                            println!("response body too large");
                            return None;
                        }
                        body_mode = BODY_CONTENT_LENGTH;
                        remaining = len;
                    }
                    break;
                }
            }
            if !header_done {
                continue;
            }
        }

        let payload = &buf[payload_start..got];
        match body_mode {
            BODY_CHUNKED => {
                if feed_chunked(&mut chunk_decoder, payload, body)? {
                    break;
                }
            }
            BODY_CONTENT_LENGTH => {
                let take = remaining.min(payload.len());
                if !append_body(body, &payload[..take]) {
                    return None;
                }
                remaining -= take;
                if remaining == 0 {
                    break;
                }
            }
            _ => {
                if !append_body(body, payload) {
                    return None;
                }
            }
        }
    }

    if !header_done {
        println!("incomplete http response");
        return None;
    }
    if body_mode == BODY_CONTENT_LENGTH && remaining != 0 {
        println!("truncated response body");
        return None;
    }
    if body_mode == BODY_CHUNKED && chunk_decoder.state != CHUNK_DONE {
        println!("truncated chunked response");
        return None;
    }

    match status {
        Some(v) => Some(v),
        None => {
            println!("invalid http status");
            None
        }
    }
}

fn read_http_pack_body(tls: usize, output: &str) -> Option<(u16, usize)> {
    let mut buf = [0u8; READ_BUF_SIZE];
    let mut header = Vec::new();
    let mut header_match = 0usize;
    let mut header_done = false;
    let mut status = None;
    let mut body_mode = BODY_UNTIL_CLOSE;
    let mut remaining = 0usize;
    let mut chunk_decoder = ChunkDecoder::new();
    let mut writer = PackWriter::open(output)?;
    let mut stream = SidebandStream::new();

    loop {
        let ret = tls_read_with_wait(tls, &mut buf);
        if ret < 0 {
            println!("tls read failed: {} during upload-pack", ret);
            return None;
        }
        if ret == 0 {
            break;
        }

        let got = ret as usize;
        let mut payload_start = 0usize;
        if !header_done {
            let mut i = 0usize;
            while i < got {
                if header.len() >= HTTP_HEADER_LIMIT {
                    println!("http header too large");
                    return None;
                }
                header.push(buf[i]);
                header_match = update_header_match(header_match, buf[i]);
                i += 1;
                if header_match == 4 {
                    header_done = true;
                    payload_start = i;
                    let header_fields = &header[..header.len() - 4];
                    status = parse_status_code(header_fields);
                    if header_has_value(header_fields, b"transfer-encoding", b"chunked") {
                        body_mode = BODY_CHUNKED;
                    } else if let Some(len) = header_usize(header_fields, b"content-length") {
                        body_mode = BODY_CONTENT_LENGTH;
                        remaining = len;
                    }
                    break;
                }
            }
            if !header_done {
                continue;
            }
        }

        let payload = &buf[payload_start..got];
        match body_mode {
            BODY_CHUNKED => {
                if feed_chunked_to_sideband(&mut chunk_decoder, payload, &mut stream, &mut writer)?
                {
                    break;
                }
            }
            BODY_CONTENT_LENGTH => {
                let take = remaining.min(payload.len());
                stream.feed(&payload[..take], &mut writer)?;
                remaining -= take;
                if remaining == 0 {
                    break;
                }
            }
            _ => {
                stream.feed(payload, &mut writer)?;
            }
        }
        if stream.complete {
            break;
        }
    }

    if !header_done {
        println!("incomplete http response");
        return None;
    }
    if body_mode == BODY_CONTENT_LENGTH && remaining != 0 {
        println!("truncated response body");
        return None;
    }
    if body_mode == BODY_CHUNKED && chunk_decoder.state != CHUNK_DONE && !stream.complete {
        println!("truncated chunked response");
        return None;
    }

    let status = match status {
        Some(v) => v,
        None => {
            println!("invalid http status");
            return None;
        }
    };
    let bytes = writer.finish()?;
    Some((status, bytes))
}

fn tls_read_with_wait(tls: usize, buf: &mut [u8]) -> isize {
    let mut idle = 0usize;
    loop {
        let ret = tls_read(tls, buf);
        if ret != EAGAIN_RET && ret != -110 {
            return ret;
        }
        idle += 1;
        if idle >= TLS_READ_IDLE_LIMIT {
            return ret;
        }
        sleep(TLS_READ_IDLE_SLEEP_MS);
    }
}

fn append_body(out: &mut Vec<u8>, bytes: &[u8]) -> bool {
    if out.len() + bytes.len() > MAX_BODY_LEN {
        println!("response body too large");
        return false;
    }
    out.extend_from_slice(bytes);
    true
}

fn feed_chunked(dec: &mut ChunkDecoder, mut input: &[u8], out: &mut Vec<u8>) -> Option<bool> {
    while !input.is_empty() {
        match dec.state {
            CHUNK_SIZE => {
                let b = input[0];
                input = &input[1..];
                if b == b'\n' {
                    let line = if dec.line_len > 0 && dec.line[dec.line_len - 1] == b'\r' {
                        &dec.line[..dec.line_len - 1]
                    } else {
                        &dec.line[..dec.line_len]
                    };
                    let size = match parse_chunk_size(line) {
                        Some(v) => v,
                        None => {
                            println!("invalid chunk size");
                            return None;
                        }
                    };
                    dec.line_len = 0;
                    dec.remaining = size;
                    dec.state = if size == 0 { CHUNK_DONE } else { CHUNK_DATA };
                    if size == 0 {
                        return Some(true);
                    }
                } else {
                    if dec.line_len >= dec.line.len() {
                        println!("chunk size line too large");
                        return None;
                    }
                    dec.line[dec.line_len] = b;
                    dec.line_len += 1;
                }
            }
            CHUNK_DATA => {
                let take = dec.remaining.min(input.len());
                if !append_body(out, &input[..take]) {
                    return None;
                }
                input = &input[take..];
                dec.remaining -= take;
                if dec.remaining == 0 {
                    dec.state = CHUNK_DATA_CR;
                }
            }
            CHUNK_DATA_CR => {
                if input[0] != b'\r' {
                    println!("invalid chunk terminator");
                    return None;
                }
                input = &input[1..];
                dec.state = CHUNK_DATA_LF;
            }
            CHUNK_DATA_LF => {
                if input[0] != b'\n' {
                    println!("invalid chunk terminator");
                    return None;
                }
                input = &input[1..];
                dec.state = CHUNK_SIZE;
            }
            CHUNK_DONE => return Some(true),
            _ => return None,
        }
    }
    Some(dec.state == CHUNK_DONE)
}

fn feed_chunked_to_sideband(
    dec: &mut ChunkDecoder,
    mut input: &[u8],
    stream: &mut SidebandStream,
    writer: &mut PackWriter,
) -> Option<bool> {
    while !input.is_empty() {
        match dec.state {
            CHUNK_SIZE => {
                let b = input[0];
                input = &input[1..];
                if b == b'\n' {
                    let line = if dec.line_len > 0 && dec.line[dec.line_len - 1] == b'\r' {
                        &dec.line[..dec.line_len - 1]
                    } else {
                        &dec.line[..dec.line_len]
                    };
                    let size = match parse_chunk_size(line) {
                        Some(v) => v,
                        None => {
                            println!("invalid chunk size");
                            return None;
                        }
                    };
                    dec.line_len = 0;
                    dec.remaining = size;
                    dec.state = if size == 0 { CHUNK_DONE } else { CHUNK_DATA };
                    if size == 0 {
                        return Some(true);
                    }
                } else {
                    if dec.line_len >= dec.line.len() {
                        println!("chunk size line too large");
                        return None;
                    }
                    dec.line[dec.line_len] = b;
                    dec.line_len += 1;
                }
            }
            CHUNK_DATA => {
                let take = dec.remaining.min(input.len());
                stream.feed(&input[..take], writer)?;
                input = &input[take..];
                dec.remaining -= take;
                if dec.remaining == 0 {
                    dec.state = CHUNK_DATA_CR;
                }
            }
            CHUNK_DATA_CR => {
                if input[0] != b'\r' {
                    println!("invalid chunk terminator");
                    return None;
                }
                input = &input[1..];
                dec.state = CHUNK_DATA_LF;
            }
            CHUNK_DATA_LF => {
                if input[0] != b'\n' {
                    println!("invalid chunk terminator");
                    return None;
                }
                input = &input[1..];
                dec.state = CHUNK_SIZE;
            }
            CHUNK_DONE => return Some(true),
            _ => return None,
        }
    }
    Some(dec.state == CHUNK_DONE)
}

fn header_has_value(header: &[u8], name: &[u8], value: &[u8]) -> bool {
    let mut pos = 0usize;
    while pos < header.len() {
        let line_start = pos;
        while pos < header.len() && header[pos] != b'\n' {
            pos += 1;
        }
        let mut line_end = pos;
        if line_end > line_start && header[line_end - 1] == b'\r' {
            line_end -= 1;
        }
        let line = &header[line_start..line_end];
        if header_line_has_value(line, name, value) {
            return true;
        }
        pos += 1;
    }
    false
}

fn header_usize(header: &[u8], name: &[u8]) -> Option<usize> {
    let mut pos = 0usize;
    while pos < header.len() {
        let line_start = pos;
        while pos < header.len() && header[pos] != b'\n' {
            pos += 1;
        }
        let mut line_end = pos;
        if line_end > line_start && header[line_end - 1] == b'\r' {
            line_end -= 1;
        }
        let line = &header[line_start..line_end];
        if let Some(v) = header_line_usize(line, name) {
            return Some(v);
        }
        pos += 1;
    }
    None
}

fn header_line_has_value(line: &[u8], name: &[u8], value: &[u8]) -> bool {
    let colon = match line.iter().position(|&b| b == b':') {
        Some(v) => v,
        None => return false,
    };
    if !eq_ignore_ascii_case(trim_ascii(&line[..colon]), name) {
        return false;
    }
    contains_token_ignore_ascii_case(&line[colon + 1..], value)
}

fn header_line_usize(line: &[u8], name: &[u8]) -> Option<usize> {
    let colon = line.iter().position(|&b| b == b':')?;
    if !eq_ignore_ascii_case(trim_ascii(&line[..colon]), name) {
        return None;
    }
    parse_usize_bytes(trim_ascii(&line[colon + 1..]))
}

fn parse_usize_bytes(input: &[u8]) -> Option<usize> {
    if input.is_empty() {
        return None;
    }
    let mut out = 0usize;
    for &b in input {
        if !b.is_ascii_digit() {
            return None;
        }
        out = out.checked_mul(10)?.checked_add((b - b'0') as usize)?;
    }
    Some(out)
}

fn parse_chunk_size(line: &[u8]) -> Option<usize> {
    let mut out = 0usize;
    let mut saw_digit = false;
    for &b in line {
        if b == b';' {
            break;
        }
        let v = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            _ => return None,
        };
        saw_digit = true;
        out = out.checked_mul(16)?.checked_add(v as usize)?;
    }
    if saw_digit { Some(out) } else { None }
}

fn trim_ascii(mut s: &[u8]) -> &[u8] {
    while !s.is_empty() && (s[0] == b' ' || s[0] == b'\t') {
        s = &s[1..];
    }
    while !s.is_empty() && (s[s.len() - 1] == b' ' || s[s.len() - 1] == b'\t') {
        s = &s[..s.len() - 1];
    }
    s
}

fn eq_ignore_ascii_case(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b.iter())
            .all(|(&x, &y)| to_ascii_lower(x) == to_ascii_lower(y))
}

fn contains_token_ignore_ascii_case(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    let mut start = 0usize;
    while start < haystack.len() {
        while start < haystack.len()
            && (haystack[start] == b' ' || haystack[start] == b'\t' || haystack[start] == b',')
        {
            start += 1;
        }
        let mut end = start;
        while end < haystack.len() && haystack[end] != b',' {
            end += 1;
        }
        if eq_ignore_ascii_case(trim_ascii(&haystack[start..end]), needle) {
            return true;
        }
        start = end.saturating_add(1);
    }
    false
}

fn to_ascii_lower(b: u8) -> u8 {
    if b.is_ascii_uppercase() { b + 32 } else { b }
}

fn build_upload_pack_command(repo: &str) -> String {
    let mut out = String::new();
    out.push_str("git-upload-pack '");
    out.push_str(repo);
    out.push('\'');
    out
}

fn valid_ssh_repo(repo: &str) -> bool {
    !repo.is_empty()
        && repo
            .bytes()
            .all(|b| b != b'\'' && b != 0 && b != b'\r' && b != b'\n')
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
        buf = &buf[ret as usize..];
    }
    true
}

fn resolve_append(domain: &str, dns: u32, out: &mut Vec<u32>) {
    let mut query = [0u8; 512];
    let qlen = match build_query(domain, &mut query) {
        Some(v) => v,
        None => return,
    };
    let fd = socket(AF_INET, SOCK_DGRAM | SOCK_NONBLOCK, 0);
    if fd < 0 {
        println!("dns socket failed: {}", fd);
        return;
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
        return;
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
            parse_dns_response(&resp, n as usize, out);
            let _ = close(fd);
            return;
        }
        sleep(10);
    }

    println!("dns recv timeout");
    let _ = close(fd);
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

fn parse_dns_response(buf: &[u8], len: usize, out: &mut Vec<u32>) -> bool {
    if len < 12 || buf[0] != (TXID >> 8) as u8 || buf[1] != TXID as u8 {
        return false;
    }
    let flags = ((buf[2] as u16) << 8) | buf[3] as u16;
    if flags & 0x8000 == 0 || flags & 0x000f != 0 {
        return false;
    }
    let qd = ((buf[4] as u16) << 8) | buf[5] as u16;
    let an = ((buf[6] as u16) << 8) | buf[7] as u16;
    let mut p = 12usize;
    for _ in 0..qd {
        p = match skip_name(buf, len, p) {
            Some(v) => v,
            None => return false,
        };
        if p + 4 > len {
            return false;
        }
        p += 4;
    }
    for _ in 0..an {
        p = match skip_name(buf, len, p) {
            Some(v) => v,
            None => return false,
        };
        if p + 10 > len {
            return false;
        }
        let typ = ((buf[p] as u16) << 8) | buf[p + 1] as u16;
        let class = ((buf[p + 2] as u16) << 8) | buf[p + 3] as u16;
        let rdlen = ((buf[p + 8] as u16) << 8) | buf[p + 9] as u16;
        p += 10;
        if p + rdlen as usize > len {
            return false;
        }
        if typ == 1 && class == 1 && rdlen == 4 {
            let ip = ((buf[p] as u32) << 24)
                | ((buf[p + 1] as u32) << 16)
                | ((buf[p + 2] as u32) << 8)
                | buf[p + 3] as u32;
            push_unique_ip(out, ip);
        }
        p += rdlen as usize;
    }
    true
}

fn push_unique_ip(out: &mut Vec<u32>, ip: u32) {
    if out.len() >= MAX_DNS_ADDRS {
        return;
    }
    if !out.iter().any(|&v| v == ip) {
        out.push(ip);
    }
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

fn update_header_match(state: usize, b: u8) -> usize {
    const NEEDLE: [u8; 4] = [b'\r', b'\n', b'\r', b'\n'];
    if b == NEEDLE[state] {
        return state + 1;
    }
    if b == b'\r' { 1 } else { 0 }
}

fn parse_status_code(header: &[u8]) -> Option<u16> {
    if header.len() < 12 || &header[..4] != b"HTTP" {
        return None;
    }
    let mut i = 0usize;
    while i < header.len() && header[i] != b' ' {
        i += 1;
    }
    while i < header.len() && header[i] == b' ' {
        i += 1;
    }
    if i + 3 > header.len() {
        return None;
    }
    let mut code = 0u16;
    for _ in 0..3 {
        let b = header[i];
        if !b.is_ascii_digit() {
            return None;
        }
        code = code * 10 + (b - b'0') as u16;
        i += 1;
    }
    Some(code)
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

fn append_u16_vec(dst: &mut Vec<u8>, value: u16) {
    append_usize_vec(dst, value as usize);
}

fn append_usize_vec(dst: &mut Vec<u8>, mut value: usize) {
    let mut tmp = [0u8; 20];
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
        dst.push(tmp[n]);
    }
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
    if bytes.len() >= prefix.len() && &bytes[..prefix.len()] == prefix {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

fn starts_with(s: &str, prefix: &str) -> bool {
    strip_prefix(s, prefix).is_some()
}

fn starts_with_bytes(input: &[u8], prefix: &[u8]) -> bool {
    input.len() >= prefix.len() && &input[..prefix.len()] == prefix
}

fn find_byte(s: &str, needle: u8) -> Option<usize> {
    for (idx, b) in s.bytes().enumerate() {
        if b == needle {
            return Some(idx);
        }
    }
    None
}

fn print_usage() {
    println!("usage: gitfetch [options] <url> [dns-ip]");
    println!("https: gitfetch https://github.com/user/repo.git");
    println!("ssh:   gitfetch ssh://user@host/repo.git --password PASS");
    println!("ssh:   gitfetch git@github.com:user/repo.git --key /path/id_ed25519");
    println!("ssh:   gitfetch user@host:repo.git --password PASS");
    println!("options:");
    println!("  -h, --help          show this help");
    println!("  -o, --output PATH   save pack path, default /musl/gitfetch.pack");
    println!("      --meta PATH     save selected ref metadata for gitcheckout");
    println!("      --repo DIR      read DIR/.git/HEAD and send it as have");
    println!("      --have OID      send an existing commit oid as have");
    println!("  -d, --dns IP        DNS server, default 10.0.2.3");
    println!("      --ip IP         skip DNS and connect to this IPv4");
    println!("  -p, --port PORT     TCP port, default 443 for HTTPS or 22 for SSH");
    println!("  -u, --user USER     SSH username when not present in URL");
    println!("      --password P    SSH password auth");
    println!("  -i, --key PATH      SSH OpenSSH ed25519 private key auth");
    println!("  -v, --verbose       print HTTP request or SSH exec command");
}
