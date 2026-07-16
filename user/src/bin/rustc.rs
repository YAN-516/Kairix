#![no_std]
#![no_main]

extern crate alloc;
#[macro_use]
extern crate user_lib;

use alloc::vec::Vec;
use user_lib::execve;

const REAL_RUSTC: &str = "/usr/bin/rustc";
const MAX_ARG_LEN: usize = 4096;

#[unsafe(no_mangle)]
pub fn main_with_args(argc: usize, argv: *const usize) -> i32 {
    if argc == 2 {
        match argv_str(argv, 1) {
            Some("-h") | Some("--h") => {
                print_help();
                return 0;
            }
            _ => {}
        }
    }

    let mut args = Vec::with_capacity(argc.max(1));
    args.push("rustc");
    for index in 1..argc {
        let Some(arg) = argv_str(argv, index) else {
            println!("rustc: invalid argument {}", index);
            return 2;
        };
        args.push(arg);
    }

    let env = [
        "PATH=/usr/bin:/bin:/sbin:/musl",
        "LD_LIBRARY_PATH=/usr/lib:/lib",
        "HOME=/",
        "TMPDIR=/tmp",
    ];
    let ret = execve(REAL_RUSTC, &args, &env);
    println!("rustc: failed to execute {}: {}", REAL_RUSTC, ret);
    127
}

fn argv_str(argv: *const usize, index: usize) -> Option<&'static str> {
    cstr_to_str(unsafe { *argv.add(index) as *const u8 })
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

fn print_help() {
    println!("Usage: rustc [OPTIONS] INPUT");
    println!("Options:");
    println!("  -h, --h, --help    show this help");
}
