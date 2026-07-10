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
    AT_FDCWD, OpenFlags, close, connect, mkdir, open, read, recvfrom, sendto, sleep, socket,
    ssh_auth_password, ssh_auth_publickey, ssh_channel_close, ssh_channel_status,
    ssh_channel_try_read, ssh_channel_write, ssh_close, ssh_connect, ssh_exec, write,
};

const AF_INET: i32 = 2;
const SOCK_DGRAM: i32 = 2;
const SOCK_NONBLOCK: i32 = 0o0004000;
const SOCK_STREAM: i32 = 1;
const IPPROTO_TCP: i32 = 6;
const DNS_PORT: u16 = 53;
const DEFAULT_DNS: u32 = 0x0A000203;
const TXID: u16 = 0x4750;
const SSH_PORT: u16 = 22;
const DEFAULT_SSH_IDENT: &str = "SSH-2.0-kairix-gitpush_0.1";
const EAGAIN_RET: isize = -11;
const SSH_IDLE_LIMIT: usize = 1000;
const SSH_IDLE_SLEEP_MS: usize = 10;
const TCP_CONNECT_RETRIES: usize = 3;
const READ_BUF_SIZE: usize = 1024;
const MAX_ARG_LEN: usize = 512;
const MAX_PATH_LEN: usize = 512;
const MAX_CONFIG_LEN: usize = 2048;
const MAX_REF_LEN: usize = 256;
const MAX_OBJECT_FILE_LEN: usize = 1024 * 1024;
const MAX_OBJECT_SIZE: usize = 1024 * 1024;
const MAX_KEY_FILE_LEN: usize = 16 * 1024;
const MAX_DNS_ADDRS: usize = 8;
const INITIAL_REFS: usize = 128;
const MAX_REFS: usize = 4096;
const MAX_CAPS: usize = 64;
const ZERO_OID: &str = "0000000000000000000000000000000000000000";
const TMP_PACK_PATH: &str = "/tmp/gitpush.pack";
const FILE_BUF_SIZE: usize = 4096;
const ZLIB_MAX_BLOCK: usize = 65535;

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
    repo_dir: Option<&'static str>,
    url: Option<&'static str>,
    dns: u32,
    ip_override: Option<u32>,
    port: Option<u16>,
    ssh_user: Option<&'static str>,
    ssh_password: Option<&'static str>,
    ssh_key_path: Option<&'static str>,
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

#[derive(Clone)]
struct ObjectRecord {
    oid: [u8; 20],
    typ: &'static str,
    pack_type: u8,
    body: Vec<u8>,
}

#[derive(Clone, Copy)]
struct ObjectMeta {
    oid: [u8; 20],
    pack_type: u8,
}

struct PackFileWriter {
    fd: usize,
    sha: Sha1State,
    written: usize,
}

struct ZlibStoreWriter<'a> {
    pack: &'a mut PackFileWriter,
    remaining: usize,
    adler_a: u32,
    adler_b: u32,
    finished: bool,
}

#[unsafe(no_mangle)]
pub fn main_with_args(argc: usize, argv: *const usize) -> i32 {
    let cfg = match parse_args(argc, argv) {
        Some(v) => v,
        None => return -1,
    };
    match run_gitpush(&cfg) {
        Some(()) => 0,
        None => -1,
    }
}

fn parse_args(argc: usize, argv: *const usize) -> Option<Config> {
    let mut cfg = Config {
        repo_dir: None,
        url: None,
        dns: DEFAULT_DNS,
        ip_override: None,
        port: None,
        ssh_user: None,
        ssh_password: None,
        ssh_key_path: None,
    };
    let mut i = 1usize;
    while i < argc {
        let arg = argv_str(argv, i)?;
        if arg == "-h" || arg == "--help" {
            print_usage();
            return None;
        } else if arg == "--repo" {
            i += 1;
            cfg.repo_dir = Some(argv_str(argv, i)?);
        } else if let Some(v) = strip_prefix(arg, "--repo=") {
            cfg.repo_dir = Some(v);
        } else if arg == "-d" || arg == "--dns" {
            i += 1;
            cfg.dns = parse_ipv4(argv_str(argv, i)?)?;
        } else if let Some(v) = strip_prefix(arg, "--dns=") {
            cfg.dns = parse_ipv4(v)?;
        } else if arg == "--ip" {
            i += 1;
            cfg.ip_override = Some(parse_ipv4(argv_str(argv, i)?)?);
        } else if let Some(v) = strip_prefix(arg, "--ip=") {
            cfg.ip_override = Some(parse_ipv4(v)?);
        } else if arg == "-p" || arg == "--port" {
            i += 1;
            cfg.port = Some(parse_port(argv_str(argv, i)?)?);
        } else if let Some(v) = strip_prefix(arg, "--port=") {
            cfg.port = Some(parse_port(v)?);
        } else if arg == "-u" || arg == "--user" {
            i += 1;
            cfg.ssh_user = Some(argv_str(argv, i)?);
        } else if let Some(v) = strip_prefix(arg, "--user=") {
            cfg.ssh_user = Some(v);
        } else if arg == "--password" {
            i += 1;
            cfg.ssh_password = Some(argv_str(argv, i)?);
        } else if let Some(v) = strip_prefix(arg, "--password=") {
            cfg.ssh_password = Some(v);
        } else if arg == "--key" || arg == "-i" {
            i += 1;
            cfg.ssh_key_path = Some(argv_str(argv, i)?);
        } else if let Some(v) = strip_prefix(arg, "--key=") {
            cfg.ssh_key_path = Some(v);
        } else if starts_with(arg, "-") {
            println!("unknown option: {}", arg);
            return None;
        } else if cfg.repo_dir.is_none() {
            cfg.repo_dir = Some(arg);
        } else if cfg.url.is_none() {
            cfg.url = Some(arg);
        } else {
            println!("too many arguments");
            return None;
        }
        i += 1;
    }
    if cfg.repo_dir.is_none() {
        cfg.repo_dir = Some(".");
    }
    Some(cfg)
}

