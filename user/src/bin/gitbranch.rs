#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use alloc::{string::String, vec::Vec};
use user_lib::{AT_FDCWD, OpenFlags, close, getdents64, open, read};

const DEFAULT_REPO: &str = ".";
const MAX_ARG_LEN: usize = 512;
const MAX_PATH_LEN: usize = 512;
const DT_REG: u8 = 8;

struct Config {
    repo_dir: &'static str,
    remotes: bool,
    all: bool,
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
    };
    let mut positional = 0usize;
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
        } else if arg == "--repo" {
            i += 1;
            if i >= argc {
                println!("missing value for --repo");
                return None;
            }
            cfg.repo_dir = argv_str(argv, i)?;
        } else if let Some(v) = strip_prefix(arg, "--repo=") {
            cfg.repo_dir = v;
        } else if starts_with(arg, "-") {
            println!("unknown option: {}", arg);
            return None;
        } else if positional == 0 {
            cfg.repo_dir = arg;
            positional += 1;
        } else {
            println!("too many arguments");
            return None;
        }
        i += 1;
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

    if !cfg.remotes {
        let heads_dir = join_path(&git_dir, "refs/heads")?;
        let mut branches = Vec::new();
        collect_branch_names(&heads_dir, "", &mut branches)?;
        branches.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        for branch in branches {
            if current.as_deref() == Some(branch.as_str()) {
                println!("* {}", branch);
            } else {
                println!("  {}", branch);
            }
        }
    }

    if cfg.remotes || cfg.all {
        let remotes_dir = join_path(&git_dir, "refs/remotes")?;
        let mut branches = Vec::new();
        collect_branch_names(&remotes_dir, "", &mut branches)?;
        branches.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        for branch in branches {
            if cfg.all {
                println!("  remotes/{}", branch);
            } else {
                println!("  {}", branch);
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
        let d_type = buf[offset + 18];
        let name_start = offset + 19;
        let mut name_end = name_start;
        while name_end < offset + reclen && buf[name_end] != 0 {
            name_end += 1;
        }
        if let Ok(name) = core::str::from_utf8(&buf[name_start..name_end]) {
            if should_visit_name(name) {
                let branch = join_rel_path(prefix, name)?;
                if d_type == DT_REG {
                    out.push(branch);
                } else {
                    let child = join_path(dir, name)?;
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
