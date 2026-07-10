#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use alloc::string::String;
use user_lib::{AT_FDCWD, OpenFlags, close, mkdir, open, write};

const DEFAULT_REPO: &str = ".";
const MAX_ARG_LEN: usize = 512;
const MAX_PATH_LEN: usize = 512;
const EMPTY_INDEX: [u8; 32] = [
    b'D', b'I', b'R', b'C', 0, 0, 0, 2, 0, 0, 0, 0, 0x39, 0xd8, 0x90, 0x13, 0x9e, 0xe5, 0x35, 0x6c,
    0x7e, 0xf5, 0x72, 0x21, 0x6c, 0xeb, 0xcd, 0x27, 0xaa, 0x41, 0xf9, 0xdf,
];

#[unsafe(no_mangle)]
pub fn main_with_args(argc: usize, argv: *const usize) -> i32 {
    let repo_dir = match parse_args(argc, argv) {
        Some(v) => v,
        None => return -1,
    };
    match init_repository(repo_dir) {
        Some(()) => 0,
        None => -1,
    }
}

fn parse_args(argc: usize, argv: *const usize) -> Option<&'static str> {
    let mut repo_dir = DEFAULT_REPO;
    let mut positional = 0usize;
    let mut i = 1usize;
    while i < argc {
        let arg = argv_str(argv, i)?;
        if arg == "-h" || arg == "--help" {
            print_usage();
            return None;
        } else if starts_with(arg, "-") {
            println!("unknown option: {}", arg);
            return None;
        } else if positional == 0 {
            repo_dir = arg;
            positional += 1;
        } else {
            println!("too many arguments");
            return None;
        }
        i += 1;
    }
    Some(repo_dir)
}

fn init_repository(repo_dir: &str) -> Option<()> {
    if repo_dir != "." {
        ensure_directory(repo_dir)?;
    }

    let git_dir = join_path(repo_dir, ".git")?;
    let already_exists = is_directory(&git_dir);
    ensure_directory(&git_dir)?;
    for dir in [
        "objects",
        "objects/info",
        "objects/pack",
        "refs",
        "refs/heads",
        "refs/tags",
    ] {
        ensure_directory(&join_path(&git_dir, dir)?)?;
    }

    write_file_if_missing(&join_path(&git_dir, "HEAD")?, b"ref: refs/heads/main\n")?;
    write_file_if_missing(
        &join_path(&git_dir, "config")?,
        b"[core]\n\trepositoryformatversion = 0\n\tfilemode = false\n\tbare = false\n",
    )?;
    write_file_if_missing(
        &join_path(&git_dir, "description")?,
        b"Unnamed repository\n",
    )?;
    write_file_if_missing(&join_path(&git_dir, "index")?, &EMPTY_INDEX)?;

    if already_exists {
        println!("Reinitialized existing Git repository in {}", git_dir);
    } else {
        println!("Initialized empty Git repository in {}", git_dir);
    }
    Some(())
}

fn ensure_directory(path: &str) -> Option<()> {
    if is_directory(path) {
        return Some(());
    }
    if mkdir(path, 0o755) < 0 || !is_directory(path) {
        println!("mkdir failed: {}", path);
        return None;
    }
    Some(())
}

fn is_directory(path: &str) -> bool {
    let fd = open(
        AT_FDCWD,
        path,
        OpenFlags::RDONLY | OpenFlags::O_DIRECTORY,
        0,
    );
    if fd < 0 {
        false
    } else {
        let _ = close(fd as usize);
        true
    }
}

fn file_exists(path: &str) -> bool {
    let fd = open(AT_FDCWD, path, OpenFlags::RDONLY, 0);
    if fd < 0 {
        false
    } else {
        let _ = close(fd as usize);
        true
    }
}

fn write_file_if_missing(path: &str, data: &[u8]) -> Option<()> {
    if file_exists(path) {
        return Some(());
    }
    if write_file(path, data) {
        Some(())
    } else {
        None
    }
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
    let mut out = String::from(parent);
    if !parent.ends_with('/') {
        out.push('/');
    }
    out.push_str(name);
    Some(out)
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
    println!("usage: git init [directory]");
}