fn run_gitpush(cfg: &Config) -> Option<()> {
    let (repo_dir, remote_or_url) = resolve_push_args(cfg)?;
    let git_dir = join_path(&repo_dir, ".git")?;
    let branch = read_head_ref(&git_dir)?;
    let new_oid = read_ref_oid(&git_dir, &branch)?;
    let mut remote_name = String::from("origin");
    let url = match remote_or_url {
        Some(v) if is_git_url(v) => String::from(v),
        Some(v) => {
            remote_name = String::from(v);
            read_remote_url(&git_dir, v)?
        }
        None => read_remote_url(&git_dir, "origin")?,
    };
    if starts_with(&url, "https://") {
        println!("https push is not supported yet; use ssh url");
        return None;
    }
    let target = prepare_ssh_target(cfg, &url)?;
    push_ssh(&git_dir, &remote_name, &target, &branch, &new_oid)
}

fn resolve_push_args(cfg: &Config) -> Option<(String, Option<&'static str>)> {
    let first = cfg.repo_dir.unwrap_or(".");
    if cfg.url.is_some() {
        let repo = user_lib::git::discover_repository(first)?;
        return Some((repo, cfg.url));
    }
    if first == "." {
        return Some((user_lib::git::discover_repository(".")?, None));
    }
    match user_lib::git::discover_repository(first) {
        Some(repo) => Some((repo, None)),
        None => Some((user_lib::git::discover_repository(".")?, Some(first))),
    }
}

fn push_ssh(
    git_dir: &str,
    remote_name: &str,
    target: &SshTarget<'_>,
    branch: &str,
    new_oid: &[u8; 20],
) -> Option<()> {
    let (fd, ip) = open_connected_socket_any(&target.ips, target.port)?;
    print!("gitpush ssh: {}@{} (", target.user, target.host);
    print_ipv4(ip);
    println!(") {}", target.repo);

    let ssh_id = ssh_connect(fd, DEFAULT_SSH_IDENT);
    if ssh_id < 0 {
        println!("ssh connect failed: {}", ssh_id);
        let _ = close(fd);
        return None;
    }
    let ssh_id = ssh_id as usize;
    if !auth_ssh_target(ssh_id, target) {
        let _ = ssh_close(ssh_id);
        let _ = close(fd);
        return None;
    }
    let command = build_receive_pack_command(target.repo);
    let channel_id = ssh_exec(ssh_id, &command);
    if channel_id < 0 {
        println!("ssh exec failed: {}", channel_id);
        let _ = ssh_close(ssh_id);
        let _ = close(fd);
        return None;
    }
    let channel_id = channel_id as usize;
    let advert = read_ssh_advert(ssh_id, channel_id)?;
    let old_oid = find_remote_ref_oid(&advert, branch).unwrap_or_else(|| String::from(ZERO_OID));
    if !verify_remote_tracking_ref(git_dir, remote_name, branch, &old_oid) {
        let _ = ssh_channel_close(ssh_id, channel_id);
        let _ = ssh_close(ssh_id);
        let _ = close(fd);
        return None;
    }
    println!("push {} -> {}", old_oid, branch);
    if old_oid == oid_to_hex(new_oid) {
        if !write_remote_tracking_ref(git_dir, remote_name, branch, new_oid) {
            let _ = ssh_channel_close(ssh_id, channel_id);
            let _ = ssh_close(ssh_id);
            let _ = close(fd);
            return None;
        }
        println!("already up to date");
        println!("gitpush complete");
        let _ = ssh_channel_close(ssh_id, channel_id);
        let _ = ssh_close(ssh_id);
        let _ = close(fd);
        return Some(());
    }
    let stop_oid = parse_stop_oid(&old_oid);
    let objects = collect_push_objects(git_dir, new_oid, stop_oid.as_ref())?;
    let pack_bytes = write_pack_file(git_dir, &objects, TMP_PACK_PATH)?;
    let request = build_push_request(&old_oid, new_oid, branch)?;
    if !write_all_ssh_channel(ssh_id, channel_id, &request)
        || !write_file_to_ssh_channel(ssh_id, channel_id, TMP_PACK_PATH)
    {
        let _ = ssh_channel_close(ssh_id, channel_id);
        let _ = ssh_close(ssh_id);
        let _ = close(fd);
        return None;
    }
    println!("pack bytes: {}", pack_bytes);
    let ok = read_push_report(ssh_id, channel_id);
    let _ = ssh_channel_close(ssh_id, channel_id);
    let _ = ssh_close(ssh_id);
    let _ = close(fd);
    if ok {
        if !write_remote_tracking_ref(git_dir, remote_name, branch, new_oid) {
            return None;
        }
        println!("gitpush complete");
        Some(())
    } else {
        None
    }
}

fn verify_remote_tracking_ref(
    git_dir: &str,
    remote_name: &str,
    branch: &str,
    remote_oid: &str,
) -> bool {
    if remote_oid == ZERO_OID {
        return true;
    }
    let local_oid = match read_remote_tracking_ref(git_dir, remote_name, branch) {
        Some(v) => v,
        None => {
            println!("missing local tracking ref for {}", branch);
            println!("run git fetch or git pull before pushing");
            return false;
        }
    };
    let local_hex = oid_to_hex(&local_oid);
    if local_hex == remote_oid {
        return true;
    }
    println!("push rejected: remote branch changed");
    println!("local origin: {}", local_hex);
    println!("remote:       {}", remote_oid);
    println!("run git fetch or git pull before pushing");
    false
}

fn read_remote_tracking_ref(git_dir: &str, remote_name: &str, branch: &str) -> Option<[u8; 20]> {
    let remote_ref = remote_tracking_ref(remote_name, branch)?;
    let path = join_path(git_dir, &remote_ref)?;
    let data = read_small_file(&path, MAX_REF_LEN)?;
    parse_hex_oid(trim_ascii_str(&data)?.as_bytes())
}

fn write_remote_tracking_ref(
    git_dir: &str,
    remote_name: &str,
    branch: &str,
    oid: &[u8; 20],
) -> bool {
    let Some(remote_ref) = remote_tracking_ref(remote_name, branch) else {
        return false;
    };
    if !mkdir_ref_parents(git_dir, &remote_ref) {
        return false;
    }
    let path = match join_path(git_dir, &remote_ref) {
        Some(v) => v,
        None => return false,
    };
    let mut data = Vec::new();
    data.extend_from_slice(oid_to_hex(oid).as_bytes());
    data.push(b'\n');
    if !write_file(&path, &data) {
        return false;
    }
    println!("updated remote ref: {}", remote_ref);
    true
}

