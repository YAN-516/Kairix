// user/src/bin/ls.rs
#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
use user_lib::{OpenFlags, close, getdents64, open};

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    let fd = open(-100, ".", OpenFlags::RDONLY, 0);
    println!("fd: {}", fd);
    if fd < 0 {
        println!("ls: cannot open current directory");
        return -1;
    }
    let mut buf = [0u8; 2048];
    let mut printed = false;
    loop {
        let read_bytes = getdents64(fd as usize, &mut buf);
        println!("ls: getdents64 -> {}", read_bytes);
        if read_bytes <= 0 {
            break;
        }
        printed |= print_dirents(&buf[..read_bytes as usize]);
    }
    if printed {
        println!("");
    }

    println!("ls: before close");
    close(fd as usize);
    println!("ls: after close");
    println!("ls: return");
    0
}

const DT_DIR: u8 = 4;
const DT_REG: u8 = 8;

fn print_dirents(buf: &[u8]) -> bool {
    let mut offset = 0;
    let mut printed = false;

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

        if let Ok(name_str) = core::str::from_utf8(&buf[name_start..name_end]) {
            if !name_str.is_empty() && name_str != "." && name_str != ".." {
                print_one(name_str, d_type);
                printed = true;
            }
        }
        offset += reclen;
    }
    printed
}

fn print_one(name: &str, d_type: u8) {
    match d_type {
        DT_DIR => print!("\x1b[1m\x1b[34m{}\x1b[0m", name),
        DT_REG => print!("\x1b[1m\x1b[32m{}\x1b[0m", name),
        _ => print!("{}", name),
    }
    print!("  ");
}
