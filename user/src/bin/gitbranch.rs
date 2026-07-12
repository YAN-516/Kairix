#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use alloc::{string::String, vec::Vec};
use user_lib::{AT_FDCWD, OpenFlags, close, getdents64, open, read, unlinkat, write};

const DEFAULT_REPO: &str = ".";
const MAX_ARG_LEN: usize = 512;
const MAX_PATH_LEN: usize = 512;

struct Config {
    repo_dir: &'static str,
    remotes: bool,
    all: bool,
    verbose: bool,
    create: Option<&'static str>,
    start: Option<&'static str>,
    delete: Option<&'static str>,
    force_delete: bool,
}

#[unsafe(no_mangle)]
pub fn main_with_args(argc: usize, argv: *const usize) -> i32 {
    let cfg = match parse_args(argc, argv) {
        Some(v) => v,
        None => return -1,
    };
    match run_gitbranch(&cfg) {
        Some(()) => 0,
        None => -1,
    }
}

fn parse_args(argc: usize, argv: *const usize) -> Option<Config> {
    let mut cfg = Config {
        repo_dir: DEFAULT_REPO,
        remotes: false,
        all: false,
        verbose: false,
        create: None,
        start: None,
        delete: None,
        force_delete: false,
    };
    let mut repo_set = false;
    let mut pos1 = None;
    let mut pos2 = None;
    let mut i = 1usize;
    while i < argc {
        let arg = argv_str(argv, i)?;
        if arg == "-h" || arg == "--help" {
            print_usage();
            return None;
        } else if arg == "-r" || arg == "--remotes" {
            cfg.remotes = true;
        } else if arg == "-a" || arg == "--all" {
            cfg.all = true;
        } else if arg == "-vv" {
            cfg.verbose = true;
        } else if arg == "-d" || arg == "--delete" {
            i += 1;
            if i >= argc {
                println!("missing branch name for {}", arg);
                return None;
            }
            cfg.delete = argv_str(argv, i);
            cfg.force_delete = false;
        } else if arg == "-D" {
            i += 1;
            if i >= argc {
                println!("missing branch name for -D");
                return None;
            }
            cfg.delete = argv_str(argv, i);
            cfg.force_delete = true;
        } else if arg == "--repo" {
            i += 1;
            if i >= argc {
                println!("missing value for --repo");
                return None;
            }
            cfg.repo_dir = argv_str(argv, i)?;
            repo_set = true;
        } else if let Some(v) = strip_prefix(arg, "--repo=") {
            cfg.repo_dir = v;
            repo_set = true;
        } else if starts_with(arg, "-") {
            println!("unknown option: {}", arg);
            return None;
        } else if pos1.is_none() {
            pos1 = Some(arg);
        } else if pos2.is_none() {
            pos2 = Some(arg);
        } else {
            println!("too many arguments");
            return None;
        }
        i += 1;
    }
    if cfg.delete.is_some() {
        if pos1.is_some() || pos2.is_some() || cfg.remotes || cfg.all {
            println!("branch delete does not take extra branch list arguments");
            return None;
        }
        return Some(cfg);
    }
    if let Some(first) = pos1 {
        if pos2.is_none() && !repo_set && user_lib::git::discover_repository(first).is_some() {
            cfg.repo_dir = first;
        } else {
            cfg.create = Some(first);
            cfg.start = pos2;
        }
    }
    Some(cfg)
}

fn run_gitbranch(cfg: &Config) -> Option<()> {
    let repo = match user_lib::git::discover_repository(cfg.repo_dir) {
        Some(v) => v,
        None => {
            println!("not a git repository: {}", cfg.repo_dir);
            return None;
        }
    };
    let git_dir = join_path(&repo, ".git")?;
    let current = current_branch(&git_dir);

    if let Some(name) = cfg.delete {
        delete_branch(&git_dir, current.as_deref(), name, cfg.force_delete)?;
        return Some(());
    }

    if let Some(name) = cfg.create {
        create_branch(&git_dir, name, cfg.start)?;
        return Some(());
    }

    if !cfg.remotes {
        let heads_dir = join_path(&git_dir, "refs/heads")?;
        let mut branches = Vec::new();
        collect_branch_names(&heads_dir, "", &mut branches)?;
        branches.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        for branch in branches {
            print_branch_line(
                &git_dir,
                &branch,
                false,
                current.as_deref() == Some(branch.as_str()),
                cfg.verbose,
            )?;
        }
    }

    if cfg.remotes || cfg.all {
        let remotes_dir = join_path(&git_dir, "refs/remotes")?;
        let mut branches = Vec::new();
        collect_branch_names(&remotes_dir, "", &mut branches)?;
        branches.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        for branch in branches {
            if cfg.all {
                let mut display = String::from("remotes/");
                display.push_str(&branch);
                print_branch_line(&git_dir, &display, true, false, cfg.verbose)?;
            } else {
                print_branch_line(&git_dir, &branch, true, false, cfg.verbose)?;
            }
        }
    }

    Some(())
}