fn remote_tracking_ref(remote_name: &str, branch: &str) -> Option<String> {
    if !is_safe_remote_name(remote_name) {
        return None;
    }
    let branch_name = strip_prefix(branch, "refs/heads/")?;
    if branch_name.is_empty() || branch_name.ends_with('/') {
        return None;
    }
    let mut out = String::new();
    out.push_str("refs/remotes/");
    out.push_str(remote_name);
    out.push('/');
    out.push_str(branch_name);
    if is_safe_remote_ref(&out) {
        Some(out)
    } else {
        None
    }
}

fn build_push_request(old_oid: &str, new_oid: &[u8; 20], branch: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut line = String::new();
    line.push_str(old_oid);
    line.push(' ');
    line.push_str(&oid_to_hex(new_oid));
    line.push(' ');
    line.push_str(branch);
    line.push('\0');
    line.push_str("report-status agent=kairix-gitpush\n");
    encode_pkt_data(line.as_bytes(), &mut out).ok()?;
    encode_pkt_flush(&mut out);
    Some(out)
}

fn read_ssh_advert(ssh_id: usize, channel_id: usize) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = [0u8; READ_BUF_SIZE];
    let mut idle = 0usize;
    loop {
        let n = ssh_channel_try_read(ssh_id, channel_id, &mut buf);
        if n == EAGAIN_RET {
            idle += 1;
            if idle > SSH_IDLE_LIMIT {
                println!("ssh receive-pack refs timeout");
                return None;
            }
            sleep(SSH_IDLE_SLEEP_MS);
            continue;
        }
        idle = 0;
        if n < 0 {
            println!("ssh read failed: {}", n);
            return None;
        }
        if n == 0 {
            return Some(out);
        }
        out.extend_from_slice(&buf[..n as usize]);
        if refs_advertisement_complete(&out) {
            return Some(out);
        }
    }
}

fn read_push_report(ssh_id: usize, channel_id: usize) -> bool {
    let mut pending = Vec::new();
    let mut buf = [0u8; READ_BUF_SIZE];
    let mut idle = 0usize;
    let mut ok = false;
    loop {
        let n = ssh_channel_try_read(ssh_id, channel_id, &mut buf);
        if n == EAGAIN_RET {
            idle += 1;
            if idle >= SSH_IDLE_LIMIT {
                let status = ssh_channel_status(ssh_id, channel_id);
                return ok && status >= 0;
            }
            sleep(SSH_IDLE_SLEEP_MS);
            continue;
        }
        idle = 0;
        if n < 0 {
            println!("ssh report read failed: {}", n);
            return false;
        }
        if n == 0 {
            return ok;
        }
        pending.extend_from_slice(&buf[..n as usize]);
        loop {
            match parse_pkt_line(&pending) {
                Ok((PktLine::Flush, used)) => {
                    pending.drain(0..used);
                    return ok;
                }
                Ok((PktLine::Data(data), used)) => {
                    let payload = data.to_vec();
                    pending.drain(0..used);
                    if starts_with_bytes(&payload, b"unpack ok") {
                        println!("remote: unpack ok");
                    } else if starts_with_bytes(&payload, b"ok ") {
                        print!("remote: ");
                        print_lossy_line(&payload);
                        println!("");
                        ok = true;
                    } else if starts_with_bytes(&payload, b"ng ") {
                        print!("remote reject: ");
                        print_lossy_line(&payload);
                        println!("");
                        return false;
                    } else {
                        print!("remote: ");
                        print_lossy_line(&payload);
                        println!("");
                    }
                }
                Err(PktLineError::Incomplete) => break,
                Err(err) => {
                    println!("invalid push report: {:?}", err);
                    return false;
                }
            }
        }
    }
}

fn find_remote_ref_oid(advert: &[u8], branch: &str) -> Option<String> {
    let mut cap = INITIAL_REFS;
    loop {
        let mut refs = vec![GitRef { oid: "", name: "" }; cap];
        let mut caps = vec![""; MAX_CAPS];
        match parse_ref_advertisement(advert, &mut refs, &mut caps) {
            Ok(parsed) => {
                for r in parsed.refs {
                    if r.name == branch {
                        return Some(String::from(r.oid));
                    }
                }
                return None;
            }
            Err(PktLineError::OutputTooSmall) if cap < MAX_REFS => {
                cap = (cap * 2).min(MAX_REFS);
            }
            Err(err) => {
                println!("parse receive-pack refs failed: {:?}", err);
                return None;
            }
        }
    }
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
            Err(_) => return false,
        }
    }
    false
}

fn collect_push_objects(
    git_dir: &str,
    head_oid: &[u8; 20],
    stop_oid: Option<&[u8; 20]>,
) -> Option<Vec<ObjectMeta>> {
    let mut out = Vec::new();
    collect_commit_chain(git_dir, head_oid, stop_oid, &mut out)?;
    Some(out)
}

fn collect_commit_chain(
    git_dir: &str,
    oid: &[u8; 20],
    stop_oid: Option<&[u8; 20]>,
    out: &mut Vec<ObjectMeta>,
) -> Option<()> {
    if stop_oid == Some(oid) || has_object(out, oid) {
        return Some(());
    }
    let commit = read_object_record(git_dir, oid)?;
    if commit.typ != "commit" {
        println!("push object is not a commit");
        return None;
    }
    let tree_oid = commit_tree_oid(&commit.body)?;
    let parents = commit_parent_oids(&commit.body)?;
    push_unique(out, object_meta(&commit));
    collect_tree_objects(git_dir, &tree_oid, out)?;
    for parent in parents {
        collect_commit_chain(git_dir, &parent, stop_oid, out)?;
    }
    Some(())
}

fn collect_tree_objects(git_dir: &str, oid: &[u8; 20], out: &mut Vec<ObjectMeta>) -> Option<()> {
    if has_object(out, oid) {
        return Some(());
    }
    let tree = read_object_record(git_dir, oid)?;
    push_unique(out, object_meta(&tree));
    let body = tree.body;
    let mut pos = 0usize;
    while pos < body.len() {
        let mode_start = pos;
        while pos < body.len() && body[pos] != b' ' {
            pos += 1;
        }
        if pos >= body.len() {
            return None;
        }
        let mode = core::str::from_utf8(&body[mode_start..pos]).ok();
        pos += 1;
        while pos < body.len() && body[pos] != 0 {
            pos += 1;
        }
        if pos + 21 > body.len() {
            return None;
        }
        pos += 1;
        let mut child = [0u8; 20];
        child.copy_from_slice(&body[pos..pos + 20]);
        pos += 20;
        if mode == Some("40000") {
            collect_tree_objects(git_dir, &child, out)?;
        } else {
            push_unique(out, ObjectMeta {
                oid: child,
                pack_type: 3,
            });
        }
    }
    Some(())
}

