#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{close, exit, fork, mmap, munmap, pipe, read, waitpid, write};

const PAGE_SIZE: usize = 4096;
const PAGE_COUNT: usize = 2048;
const MAPPING_LEN: usize = PAGE_COUNT * PAGE_SIZE;
const WORKERS: usize = 4;
const PROT_READ: usize = 0x1;
const PROT_WRITE: usize = 0x2;
const MAP_PRIVATE: usize = 0x02;
const MAP_ANONYMOUS: usize = 0x20;

fn read_byte(fd: usize) -> bool {
    let mut byte = [0u8; 1];
    loop {
        match read(fd, &mut byte) {
            1 => return true,
            -4 => continue,
            _ => return false,
        }
    }
}

fn write_byte(fd: usize) -> bool {
    loop {
        match write(fd, &[1]) {
            1 => return true,
            -4 => continue,
            _ => return false,
        }
    }
}

fn child(ready: [i32; 2], start: [i32; 2]) -> ! {
    let _ = close(ready[0] as usize);
    let _ = close(start[1] as usize);
    let address = mmap(
        0,
        MAPPING_LEN,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0,
    );
    if address < 0 {
        exit(2);
    }
    let address = address as usize;
    for page in 0..PAGE_COUNT {
        unsafe {
            *((address + page * PAGE_SIZE) as *mut u8) = page as u8;
        }
    }
    if !write_byte(ready[1] as usize) || !read_byte(start[0] as usize) {
        exit(3);
    }
    if munmap(address, MAPPING_LEN) != 0 {
        exit(4);
    }
    exit(0);
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!(
        "[munmap_smp_test] start pages={} workers={}",
        PAGE_COUNT, WORKERS
    );
    let mut ready = [0i32; 2];
    let mut start = [0i32; 2];
    if pipe(&mut ready) < 0 || pipe(&mut start) < 0 {
        println!("[munmap_smp_test] FAIL: pipe");
        return 1;
    }

    let mut pids = [0isize; WORKERS];
    for slot in &mut pids {
        let pid = fork();
        if pid == 0 {
            child(ready, start);
        }
        if pid < 0 {
            println!("[munmap_smp_test] FAIL: fork ret={}", pid);
            return 2;
        }
        *slot = pid;
    }

    let _ = close(ready[1] as usize);
    let _ = close(start[0] as usize);
    for _ in 0..WORKERS {
        if !read_byte(ready[0] as usize) {
            println!("[munmap_smp_test] FAIL: ready");
            return 3;
        }
    }
    for _ in 0..WORKERS {
        if !write_byte(start[1] as usize) {
            println!("[munmap_smp_test] FAIL: start");
            return 4;
        }
    }
    let _ = close(ready[0] as usize);
    let _ = close(start[1] as usize);

    for pid in pids {
        let mut status = 0i32;
        let waited = waitpid(pid as usize, &mut status);
        if waited != pid || status != 0 {
            println!(
                "[munmap_smp_test] FAIL: pid={} waited={} status={}",
                pid, waited, status
            );
            return 5;
        }
    }
    println!("[munmap_smp_test] PASS");
    0
}
