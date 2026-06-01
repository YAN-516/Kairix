#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{AT_FDCWD, OpenFlags, close, open, read, write};

const STDOUT: usize = 1;

fn dump_file(path: &str) -> i32 {
    let fd = open(AT_FDCWD, path, OpenFlags::RDONLY, 0);
    if fd < 0 {
        println!("zcat: cannot open {}", path);
        return 1;
    }

    let fd = fd as usize;
    let mut buf = [0u8; 1024];
    loop {
        let len = read(fd, &mut buf);
        if len < 0 {
            println!("zcat: read error on {}", path);
            close(fd);
            return 1;
        }
        if len == 0 {
            break;
        }
        let mut written = 0usize;
        let len = len as usize;
        while written < len {
            let ret = write(STDOUT, &buf[written..len]);
            if ret < 0 {
                println!("zcat: write error");
                close(fd);
                return 1;
            }
            written += ret as usize;
        }
    }

    close(fd);
    0
}

#[unsafe(no_mangle)]
pub fn main_with_args(argc: usize, argv: *const usize) -> i32 {
    if argc <= 1 {
        return dump_file("/proc/config.gz");
    }

    let mut status = 0;
    for i in 1..argc {
        let arg_ptr = unsafe { *argv.add(i) as *const u8 };
        if arg_ptr.is_null() {
            status = 1;
            continue;
        }
        let path = match cstr_to_str(arg_ptr) {
            Some(path) => path,
            None => {
                println!("zcat: invalid utf8 path");
                status = 1;
                continue;
            }
        };
        if dump_file(path) != 0 {
            status = 1;
        }
    }
    status
}

fn cstr_to_str(ptr: *const u8) -> Option<&'static str> {
    let mut len = 0usize;
    loop {
        let b = unsafe { *ptr.add(len) };
        if b == 0 {
            break;
        }
        len += 1;
    }
    let bytes = unsafe { core::slice::from_raw_parts(ptr, len) };
    core::str::from_utf8(bytes).ok()
}