fn object_meta(obj: &ObjectRecord) -> ObjectMeta {
    ObjectMeta {
        oid: obj.oid,
        pack_type: obj.pack_type,
    }
}

fn push_unique(out: &mut Vec<ObjectMeta>, obj: ObjectMeta) {
    if !out.iter().any(|existing| existing.oid == obj.oid) {
        out.push(obj);
    }
}

fn has_object(out: &[ObjectMeta], oid: &[u8; 20]) -> bool {
    out.iter().any(|existing| &existing.oid == oid)
}

fn read_object_record(git_dir: &str, oid: &[u8; 20]) -> Option<ObjectRecord> {
    let object = read_loose_object(git_dir, oid)?;
    let nul = find_byte(&object, b'\0')?;
    let header = core::str::from_utf8(&object[..nul]).ok()?;
    let space = header.as_bytes().iter().position(|&b| b == b' ')?;
    let typ = &header[..space];
    let size = parse_usize(&header[space + 1..])?;
    let body = object[nul + 1..].to_vec();
    if body.len() != size {
        return None;
    }
    let pack_type = match typ {
        "commit" => 1,
        "tree" => 2,
        "blob" => 3,
        _ => return None,
    };
    Some(ObjectRecord {
        oid: *oid,
        typ: match typ {
            "commit" => "commit",
            "tree" => "tree",
            _ => "blob",
        },
        pack_type,
        body,
    })
}

fn write_pack_file(git_dir: &str, objects: &[ObjectMeta], path: &str) -> Option<usize> {
    let mut out = PackFileWriter::open(path)?;
    out.write_bytes(b"PACK")?;
    out.write_be_u32(2)?;
    out.write_be_u32(objects.len() as u32)?;
    for obj in objects {
        write_pack_object(git_dir, obj, &mut out)?;
    }
    let bytes = out.finish()?;
    println!("pack objects: {}", objects.len());
    Some(bytes)
}

fn write_pack_object(git_dir: &str, obj: &ObjectMeta, out: &mut PackFileWriter) -> Option<()> {
    stream_loose_body_as_pack_object(git_dir, obj, out)
}

fn append_pack_object_header(out: &mut Vec<u8>, typ: u8, mut size: usize) {
    let mut byte = ((typ & 0x07) << 4) | ((size & 0x0f) as u8);
    size >>= 4;
    if size != 0 {
        byte |= 0x80;
    }
    out.push(byte);
    while size != 0 {
        let mut b = (size & 0x7f) as u8;
        size >>= 7;
        if size != 0 {
            b |= 0x80;
        }
        out.push(b);
    }
}

fn stream_loose_body_as_pack_object(
    git_dir: &str,
    obj: &ObjectMeta,
    out: &mut PackFileWriter,
) -> Option<()> {
    let oid_hex = oid_to_hex(&obj.oid);
    let path = join_path(
        &join_path(&join_path(git_dir, "objects")?, &oid_hex[..2])?,
        &oid_hex[2..],
    )?;
    let fd = open(AT_FDCWD, &path, OpenFlags::RDONLY, 0);
    if fd < 0 {
        println!("open object failed: {}", path);
        return None;
    }
    let fd = fd as usize;
    let ok = stream_loose_fd_body_as_pack_object(fd, obj.pack_type, out);
    let _ = close(fd);
    ok
}

fn stream_loose_fd_body_as_pack_object(
    fd: usize,
    expected_pack_type: u8,
    out: &mut PackFileWriter,
) -> Option<()> {
    let mut zhdr = [0u8; 2];
    read_exact_fd(fd, &mut zhdr)?;
    if zhdr[0] != 0x78 {
        println!("unsupported loose object zlib header");
        return None;
    }

    let mut object_header = [0u8; 64];
    let mut object_header_len = 0usize;
    let mut body: Option<ZlibStoreWriter<'_>> = None;
    let mut buf = [0u8; FILE_BUF_SIZE];

    loop {
        let mut block_header = [0u8; 5];
        read_exact_fd(fd, &mut block_header)?;
        let final_block = block_header[0] & 1 != 0;
        if ((block_header[0] >> 1) & 0x03) != 0 {
            println!("unsupported loose object deflate block");
            return None;
        }
        let len = u16::from_le_bytes([block_header[1], block_header[2]]) as usize;
        let nlen = u16::from_le_bytes([block_header[3], block_header[4]]);
        if nlen != !(len as u16) {
            println!("invalid loose object zlib block");
            return None;
        }

        let mut left = len;
        while left > 0 {
            let chunk_len = left.min(buf.len());
            read_exact_fd(fd, &mut buf[..chunk_len])?;
            left -= chunk_len;

            if let Some(writer) = body.as_mut() {
                writer.write_chunk(&buf[..chunk_len])?;
                continue;
            }

            let mut pos = 0usize;
            while pos < chunk_len && buf[pos] != 0 {
                if object_header_len >= object_header.len() {
                    println!("loose object header too long");
                    return None;
                }
                object_header[object_header_len] = buf[pos];
                object_header_len += 1;
                pos += 1;
            }
            if pos == chunk_len {
                continue;
            }

            let (pack_type, body_size) = parse_loose_header(&object_header[..object_header_len])?;
            if pack_type != expected_pack_type {
                println!("push object type mismatch");
                return None;
            }
            let mut pack_header = Vec::new();
            append_pack_object_header(&mut pack_header, pack_type, body_size);
            out.write_bytes(&pack_header)?;
            let mut writer = ZlibStoreWriter::open(out, body_size)?;
            pos += 1;
            if pos < chunk_len {
                writer.write_chunk(&buf[pos..chunk_len])?;
            }
            body = Some(writer);
        }

        if final_block {
            break;
        }
    }

    let Some(writer) = body else {
        println!("missing loose object body");
        return None;
    };
    writer.finish()
}

