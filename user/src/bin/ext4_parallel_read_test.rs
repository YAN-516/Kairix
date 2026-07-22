#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    AT_FDCWD, OpenFlags, close, exit, fork, fstat, open, read, sync, unlinkat, waitpid, write,
};

const WORKERS: usize = 8;
const ROUNDS: usize = 256;
const PATHS: [&str; WORKERS] = [
    "/ext4_parallel_read_test.0",
    "/ext4_parallel_read_test.1",
    "/ext4_parallel_read_test.2",
    "/ext4_parallel_read_test.3",
    "/ext4_parallel_read_test.4",
    "/ext4_parallel_read_test.5",
    "/ext4_parallel_read_test.6",
    "/ext4_parallel_read_test.7",
];

fn expected(worker: usize) -> u8 {
    0x31u8.wrapping_add((worker as u8).wrapping_mul(17))
}

fn prepare(worker: usize) -> bool {
    let _ = unlinkat(AT_FDCWD, PATHS[worker], 0);
    let fd = open(
        AT_FDCWD,
        PATHS[worker],
        OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::RDWR,
        0o600,
    );
    if fd < 0 {
        return false;
    }
    let fd = fd as usize;
    let data = [expected(worker); 64];
    write(fd, &data) == data.len() as isize && close(fd) == 0
}

fn read_worker(worker: usize) -> bool {
    for _ in 0..ROUNDS {
        let fd = open(AT_FDCWD, PATHS[worker], OpenFlags::RDONLY, 0);
        if fd < 0 {
            return false;
        }
        let fd = fd as usize;
        let mut stat = [0u8; 256];
        let mut data = [0u8; 64];
        let ok = fstat(fd, &mut stat) == 0
            && read(fd, &mut data) == data.len() as isize
            && data.iter().all(|byte| *byte == expected(worker))
            && close(fd) == 0;
        if !ok {
            return false;
        }
    }
    true
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!(
        "[ext4_parallel_read_test] start workers={} rounds={}",
        WORKERS, ROUNDS
    );
    for worker in 0..WORKERS {
        if !prepare(worker) {
            println!("[ext4_parallel_read_test] FAIL: prepare worker={}", worker);
            return 1;
        }
    }
    if sync() < 0 {
        println!("[ext4_parallel_read_test] FAIL: sync");
        return 2;
    }

    let mut children = [0usize; WORKERS];
    for worker in 0..WORKERS {
        let child = fork();
        if child == 0 {
            exit(if read_worker(worker) { 0 } else { 3 });
        }
        if child < 0 {
            println!("[ext4_parallel_read_test] FAIL: fork worker={}", worker);
            return 3;
        }
        children[worker] = child as usize;
    }

    let mut passed = true;
    for child in children {
        let mut status = -1;
        if waitpid(child, &mut status) != child as isize || status != 0 {
            passed = false;
        }
    }
    for path in PATHS {
        let _ = unlinkat(AT_FDCWD, path, 0);
    }

    if passed {
        println!("[ext4_parallel_read_test] PASS");
        0
    } else {
        println!("[ext4_parallel_read_test] FAIL: child");
        4
    }
}
