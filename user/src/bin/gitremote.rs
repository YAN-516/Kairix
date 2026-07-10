#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use alloc::{string::String, vec::Vec};
use user_lib::{AT_FDCWD, OpenFlags, close, open, read, write};

const MAX_ARG_LEN: usize = 512;
const MAX_CONFIG_LEN: usize = 8192;

#[unsafe(no_mangle)]
pub fn main_with_args(argc: usize, argv: *const usize) -> i32 {
    match run(argc, argv) {
        Some(()) => 0,
        None => -1,
    }
}

fn run(argc: usize, argv: *const usize) -> Option<()> {
    if argc == 2 {
        let arg = argv_str(argv, 1)?;
        if arg == "-h" || arg == "--help" {
            print_usage();
            return Some(());
        }
    }
    if (argc != 4 && argc != 6) || argv_str(argv, 1)? != "add" {
        print_usage();
        return None;
    }
    let name = argv_str(argv, 2)?;
    let url = argv_str(argv, 3)?;
    let repo_arg = if argc == 6 {
        if argv_str(argv, 4)? != "--repo" {
            print_usage();
            return None;
        }
        argv_str(argv, 5)?
    } else {
        "."
    };
    remote_add(repo_arg, name, url)
}

fn remote_add(repo_arg: &str, name: &str, url: &str) -> Option<()> {
    if !is_safe_remote_name(name) {
        println!("invalid remote name: {}", name);
        return None;
    }
    if url.is_empty() {
        println!("empty remote url");
        return None;
    }
    let repo = match user_lib::git::discover_repository(repo_arg) {
        Some(v) => v,
        None => {
            println!("not a git repository: {}", repo_arg);
            return None;
        }
    };
    let git_dir = join_path(&repo, ".git")?;
    let config_path = join_path(&git_dir, "config")?;
    let old = read_small_file(&config_path, MAX_CONFIG_LEN).unwrap_or_else(Vec::new);
    if has_remote(&old, name) {
        println!("remote already exists: {}", name);
        return None;
    }
    let mut out = old;
    if !out.is_empty() && out[out.len() - 1] != b'\n' {
        out.push(b'\n');
    }
    out.extend_from_slice(b"[remote \"");
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(b"\"]\n\turl = ");
    out.extend_from_slice(url.as_bytes());
    out.extend_from_slice(b"\n\tfetch = +refs/heads/*:refs/remotes/");
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(b"/*\n");
    if write_file(&config_path, &out) {
        println!("added remote {} {}", name, url);
        Some(())
    } else {
        None
    }
}

fn has_remote(config: &[u8], name: &str) -> bool {
    let mut header = Vec::new();
    header.extend_from_slice(b"[remote \"");
    header.extend_from_slice(name.as_bytes());
    header.extend_from_slice(b"\"]");
    for raw in config.split(|&b| b == b'\n') {
        if trim_ascii(raw) == header.as_slice() {
            return true;
        }
    }
    false
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

fn trim_ascii(mut input: &[u8]) -> &[u8] {
    while !input.is_empty() && input[0].is_ascii_whitespace() {
        input = &input[1..];
    }
    while !input.is_empty() && input[input.len() - 1].is_ascii_whitespace() {
        input = &input[..input.len() - 1];
    }
    input
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
    println!("usage: git remote add <name> <url>");
    println!("       git remote add <name> <url> --repo DIR");
}