fn parse_loose_header(input: &[u8]) -> Option<(u8, usize)> {
    let header = core::str::from_utf8(input).ok()?;
    let space = find_byte(header.as_bytes(), b' ')?;
    let typ = &header[..space];
    let size = parse_usize(&header[space + 1..])?;
    let pack_type = match typ {
        "commit" => 1,
        "tree" => 2,
        "blob" => 3,
        _ => return None,
    };
    Some((pack_type, size))
}

fn commit_tree_oid(data: &[u8]) -> Option<[u8; 20]> {
    let prefix = b"tree ";
    if data.len() < prefix.len() + 40 || &data[..prefix.len()] != prefix {
        return None;
    }
    parse_hex_oid(&data[prefix.len()..prefix.len() + 40])
}

fn commit_parent_oids(data: &[u8]) -> Option<Vec<[u8; 20]>> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < data.len() {
        let start = pos;
        while pos < data.len() && data[pos] != b'\n' {
            pos += 1;
        }
        let line = &data[start..pos];
        if line.is_empty() {
            break;
        }
        if starts_with_bytes(line, b"parent ") {
            out.push(parse_hex_oid(&line[7..])?);
        }
        if pos < data.len() {
            pos += 1;
        }
    }
    Some(out)
}

fn parse_stop_oid(input: &str) -> Option<[u8; 20]> {
    if input == ZERO_OID {
        None
    } else {
        parse_hex_oid(input.as_bytes())
    }
}

fn read_loose_object(git_dir: &str, oid: &[u8; 20]) -> Option<Vec<u8>> {
    let oid_hex = oid_to_hex(oid);
    let path = join_path(
        &join_path(&join_path(git_dir, "objects")?, &oid_hex[..2])?,
        &oid_hex[2..],
    )?;
    let compressed = read_small_file(&path, MAX_OBJECT_FILE_LEN)?;
    let mut out = Vec::new();
    inflate_zlib_stored(&compressed, &mut out)?;
    Some(out)
}

fn read_head_ref(git_dir: &str) -> Option<String> {
    let head_path = join_path(git_dir, "HEAD")?;
    let head_data = read_small_file(&head_path, MAX_REF_LEN)?;
    let head = trim_ascii_str(&head_data)?;
    if let Some(ref_name) = strip_prefix(head, "ref: ") {
        if is_safe_ref_name(ref_name) {
            return Some(String::from(ref_name));
        }
    }
    println!("detached HEAD push is not supported");
    None
}

fn read_ref_oid(git_dir: &str, ref_name: &str) -> Option<[u8; 20]> {
    let path = join_path(git_dir, ref_name)?;
    let data = read_small_file(&path, MAX_REF_LEN)?;
    parse_hex_oid(trim_ascii_str(&data)?.as_bytes())
}

fn read_remote_url(git_dir: &str, remote: &str) -> Option<String> {
    if !is_safe_remote_name(remote) {
        println!("invalid remote name: {}", remote);
        return None;
    }
    let path = join_path(git_dir, "config")?;
    let data = read_small_file(&path, MAX_CONFIG_LEN)?;
    let text = core::str::from_utf8(&data).ok()?;
    let mut header = String::new();
    header.push_str("[remote \"");
    header.push_str(remote);
    header.push_str("\"]");
    let mut in_remote = false;
    for raw in text.lines() {
        let line = trim_ascii(raw);
        if starts_with(line, "[") {
            in_remote = line == header;
        } else if in_remote && starts_with(line, "url") {
            if let Some(eq) = line.as_bytes().iter().position(|&b| b == b'=') {
                return Some(String::from(trim_ascii(&line[eq + 1..])));
            }
        }
    }
    println!("remote not found: {}", remote);
    None
}

