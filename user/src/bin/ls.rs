// user/src/bin/ls.rs
#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;
use alloc::string::String;
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
    let read_bytes = getdents64(fd as usize, &mut buf);
    if read_bytes > 0 {
        parse_and_print_dirents(&buf[..read_bytes as usize]);
    }

    close(fd as usize);
    0
}

const DT_DIR: u8 = 4;
const DT_REG: u8 = 8;
fn parse_and_print_dirents(buf: &[u8]) {
    let mut offset = 0;
    let mut files = alloc::vec::Vec::new();
    let mut max_len = 0;
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
            if name_str != "." && name_str != ".." {
                files.push((String::from(name_str), d_type));
                if name_str.len() > max_len {
                    max_len = name_str.len();
                }
            }
        }
        offset += reclen;
    }
    if files.is_empty() {
        return;
    }
    let term_width = 100;
    let col_width = max_len + 2;
    let cols = (term_width / col_width).max(1);
    for (i, (name, d_type)) in files.iter().enumerate() {
        match *d_type {
            DT_DIR => print!("\x1b[1m\x1b[34m{}\x1b[0m", name),
            DT_REG => print!("\x1b[1m\x1b[32m{}\x1b[0m", name),
            _ => print!("{}", name),
        }
        let padding = col_width - name.len();
        for _ in 0..padding {
            print!(" ");
        }
        if (i + 1) % cols == 0 {
            println!("");
        }
    }
    if files.len() % cols != 0 {
        println!("");
    }
}
