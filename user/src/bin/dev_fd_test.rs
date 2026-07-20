#![no_std]
#![no_main]

extern crate alloc;

#[macro_use]
extern crate user_lib;

use alloc::format;
use user_lib::{OpenFlags, close, fcntl, open, pipe, read, write};

const AT_FDCWD: isize = -100;
const F_GETFD: usize = 1;

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[dev_fd_test] start");
    let mut fds = [0i32; 2];
    if pipe(&mut fds) < 0 {
        println!("[dev_fd_test] FAIL: pipe");
        return 1;
    }

    let write_path = format!("/dev/fd/{}", fds[1]);
    let alias = open(
        AT_FDCWD,
        &write_path,
        OpenFlags::WRONLY | OpenFlags::O_CLOEXEC,
        0,
    );
    if alias < 0 {
        println!("[dev_fd_test] FAIL: open {} ret={}", write_path, alias);
        return 2;
    }
    if fcntl(alias as usize, F_GETFD, 0) != 1 {
        println!("[dev_fd_test] FAIL: CLOEXEC");
        return 3;
    }
    if write(alias as usize, b"fd") != 2 {
        println!("[dev_fd_test] FAIL: write alias");
        return 4;
    }
    let _ = close(alias as usize);

    let read_path = format!("/proc/self/fd/{}", fds[0]);
    let read_alias = open(AT_FDCWD, &read_path, OpenFlags::RDONLY, 0);
    if read_alias < 0 {
        println!("[dev_fd_test] FAIL: open {} ret={}", read_path, read_alias);
        return 5;
    }
    let mut bytes = [0u8; 2];
    if read(read_alias as usize, &mut bytes) != 2 || bytes != *b"fd" {
        println!("[dev_fd_test] FAIL: read alias bytes={:?}", bytes);
        return 6;
    }

    let _ = close(read_alias as usize);
    let _ = close(fds[0] as usize);
    let _ = close(fds[1] as usize);
    println!("[dev_fd_test] PASS");
    0
}
