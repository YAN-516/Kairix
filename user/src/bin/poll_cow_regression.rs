#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::arch::asm;
use user_lib::{close, exit, fork, mmap, munmap, pipe, read, waitpid, write};

const PAGE_SIZE: usize = 4096;
const PROT_READ: usize = 1;
const PROT_WRITE: usize = 2;
const MAP_PRIVATE: usize = 2;
const MAP_ANONYMOUS: usize = 0x20;

#[repr(C)]
struct Timespec {
    sec: i64,
    nsec: i64,
}

#[cfg(target_arch = "riscv64")]
fn pselect(nfds: usize, readfds: *mut u64, timeout: *const Timespec) -> isize {
    let result: isize;
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") nfds => result,
            in("a1") readfds,
            in("a2") 0usize,
            in("a3") 0usize,
            in("a4") timeout,
            in("a5") 0usize,
            in("a7") 72usize,
        );
    }
    result
}

#[cfg(target_arch = "loongarch64")]
fn pselect(nfds: usize, readfds: *mut u64, timeout: *const Timespec) -> isize {
    let result: isize;
    unsafe {
        asm!(
            "syscall 0",
            inlateout("$a0") nfds => result,
            in("$a1") readfds,
            in("$a2") 0usize,
            in("$a3") 0usize,
            in("$a4") timeout,
            in("$a5") 0usize,
            in("$a7") 72usize,
        );
    }
    result
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[poll_cow_regression] start");
    let mapping = mmap(
        0,
        PAGE_SIZE,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0,
    );
    if mapping < 0 {
        println!("[poll_cow_regression] FAIL mmap={}", mapping);
        return 1;
    }
    let read_set = mapping as usize as *mut u64;
    let mut pipefd = [0i32; 2];
    if pipe(&mut pipefd) != 0 {
        println!("[poll_cow_regression] FAIL pipe");
        let _ = munmap(mapping as usize, PAGE_SIZE);
        return 2;
    }
    let read_fd = pipefd[0] as usize;
    if read_fd >= 1024 {
        println!("[poll_cow_regression] FAIL fd={}", read_fd);
        return 3;
    }
    unsafe {
        *read_set.add(read_fd / 64) = 1u64 << (read_fd % 64);
    }

    let child = fork();
    if child < 0 {
        println!("[poll_cow_regression] FAIL fork={}", child);
        return 4;
    }
    if child == 0 {
        let _ = close(pipefd[1] as usize);
        let mut byte = [0u8; 1];
        if read(read_fd, &mut byte) != 1 {
            exit(5);
        }
        let preserved = unsafe { *read_set.add(read_fd / 64) & (1u64 << (read_fd % 64)) != 0 };
        exit(if preserved { 0 } else { 6 });
    }

    let timeout = Timespec { sec: 0, nsec: 0 };
    let selected = pselect(read_fd + 1, read_set, &timeout);
    let parent_cleared = unsafe { *read_set.add(read_fd / 64) & (1u64 << (read_fd % 64)) == 0 };
    let signal = write(pipefd[1] as usize, &[1]);
    let mut status = 0;
    let waited = waitpid(child as usize, &mut status);
    let _ = close(pipefd[0] as usize);
    let _ = close(pipefd[1] as usize);
    let _ = munmap(mapping as usize, PAGE_SIZE);

    if selected == 0 && parent_cleared && signal == 1 && waited == child && status == 0 {
        println!("[poll_cow_regression] PASS");
        0
    } else {
        println!(
            "[poll_cow_regression] FAIL select={} parent_cleared={} signal={} waited={} status={}",
            selected, parent_cleared, signal, waited, status
        );
        1
    }
}
