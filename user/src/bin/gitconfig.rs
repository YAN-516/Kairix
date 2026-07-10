#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use alloc::vec::Vec;
use user_lib::{AT_FDCWD, OpenFlags, close, open, read, write};

const GLOBAL_CONFIG: &str = "/tmp/.gitconfig";
const MAX_ARG_LEN: usize = 512;
const MAX_CONFIG_LEN: usize = 4096;

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
    if argc != 4 || argv_str(argv, 1)? != "--global" {
        print_usage();
        return None;
    }
    let key = argv_str(argv, 2)?;
    let value = argv_str(argv, 3)?;
    if value.is_empty() {
        println!("empty config value");
        return None;
    }
    match key {
        "user.name" => write_user_config(Some(value), None),
        "user.email" => write_user_config(None, Some(value)),
        _ => {
            println!("unsupported config key: {}", key);
            None
        }
    }
}

fn write_user_config(name: Option<&str>, email: Option<&str>) -> Option<()> {
    let old = read_small_file(GLOBAL_CONFIG, MAX_CONFIG_LEN).unwrap_or_else(Vec::new);
    let mut old_name = None;
    let mut old_email = None;
    let mut in_user = false;
    for raw in old.split(|&b| b == b'\n') {
        let line = trim_ascii(raw);
        if line.is_empty() || line[0] == b'#' || line[0] == b';' {
            continue;
        }
        if line[0] == b'[' {
            in_user = line == b"[user]";
            continue;
        }
        if !in_user {
            continue;
        }
        let eq = match find_byte(line, b'=') {
            Some(v) => v,
            None => continue,
        };
        let key = trim_ascii(&line[..eq]);
        let value = trim_ascii(&line[eq + 1..]);
        if key == b"name" {
            old_name = core::str::from_utf8(value).ok();
        } else if key == b"email" {
            old_email = core::str::from_utf8(value).ok();
        }
    }

    let final_name = name.or(old_name).unwrap_or("Kairix");
    let final_email = email.or(old_email).unwrap_or("kairix@example.local");
    let mut out = Vec::new();
    out.extend_from_slice(b"[user]\n\tname = ");
    out.extend_from_slice(final_name.as_bytes());
    out.extend_from_slice(b"\n\temail = ");
    out.extend_from_slice(final_email.as_bytes());
    out.push(b'\n');
    if write_file(GLOBAL_CONFIG, &out) {
        println!(
            "set global {}",
            if name.is_some() {
                "user.name"
            } else {
                "user.email"
            }
        );
        Some(())
    } else {
        None
    }
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

fn trim_ascii(mut input: &[u8]) -> &[u8] {
    while !input.is_empty() && input[0].is_ascii_whitespace() {
        input = &input[1..];
    }
    while !input.is_empty() && input[input.len() - 1].is_ascii_whitespace() {
        input = &input[..input.len() - 1];
    }
    input
}

fn find_byte(input: &[u8], value: u8) -> Option<usize> {
    input.iter().position(|&b| b == value)
}

fn print_usage() {
    println!("usage: git config --global user.name NAME");
    println!("       git config --global user.email EMAIL");
}