fn prepare_ssh_target<'a>(cfg: &'a Config, url: &'a str) -> Option<SshTarget<'a>> {
    let mut user = cfg.ssh_user;
    let password = cfg.ssh_password;
    let key_path = cfg.ssh_key_path;
    let mut host;
    let repo;
    let mut port = cfg.port.unwrap_or(SSH_PORT);
    if let Some(rest) = strip_prefix(url, "ssh://") {
        let slash = find_byte(rest.as_bytes(), b'/')?;
        let mut authority = &rest[..slash];
        repo = &rest[slash..];
        if let Some(at) = find_byte(authority.as_bytes(), b'@') {
            if user.is_none() {
                user = Some(&authority[..at]);
            }
            authority = &authority[at + 1..];
        }
        host = authority;
        if let Some(colon) = find_byte(host.as_bytes(), b':') {
            if cfg.port.is_none() {
                port = parse_port(&host[colon + 1..])?;
            }
            host = &host[..colon];
        }
    } else if let Some(at) = find_byte(url.as_bytes(), b'@') {
        let after_user = &url[at + 1..];
        let colon = find_byte(after_user.as_bytes(), b':')?;
        if user.is_none() {
            user = Some(&url[..at]);
        }
        host = &after_user[..colon];
        repo = &after_user[colon + 1..];
    } else {
        println!("push only supports ssh URLs for now");
        return None;
    }
    let user = user?;
    if password.is_none() && key_path.is_none() {
        println!("missing ssh auth; use --key or --password");
        return None;
    }
    let ips = resolve_target_ips(host, cfg)?;
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

fn auth_ssh_target(ssh_id: usize, target: &SshTarget<'_>) -> bool {
    if let Some(path) = target.key_path {
        let key = match read_small_file(path, MAX_KEY_FILE_LEN) {
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

fn resolve_append(host: &str, dns: u32, out: &mut Vec<u32>) {
    if let Some(ip) = dns_query(host, dns) {
        if !out.contains(&ip) {
            out.push(ip);
        }
    }
}

fn dns_query(host: &str, dns: u32) -> Option<u32> {
    let fd = socket(AF_INET, SOCK_DGRAM | SOCK_NONBLOCK, 0);
    if fd < 0 {
        return None;
    }
    let fd = fd as usize;
    let server = SockAddrIn::new(dns, DNS_PORT);
    let mut query = [0u8; 256];
    let qlen = build_dns_query(host, &mut query)?;
    let sent = sendto(
        fd,
        query.as_ptr(),
        qlen,
        0,
        &server as *const SockAddrIn as *const u8,
        core::mem::size_of::<SockAddrIn>(),
    );
    if sent < 0 {
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
            let parsed = parse_dns_response(&resp[..n as usize]);
            let _ = close(fd);
            return parsed;
        }
        sleep(10);
    }
    let _ = close(fd);
    None
}

fn build_dns_query(host: &str, out: &mut [u8]) -> Option<usize> {
    if out.len() < 12 {
        return None;
    }
    out[..12].copy_from_slice(&[0x47, 0x50, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]);
    let mut pos = 12usize;
    for label in host.split('.') {
        if label.is_empty() || label.len() > 63 || pos + 1 + label.len() >= out.len() {
            return None;
        }
        out[pos] = label.len() as u8;
        pos += 1;
        out[pos..pos + label.len()].copy_from_slice(label.as_bytes());
        pos += label.len();
    }
    if pos + 5 > out.len() {
        return None;
    }
    out[pos] = 0;
    pos += 1;
    out[pos..pos + 4].copy_from_slice(&[0, 1, 0, 1]);
    Some(pos + 4)
}

fn parse_dns_response(resp: &[u8]) -> Option<u32> {
    if resp.len() < 12 || resp[0] != ((TXID >> 8) as u8) || resp[1] != (TXID as u8) {
        return None;
    }
    let qd = u16::from_be_bytes([resp[4], resp[5]]) as usize;
    let an = u16::from_be_bytes([resp[6], resp[7]]) as usize;
    let mut pos = 12usize;
    for _ in 0..qd {
        skip_dns_name(resp, &mut pos)?;
        pos += 4;
    }
    for _ in 0..an {
        skip_dns_name(resp, &mut pos)?;
        if pos + 10 > resp.len() {
            return None;
        }
        let typ = u16::from_be_bytes([resp[pos], resp[pos + 1]]);
        let class = u16::from_be_bytes([resp[pos + 2], resp[pos + 3]]);
        let len = u16::from_be_bytes([resp[pos + 8], resp[pos + 9]]) as usize;
        pos += 10;
        if pos + len > resp.len() {
            return None;
        }
        if typ == 1 && class == 1 && len == 4 {
            return Some(
                ((resp[pos] as u32) << 24)
                    | ((resp[pos + 1] as u32) << 16)
                    | ((resp[pos + 2] as u32) << 8)
                    | resp[pos + 3] as u32,
            );
        }
        pos += len;
    }
    None
}

fn skip_dns_name(buf: &[u8], pos: &mut usize) -> Option<()> {
    loop {
        if *pos >= buf.len() {
            return None;
        }
        let len = buf[*pos];
        *pos += 1;
        if len & 0xc0 == 0xc0 {
            *pos += 1;
            return Some(());
        }
        if len == 0 {
            return Some(());
        }
        *pos += len as usize;
    }
}

fn open_connected_socket_any(ips: &[u32], port: u16) -> Option<(usize, u32)> {
    for &ip in ips {
        if let Some(fd) = open_connected_socket(ip, port) {
            return Some((fd, ip));
        }
    }
    None
}

fn open_connected_socket(ip: u32, port: u16) -> Option<usize> {
    let addr = SockAddrIn::new(ip, port);
    for attempt in 0..TCP_CONNECT_RETRIES {
        let fd = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        if fd < 0 {
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
        let _ = close(fd);
        if attempt + 1 < TCP_CONNECT_RETRIES {
            sleep(200);
        }
    }
    None
}

fn write_all_ssh_channel(ssh_id: usize, channel_id: usize, mut buf: &[u8]) -> bool {
    while !buf.is_empty() {
        let n = ssh_channel_write(ssh_id, channel_id, buf);
        if n <= 0 {
            println!("ssh channel write failed: {}", n);
            return false;
        }
        buf = &buf[n as usize..];
    }
    true
}

fn write_file_to_ssh_channel(ssh_id: usize, channel_id: usize, path: &str) -> bool {
    let fd = open(AT_FDCWD, path, OpenFlags::RDONLY, 0);
    if fd < 0 {
        println!("open pack failed: {}", path);
        return false;
    }
    let fd = fd as usize;
    let mut buf = [0u8; FILE_BUF_SIZE];
    loop {
        let n = read(fd, &mut buf);
        if n < 0 {
            println!("read pack failed: {}", n);
            let _ = close(fd);
            return false;
        }
        if n == 0 {
            break;
        }
        if !write_all_ssh_channel(ssh_id, channel_id, &buf[..n as usize]) {
            let _ = close(fd);
            return false;
        }
    }
    let _ = close(fd);
    true
}

fn build_receive_pack_command(repo: &str) -> String {
    let mut out = String::new();
    out.push_str("git-receive-pack '");
    for c in repo.chars() {
        if c != '\'' {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

fn read_small_file(path: &str, max_len: usize) -> Option<Vec<u8>> {
    let fd = open(AT_FDCWD, path, OpenFlags::RDONLY, 0);
    if fd < 0 {
        return None;
    }
    let fd = fd as usize;
    let mut out = Vec::new();
    let mut buf = [0u8; 2048];
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

fn write_file(path: &str, data: &[u8]) -> bool {
    let fd = open(
        AT_FDCWD,
        path,
        OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::WRONLY,
        0o644,
    );
    if fd < 0 {
        println!("open output failed: {}", path);
        return false;
    }
    let fd = fd as usize;
    let mut written = 0usize;
    while written < data.len() {
        let n = write(fd, &data[written..]);
        if n <= 0 {
            let _ = close(fd);
            println!("write failed: {}", path);
            return false;
        }
        written += n as usize;
    }
    let _ = close(fd);
    true
}

impl PackFileWriter {
    fn open(path: &str) -> Option<Self> {
        let fd = open(
            AT_FDCWD,
            path,
            OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::WRONLY,
            0o644,
        );
        if fd < 0 {
            println!("open pack output failed: {}", path);
            return None;
        }
        Some(Self {
            fd: fd as usize,
            sha: Sha1State::new(),
            written: 0,
        })
    }

    fn write_bytes(&mut self, data: &[u8]) -> Option<()> {
        if !write_all_fd(self.fd, data) {
            return None;
        }
        self.sha.update(data);
        self.written += data.len();
        Some(())
    }

    fn write_be_u32(&mut self, value: u32) -> Option<()> {
        self.write_bytes(&value.to_be_bytes())
    }

    fn finish(mut self) -> Option<usize> {
        let trailer = core::mem::replace(&mut self.sha, Sha1State::new()).finish();
        if !write_all_fd(self.fd, &trailer) {
            return None;
        }
        self.written += trailer.len();
        let _ = close(self.fd);
        self.fd = usize::MAX;
        Some(self.written)
    }
}

impl Drop for PackFileWriter {
    fn drop(&mut self) {
        if self.fd != usize::MAX {
            let _ = close(self.fd);
        }
    }
}

impl<'a> ZlibStoreWriter<'a> {
    fn open(pack: &'a mut PackFileWriter, size: usize) -> Option<Self> {
        pack.write_bytes(&[0x78, 0x01])?;
        Some(Self {
            pack,
            remaining: size,
            adler_a: 1,
            adler_b: 0,
            finished: false,
        })
    }

    fn write_chunk(&mut self, data: &[u8]) -> Option<()> {
        let mut pos = 0usize;
        while pos < data.len() {
            if self.remaining == 0 {
                println!("object body too large");
                return None;
            }
            let chunk_len = (data.len() - pos).min(self.remaining).min(ZLIB_MAX_BLOCK);
            let final_block = self.remaining == chunk_len;
            let len = chunk_len as u16;
            let nlen = !len;
            let header = [
                if final_block { 1 } else { 0 },
                (len & 0xff) as u8,
                (len >> 8) as u8,
                (nlen & 0xff) as u8,
                (nlen >> 8) as u8,
            ];
            let chunk = &data[pos..pos + chunk_len];
            update_adler32(&mut self.adler_a, &mut self.adler_b, chunk);
            self.pack.write_bytes(&header)?;
            self.pack.write_bytes(chunk)?;
            self.remaining -= chunk_len;
            pos += chunk_len;
        }
        Some(())
    }

    fn finish(mut self) -> Option<()> {
        if self.remaining != 0 {
            println!("short object body");
            return None;
        }
        if self.adler_a == 1 && self.adler_b == 0 {
            self.pack.write_bytes(&[1, 0, 0, 0xff, 0xff])?;
        }
        let sum = (self.adler_b << 16) | self.adler_a;
        self.pack.write_bytes(&sum.to_be_bytes())?;
        self.finished = true;
        Some(())
    }
}

fn write_all_fd(fd: usize, data: &[u8]) -> bool {
    let mut written = 0usize;
    while written < data.len() {
        let n = write(fd, &data[written..]);
        if n <= 0 {
            println!("write failed: {}", n);
            return false;
        }
        written += n as usize;
    }
    true
}

fn read_exact_fd(fd: usize, out: &mut [u8]) -> Option<()> {
    let mut got = 0usize;
    while got < out.len() {
        let n = read(fd, &mut out[got..]);
        if n <= 0 {
            return None;
        }
        got += n as usize;
    }
    Some(())
}

fn mkdir_ref_parents(git_dir: &str, ref_name: &str) -> bool {
    let bytes = ref_name.as_bytes();
    let mut path = String::new();
    path.push_str(git_dir);
    let mut start = 0usize;
    while start < bytes.len() {
        let mut end = start;
        while end < bytes.len() && bytes[end] != b'/' {
            end += 1;
        }
        if end == bytes.len() {
            return true;
        }
        path.push('/');
        let Some(part) = core::str::from_utf8(&bytes[start..end]).ok() else {
            return false;
        };
        path.push_str(part);
        let _ = mkdir(&path, 0o755);
        start = end + 1;
    }
    true
}

fn inflate_zlib_stored(input: &[u8], out: &mut Vec<u8>) -> Option<()> {
    if input.len() < 6 || input[0] != 0x78 {
        return None;
    }
    let mut pos = 2usize;
    loop {
        let header = *input.get(pos)?;
        pos += 1;
        let final_block = header & 1 != 0;
        if ((header >> 1) & 0x03) != 0 {
            return None;
        }
        if pos + 4 > input.len() {
            return None;
        }
        let len = u16::from_le_bytes([input[pos], input[pos + 1]]) as usize;
        let nlen = u16::from_le_bytes([input[pos + 2], input[pos + 3]]);
        if nlen != !(len as u16) {
            return None;
        }
        pos += 4;
        if pos + len > input.len() || out.len() + len > MAX_OBJECT_SIZE {
            return None;
        }
        out.extend_from_slice(&input[pos..pos + len]);
        pos += len;
        if final_block {
            break;
        }
    }
    Some(())
}

fn update_adler32(a: &mut u32, b: &mut u32, data: &[u8]) {
    const MOD: u32 = 65521;
    for &byte in data {
        *a = (*a + byte as u32) % MOD;
        *b = (*b + *a) % MOD;
    }
}

fn parse_usize(input: &str) -> Option<usize> {
    let mut out = 0usize;
    for b in input.bytes() {
        if !b.is_ascii_digit() {
            return None;
        }
        out = out.checked_mul(10)?.checked_add((b - b'0') as usize)?;
    }
    Some(out)
}

fn parse_hex_oid(input: &[u8]) -> Option<[u8; 20]> {
    if input.len() != 40 {
        return None;
    }
    let mut out = [0u8; 20];
    for i in 0..20 {
        out[i] = (hex_value(input[i * 2])? << 4) | hex_value(input[i * 2 + 1])?;
    }
    Some(out)
}

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn oid_to_hex(oid: &[u8; 20]) -> String {
    let mut out = String::new();
    for &b in oid {
        push_hex_byte(&mut out, b);
    }
    out
}

fn push_hex_byte(out: &mut String, b: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push(HEX[(b >> 4) as usize] as char);
    out.push(HEX[(b & 0x0f) as usize] as char);
}

struct Sha1State {
    h0: u32,
    h1: u32,
    h2: u32,
    h3: u32,
    h4: u32,
    total_len: u64,
    block: [u8; 64],
    block_len: usize,
}

impl Sha1State {
    fn new() -> Self {
        Self {
            h0: 0x67452301,
            h1: 0xefcdab89,
            h2: 0x98badcfe,
            h3: 0x10325476,
            h4: 0xc3d2e1f0,
            total_len: 0,
            block: [0u8; 64],
            block_len: 0,
        }
    }

    fn update(&mut self, data: &[u8]) {
        self.total_len += data.len() as u64;
        for &byte in data {
            self.block[self.block_len] = byte;
            self.block_len += 1;
            if self.block_len == 64 {
                sha1_process_block(
                    &self.block,
                    &mut self.h0,
                    &mut self.h1,
                    &mut self.h2,
                    &mut self.h3,
                    &mut self.h4,
                );
                self.block_len = 0;
            }
        }
    }

    fn finish(mut self) -> [u8; 20] {
        let bit_len = self.total_len * 8;
        self.block[self.block_len] = 0x80;
        self.block_len += 1;
        if self.block_len > 56 {
            for b in self.block.iter_mut().skip(self.block_len) {
                *b = 0;
            }
            sha1_process_block(
                &self.block,
                &mut self.h0,
                &mut self.h1,
                &mut self.h2,
                &mut self.h3,
                &mut self.h4,
            );
            self.block_len = 0;
        }
        for b in self.block.iter_mut().take(56).skip(self.block_len) {
            *b = 0;
        }
        self.block[56..64].copy_from_slice(&bit_len.to_be_bytes());
        sha1_process_block(
            &self.block,
            &mut self.h0,
            &mut self.h1,
            &mut self.h2,
            &mut self.h3,
            &mut self.h4,
        );
        let mut out = [0u8; 20];
        out[..4].copy_from_slice(&self.h0.to_be_bytes());
        out[4..8].copy_from_slice(&self.h1.to_be_bytes());
        out[8..12].copy_from_slice(&self.h2.to_be_bytes());
        out[12..16].copy_from_slice(&self.h3.to_be_bytes());
        out[16..20].copy_from_slice(&self.h4.to_be_bytes());
        out
    }
}

fn sha1_process_block(
    chunk: &[u8; 64],
    h0: &mut u32,
    h1: &mut u32,
    h2: &mut u32,
    h3: &mut u32,
    h4: &mut u32,
) {
    let mut w = [0u32; 80];
    for i in 0..16 {
        let j = i * 4;
        w[i] = ((chunk[j] as u32) << 24)
            | ((chunk[j + 1] as u32) << 16)
            | ((chunk[j + 2] as u32) << 8)
            | chunk[j + 3] as u32;
    }
    for i in 16..80 {
        w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
    }
    let mut a = *h0;
    let mut b = *h1;
    let mut c = *h2;
    let mut d = *h3;
    let mut e = *h4;
    for (i, &wi) in w.iter().enumerate() {
        let (f, k) = match i {
            0..=19 => ((b & c) | ((!b) & d), 0x5a827999),
            20..=39 => (b ^ c ^ d, 0x6ed9eba1),
            40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1bbcdc),
            _ => (b ^ c ^ d, 0xca62c1d6),
        };
        let temp = a
            .rotate_left(5)
            .wrapping_add(f)
            .wrapping_add(e)
            .wrapping_add(k)
            .wrapping_add(wi);
        e = d;
        d = c;
        c = b.rotate_left(30);
        b = a;
        a = temp;
    }
    *h0 = (*h0).wrapping_add(a);
    *h1 = (*h1).wrapping_add(b);
    *h2 = (*h2).wrapping_add(c);
    *h3 = (*h3).wrapping_add(d);
    *h4 = (*h4).wrapping_add(e);
}

fn join_path(parent: &str, name: &str) -> Option<String> {
    if parent.len() + name.len() + 2 > MAX_PATH_LEN {
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

fn trim_ascii_str(input: &[u8]) -> Option<&str> {
    core::str::from_utf8(input).ok().map(trim_ascii)
}

fn trim_ascii(input: &str) -> &str {
    let mut start = 0usize;
    let mut end = input.len();
    let bytes = input.as_bytes();
    while start < end && bytes[start].is_ascii_whitespace() {
        start += 1;
    }
    while end > start && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    &input[start..end]
}

fn is_safe_ref_name(input: &str) -> bool {
    starts_with(input, "refs/heads/") && !input.ends_with('/') && !input.contains("..")
}

fn is_safe_remote_ref(input: &str) -> bool {
    if !starts_with(input, "refs/remotes/") || input.ends_with('/') || input.contains("..") {
        return false;
    }
    for &b in input.as_bytes() {
        if b == b'\\' || b == 0 || b <= b' ' {
            return false;
        }
    }
    true
}

fn is_safe_remote_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && !name.contains('/')
        && !name.contains("..")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn is_git_url(input: &str) -> bool {
    starts_with(input, "ssh://")
        || starts_with(input, "https://")
        || (input.contains('@') && input.contains(':'))
}

fn parse_ipv4(s: &str) -> Option<u32> {
    let mut ip = 0u32;
    let mut parts = 0usize;
    for part in s.split('.') {
        let n = parse_port(part)?;
        if n > 255 {
            return None;
        }
        ip = (ip << 8) | n as u32;
        parts += 1;
    }
    if parts == 4 { Some(ip) } else { None }
}

fn parse_port(s: &str) -> Option<u16> {
    let mut out = 0u32;
    if s.is_empty() {
        return None;
    }
    for b in s.bytes() {
        if !b.is_ascii_digit() {
            return None;
        }
        out = out * 10 + (b - b'0') as u32;
        if out > u16::MAX as u32 {
            return None;
        }
    }
    Some(out as u16)
}

fn find_byte(input: &[u8], byte: u8) -> Option<usize> {
    input.iter().position(|&b| b == byte)
}

fn starts_with(s: &str, prefix: &str) -> bool {
    strip_prefix(s, prefix).is_some()
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

fn starts_with_bytes(input: &[u8], prefix: &[u8]) -> bool {
    input.len() >= prefix.len() && &input[..prefix.len()] == prefix
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

fn argv_str(argv: *const usize, idx: usize) -> Option<&'static str> {
    cstr_to_str(unsafe { *argv.add(idx) as *const u8 })
}

fn cstr_to_str(ptr: *const u8) -> Option<&'static str> {
    if ptr.is_null() {
        return None;
    }
    let mut len = 0usize;
    unsafe {
        while *ptr.add(len) != 0 {
            len += 1;
            if len > MAX_ARG_LEN {
                return None;
            }
        }
        core::str::from_utf8(core::slice::from_raw_parts(ptr, len)).ok()
    }
}

fn print_usage() {
    println!("usage: git push [repo-dir] [remote-or-ssh-url] --key PATH");
    println!("       cd repo; git push me --key PATH");
}
