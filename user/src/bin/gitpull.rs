#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use alloc::{string::String, vec::Vec};
use user_lib::{AT_FDCWD, OpenFlags, close, execve, exit, fork, open, read, waitpid};

const FETCH_BIN: &str = "/bin/gitfetch";
const CHECKOUT_BIN: &str = "/bin/gitcheckout";
const DEFAULT_PACK: &str = "/musl/gitpull.pack";
const DEFAULT_META: &str = "/musl/gitpull.meta";
const MAX_ARG_LEN: usize = 512;
const MAX_CONFIG_LEN: usize = 2048;

struct Config {
    repo_dir: Option<&'static str>,
    url: Option<&'static str>,
    pack_path: &'static str,
    meta_path: &'static str,
    fetch_args: Vec<&'static str>,
}

impl Config {
    fn new() -> Self {
        Self {
            repo_dir: None,
            url: None,
            pack_path: DEFAULT_PACK,
            meta_path: DEFAULT_META,
            fetch_args: Vec::new(),
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

    let (repo_dir, remote_or_url) = match resolve_pull_args(&cfg) {
        Some(v) => v,
        None => {
            println!("not a git repository");
            return -1;
        }
    };
    let url = match remote_or_url {
        Some(v) if is_git_url(v) => String::from(v),
        Some(v) => match read_remote_url(&repo_dir, v) {
            Some(v) => v,
            None => {
                println!("missing remote: {}", v);
                return -1;
            }
        },
        None => match read_remote_url(&repo_dir, "origin") {
            Some(v) => v,
            None => {
                println!("missing url and failed to read origin from .git/config");
                return -1;
            }
        },
    };

    println!("gitpull fetch: {}", url);
    if !run_gitfetch(&cfg, &repo_dir, &url) {
        return -1;
    }

    println!("gitpull checkout: {}", repo_dir);
    if !run_gitcheckout(&cfg, &repo_dir) {
        return -1;
    }

    println!("gitpull complete: {}", repo_dir);
    0
}

fn resolve_pull_args(cfg: &Config) -> Option<(String, Option<&'static str>)> {
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
        } else if arg == "--pack" {
            cfg.pack_path = match next_arg(argc, argv, &mut i, "pack") {
                Some(v) if !v.is_empty() => v,
                _ => {
                    println!("invalid pack path");
                    return ArgResult::Error;
                }
            };
        } else if let Some(v) = strip_prefix(arg, "--pack=") {
            if v.is_empty() {
                println!("invalid pack path");
                return ArgResult::Error;
            }
            cfg.pack_path = v;
        } else if arg == "--meta" {
            cfg.meta_path = match next_arg(argc, argv, &mut i, "meta") {
                Some(v) if !v.is_empty() => v,
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
            cfg.meta_path = v;
        } else if takes_value(arg) {
            cfg.fetch_args.push(arg);
            match next_arg(argc, argv, &mut i, arg) {
                Some(v) => cfg.fetch_args.push(v),
                None => return ArgResult::Error,
            }
        } else if starts_with(arg, "-") {
            cfg.fetch_args.push(arg);
        } else if cfg.repo_dir.is_none() {
            cfg.repo_dir = Some(arg);
        } else if cfg.url.is_none() {
            cfg.url = Some(arg);
        } else {
            println!("too many arguments");
            return ArgResult::Error;
        }
        i += 1;
    }
    ArgResult::Ok
}

fn run_gitfetch(cfg: &Config, repo_dir: &str, url: &str) -> bool {
    let mut args = Vec::new();
    args.push("gitfetch");
    args.push(url);
    args.push("--repo");
    args.push(repo_dir);
    args.push("-o");
    args.push(cfg.pack_path);
    args.push("--meta");
    args.push(cfg.meta_path);
    for &arg in &cfg.fetch_args {
        args.push(arg);
    }
    run_command(FETCH_BIN, &args)
}

fn run_gitcheckout(cfg: &Config, repo_dir: &str) -> bool {
    let args = [
        "gitcheckout",
        cfg.pack_path,
        repo_dir,
        "--git",
        "--meta",
        cfg.meta_path,
        "--quiet",
    ];
    run_command(CHECKOUT_BIN, &args)
}

fn run_command(path: &str, args: &[&str]) -> bool {
    let pid = fork();
    if pid < 0 {
        println!("fork failed: {}", pid);
        return false;
    }
    if pid == 0 {
        let env = ["PATH=/bin:/sbin:/musl:/usr/bin"];
        let ret = execve(path, args, &env);
        println!("exec failed: {} {}", path, ret);
        exit(-1);
    }

    let mut code = 0i32;
    let ret = waitpid(pid as usize, &mut code);
    if ret < 0 {
        println!("waitpid failed: {}", ret);
        return false;
    }
    if code != 0 {
        println!("command failed: {} exit {}", path, code);
        return false;
    }
    true
}

fn read_remote_url(repo_dir: &str, remote: &str) -> Option<String> {
    if !is_safe_remote_name(remote) {
        return None;
    }
    let git_dir = join_path(repo_dir, ".git")?;
    let config_path = join_path(&git_dir, "config")?;
    let data = read_small_file(&config_path, MAX_CONFIG_LEN)?;
    let mut header = Vec::new();
    header.extend_from_slice(b"[remote \"");
    header.extend_from_slice(remote.as_bytes());
    header.extend_from_slice(b"\"]");
    let mut in_remote = false;
    for raw_line in data.split(|&b| b == b'\n') {
        let line = trim_ascii(raw_line);
        if line.is_empty() || line[0] == b'#' || line[0] == b';' {
            continue;
        }
        if line[0] == b'[' {
            in_remote = line == header.as_slice();
            continue;
        }
        if !in_remote {
            continue;
        }
        let Some(eq) = find_byte(line, b'=') else {
            continue;
        };
        if trim_ascii(&line[..eq]) != b"url" {
            continue;
        }
        let value = trim_ascii(&line[eq + 1..]);
        if value.is_empty() || value.len() > MAX_ARG_LEN {
            return None;
        }
        let url = core::str::from_utf8(value).ok()?;
        return Some(String::from(url));
    }
    None
}

fn read_small_file(path: &str, max_len: usize) -> Option<Vec<u8>> {
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

fn takes_value(arg: &str) -> bool {
    matches!(
        arg,
        "-d" | "--dns"
            | "--ip"
            | "-p"
            | "--port"
            | "-u"
            | "--user"
            | "--password"
            | "-i"
            | "--key"
            | "--have"
            | "--depth"
    )
}

fn argv_str(argv: *const usize, idx: usize) -> Option<&'static str> {
    cstr_to_str(unsafe { *argv.add(idx) as *const u8 })
}

fn next_arg(argc: usize, argv: *const usize, idx: &mut usize, name: &str) -> Option<&'static str> {
    *idx += 1;
    if *idx >= argc {
        println!("missing value for {}", name);
        None
    } else {
        argv_str(argv, *idx)
    }
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

fn trim_ascii(mut input: &[u8]) -> &[u8] {
    while let Some((&b, rest)) = input.split_first() {
        if !b.is_ascii_whitespace() {
            break;
        }
        input = rest;
    }
    while let Some((&b, rest)) = input.split_last() {
        if !b.is_ascii_whitespace() {
            break;
        }
        input = rest;
    }
    input
}

fn find_byte(input: &[u8], needle: u8) -> Option<usize> {
    for (idx, &b) in input.iter().enumerate() {
        if b == needle {
            return Some(idx);
        }
    }
    None
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

fn print_usage() {
    println!("usage: gitpull [repo-dir] [remote-or-url] [fetch options]");
    println!("example: gitpull /musl/repo --key /musl/id_ed25519");
    println!("example: cd /musl/repo; git pull me --key /musl/id_ed25519");
    println!("example: gitpull /musl/repo git@github.com:user/repo.git --key /musl/id_ed25519");
    println!("options:");
    println!("      --pack PATH       temporary pack path, default /musl/gitpull.pack");
    println!("      --meta PATH       temporary metadata path, default /musl/gitpull.meta");
    println!("  -d, --dns IP          forwarded to gitfetch");
    println!("      --ip IP           forwarded to gitfetch");
    println!("  -p, --port PORT       forwarded to gitfetch");
    println!("  -u, --user USER       forwarded to gitfetch");
    println!("      --password PASS   forwarded to gitfetch");
    println!("  -i, --key PATH        forwarded to gitfetch");
    println!("      --have OID        forwarded to gitfetch");
    println!("      --depth N         forwarded to gitfetch for shallow fetch");
    println!("  -v, --verbose         forwarded to gitfetch");
}