fn current_branch(git_dir: &str) -> Option<String> {
    let head_path = join_path(git_dir, "HEAD")?;
    let head_data = read_small_file(&head_path, 256)?;
    let head = trim_ascii_str(&head_data)?;
    strip_prefix(head, "ref: refs/heads/").map(String::from)
}

fn create_branch(git_dir: &str, name: &str, start: Option<&str>) -> Option<()> {
    if !is_safe_branch_name(name) {
        println!("invalid branch name: {}", name);
        return None;
    }
    let branch_ref = make_head_ref(name)?;
    if read_ref_oid(git_dir, &branch_ref).is_some() {
        println!("branch already exists: {}", name);
        return None;
    }
    let oid = resolve_start_oid(git_dir, start)?;
    if !mkdir_ref_parents(git_dir, &branch_ref) {
        return None;
    }
    let path = join_path(git_dir, &branch_ref)?;
    let mut data = Vec::new();
    append_oid_hex(&mut data, &oid);
    data.push(b'\n');
    if !write_file(&path, &data) {
        return None;
    }
    println!("created branch '{}'", name);
    Some(())
}

fn delete_branch(git_dir: &str, current: Option<&str>, name: &str, _force: bool) -> Option<()> {
    let branch = normalize_branch_arg(name);
    if !is_safe_branch_name(branch) {
        println!("invalid branch name: {}", name);
        return None;
    }
    if current == Some(branch) {
        println!("cannot delete current branch: {}", branch);
        return None;
    }
    let branch_ref = make_head_ref(branch)?;
    if read_ref_oid(git_dir, &branch_ref).is_none() {
        println!("branch not found: {}", branch);
        return None;
    }
    let path = join_path(git_dir, &branch_ref)?;
    if unlinkat(AT_FDCWD, &path, 0) < 0 {
        println!("delete branch failed: {}", branch);
        return None;
    }
    println!("deleted branch {}", branch);
    Some(())
}

fn resolve_start_oid(git_dir: &str, start: Option<&str>) -> Option<[u8; 20]> {
    let Some(start) = start else {
        return read_head_oid(git_dir);
    };
    if let Some(oid) = parse_hex_oid(start.as_bytes()) {
        return Some(oid);
    }
    if starts_with(start, "refs/") {
        if let Some(oid) = read_ref_oid(git_dir, start) {
            return Some(oid);
        }
    }
    let name = normalize_branch_arg(start);
    if !is_safe_branch_name(name) {
        println!("invalid start point: {}", start);
        return None;
    }
    let local_ref = make_head_ref(name)?;
    if let Some(oid) = read_ref_oid(git_dir, &local_ref) {
        return Some(oid);
    }
    let remote_ref = make_origin_ref(name)?;
    if let Some(oid) = read_ref_oid(git_dir, &remote_ref) {
        return Some(oid);
    }
    println!("start point not found: {}", start);
    None
}

fn read_head_oid(git_dir: &str) -> Option<[u8; 20]> {
    let head_path = join_path(git_dir, "HEAD")?;
    let head_data = read_small_file(&head_path, 256)?;
    let head = trim_ascii_str(&head_data)?;
    if let Some(ref_name) = strip_prefix(head, "ref: ") {
        return read_ref_oid(git_dir, ref_name);
    }
    parse_hex_oid(head.as_bytes())
}

fn print_branch_line(
    git_dir: &str,
    branch: &str,
    remote: bool,
    current: bool,
    verbose: bool,
) -> Option<()> {
    let marker = if current { "*" } else { " " };
    if !verbose {
        println!("{} {}", marker, branch);
        return Some(());
    }
    let ref_name = if remote {
        if let Some(v) = strip_prefix(branch, "remotes/") {
            make_remote_ref(v)?
        } else {
            make_remote_ref(branch)?
        }
    } else {
        make_head_ref(branch)?
    };
    match read_ref_oid(git_dir, &ref_name) {
        Some(oid) => {
            print!("{} {} ", marker, branch);
            print_short_oid(&oid);
            println!("");
        }
        None => println!("{} {}", marker, branch),
    }
    Some(())
}

fn collect_branch_names(dir: &str, prefix: &str, out: &mut Vec<String>) -> Option<()> {
    let fd = open(AT_FDCWD, dir, OpenFlags::RDONLY | OpenFlags::O_DIRECTORY, 0);
    if fd < 0 {
        return Some(());
    }
    let fd = fd as usize;
    let mut buf = [0u8; 2048];
    loop {
        let n = getdents64(fd, &mut buf);
        if n < 0 {
            let _ = close(fd);
            return None;
        }
        if n == 0 {
            break;
        }
        parse_dirents(dir, prefix, &buf[..n as usize], out)?;
    }
    let _ = close(fd);
    Some(())
}

fn parse_dirents(dir: &str, prefix: &str, buf: &[u8], out: &mut Vec<String>) -> Option<()> {
    let mut offset = 0usize;
    while offset < buf.len() {
        if offset + 19 > buf.len() {
            break;
        }
        let reclen = u16::from_ne_bytes([buf[offset + 16], buf[offset + 17]]) as usize;
        if reclen == 0 || offset + reclen > buf.len() {
            break;
        }
        let name_start = offset + 19;
        let mut name_end = name_start;
        while name_end < offset + reclen && buf[name_end] != 0 {
            name_end += 1;
        }
        if let Ok(name) = core::str::from_utf8(&buf[name_start..name_end]) {
            if should_visit_name(name) {
                let branch = join_rel_path(prefix, name)?;
                let child = join_path(dir, name)?;
                if is_ref_file(&child) {
                    out.push(branch);
                } else {
                    collect_branch_names(&child, &branch, out)?;
                }
            }
        }
        offset += reclen;
    }
    Some(())
}

fn should_visit_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".."
}

fn read_small_file(path: &str, max_len: usize) -> Option<Vec<u8>> {
    let fd = open(AT_FDCWD, path, OpenFlags::RDONLY, 0);
    if fd < 0 {
        return None;
    }
    let fd = fd as usize;
    let mut out = Vec::new();
    let mut buf = [0u8; 256];
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

fn is_ref_file(path: &str) -> bool {
    match read_small_file(path, 256) {
        Some(data) => {
            let Some(text) = trim_ascii_str(&data) else {
                return false;
            };
            parse_hex_oid(text.as_bytes()).is_some()
        }
        None => false,
    }
}

fn read_ref_oid(git_dir: &str, ref_name: &str) -> Option<[u8; 20]> {
    let path = join_path(git_dir, ref_name)?;
    let data = read_small_file(&path, 256)?;
    let text = trim_ascii_str(&data)?;
    parse_hex_oid(text.as_bytes())
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

fn append_oid_hex(out: &mut Vec<u8>, oid: &[u8; 20]) {
    for &b in oid {
        push_hex_byte_vec(out, b);
    }
}

fn print_short_oid(oid: &[u8; 20]) {
    for &b in &oid[..4] {
        print!("{:02x}", b);
    }
}

fn push_hex_byte_vec(out: &mut Vec<u8>, b: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push(HEX[(b >> 4) as usize]);
    out.push(HEX[(b & 0x0f) as usize]);
}

fn make_head_ref(branch: &str) -> Option<String> {
    let mut out = String::from("refs/heads/");
    out.push_str(branch);
    Some(out)
}

fn make_origin_ref(branch: &str) -> Option<String> {
    let mut out = String::from("refs/remotes/origin/");
    out.push_str(branch);
    Some(out)
}

fn make_remote_ref(branch: &str) -> Option<String> {
    let mut out = String::from("refs/remotes/");
    out.push_str(branch);
    Some(out)
}

fn normalize_branch_arg(input: &str) -> &str {
    if let Some(v) = strip_prefix(input, "origin/") {
        v
    } else if let Some(v) = strip_prefix(input, "remotes/origin/") {
        v
    } else {
        input
    }
}

fn is_safe_branch_name(input: &str) -> bool {
    !input.is_empty()
        && !input.starts_with('/')
        && !input.ends_with('/')
        && !input.contains("..")
        && !input.contains("//")
        && input
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'/')
}

fn mkdir_ref_parents(git_dir: &str, ref_name: &str) -> bool {
    let bytes = ref_name.as_bytes();
    let mut path = String::from(git_dir);
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
        match core::str::from_utf8(&bytes[start..end]) {
            Ok(seg) => path.push_str(seg),
            Err(_) => return false,
        }
        let _ = user_lib::mkdir(&path, 0o755);
        start = end + 1;
    }
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
        println!("open output failed: {}", path);
        return false;
    }
    let fd = fd as usize;
    let mut written = 0usize;
    while written < data.len() {
        let n = write(fd, &data[written..]);
        if n <= 0 {
            println!("write output failed: {}", path);
            let _ = close(fd);
            return false;
        }
        written += n as usize;
    }
    let _ = close(fd);
    true
}

fn join_path(parent: &str, name: &str) -> Option<String> {
    if parent.len() + name.len() + 2 > MAX_PATH_LEN {
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

fn join_rel_path(parent: &str, name: &str) -> Option<String> {
    if name.is_empty() || name.as_bytes().iter().any(|&b| b == b'/' || b == 0) {
        return None;
    }
    if parent.len() + name.len() + 2 > MAX_PATH_LEN {
        println!("path too long");
        return None;
    }
    let mut out = String::new();
    if !parent.is_empty() {
        out.push_str(parent);
        out.push('/');
    }
    out.push_str(name);
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

fn print_usage() {
    println!("usage: git branch [-r|-a] [repo-dir]");
    println!("       git branch --repo DIR");
}
