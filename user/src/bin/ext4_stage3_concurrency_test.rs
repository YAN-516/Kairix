#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    AT_FDCWD, OpenFlags, close, exit, fork, ftruncate, lseek, mkdir, open, read, renameat, sync,
    unlinkat, waitpid, write, yield_,
};

const WORKERS: usize = 8;
const ROUNDS: usize = 32;
const AT_REMOVEDIR: u32 = 0x200;
const SEEK_SET: i32 = 0;

const DIRS: [&str; WORKERS] = [
    "/ext4_s3_0",
    "/ext4_s3_1",
    "/ext4_s3_2",
    "/ext4_s3_3",
    "/ext4_s3_4",
    "/ext4_s3_5",
    "/ext4_s3_6",
    "/ext4_s3_7",
];
const PATH_A: [&str; WORKERS] = [
    "/ext4_s3_0/a",
    "/ext4_s3_1/a",
    "/ext4_s3_2/a",
    "/ext4_s3_3/a",
    "/ext4_s3_4/a",
    "/ext4_s3_5/a",
    "/ext4_s3_6/a",
    "/ext4_s3_7/a",
];
const PATH_B: [&str; WORKERS] = [
    "/ext4_s3_0/b",
    "/ext4_s3_1/b",
    "/ext4_s3_2/b",
    "/ext4_s3_3/b",
    "/ext4_s3_4/b",
    "/ext4_s3_5/b",
    "/ext4_s3_6/b",
    "/ext4_s3_7/b",
];

fn cleanup_worker(worker: usize) {
    let _ = unlinkat(AT_FDCWD, PATH_A[worker], 0);
    let _ = unlinkat(AT_FDCWD, PATH_B[worker], 0);
}

fn run_worker(worker: usize) -> bool {
    let pattern = 0x21u8.wrapping_add((worker as u8).wrapping_mul(23));
    let data = [pattern; 4096];
    for round in 0..ROUNDS {
        cleanup_worker(worker);
        let fd = open(
            AT_FDCWD,
            PATH_A[worker],
            OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::RDWR,
            0o600,
        );
        if fd < 0 {
            return false;
        }
        let fd = fd as usize;
        if write(fd, &data) != data.len() as isize || close(fd) != 0 {
            return false;
        }
        if renameat(AT_FDCWD, PATH_A[worker], AT_FDCWD, PATH_B[worker]) != 0 {
            return false;
        }

        let fd = open(AT_FDCWD, PATH_B[worker], OpenFlags::RDWR, 0);
        if fd < 0 {
            return false;
        }
        let fd = fd as usize;
        let retained = 64 + ((worker + round) & 63);
        if ftruncate(fd, retained) != 0 || lseek(fd, 0, SEEK_SET) != 0 {
            let _ = close(fd);
            return false;
        }
        let mut verify = [0u8; 128];
        if read(fd, &mut verify[..retained]) != retained as isize
            || verify[..retained].iter().any(|byte| *byte != pattern)
            || close(fd) != 0
        {
            return false;
        }
        if unlinkat(AT_FDCWD, PATH_B[worker], 0) != 0 {
            return false;
        }
        if round & 3 == 0 {
            yield_();
        }
    }
    true
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!(
        "[ext4_stage3_concurrency_test] start workers={} rounds={}",
        WORKERS, ROUNDS
    );
    for worker in 0..WORKERS {
        cleanup_worker(worker);
        let _ = unlinkat(AT_FDCWD, DIRS[worker], AT_REMOVEDIR);
        if mkdir(DIRS[worker], 0o700) != 0 {
            println!(
                "[ext4_stage3_concurrency_test] FAIL: mkdir worker={}",
                worker
            );
            return 1;
        }
    }

    let mut children = [0usize; WORKERS];
    for (worker, child_slot) in children.iter_mut().enumerate() {
        let child = fork();
        if child == 0 {
            exit(if run_worker(worker) { 0 } else { 2 });
        }
        if child < 0 {
            println!(
                "[ext4_stage3_concurrency_test] FAIL: fork worker={}",
                worker
            );
            return 2;
        }
        *child_slot = child as usize;
    }

    let mut passed = true;
    for child in children {
        let mut status = -1;
        if waitpid(child, &mut status) != child as isize || status != 0 {
            passed = false;
        }
    }
    for worker in 0..WORKERS {
        cleanup_worker(worker);
        if unlinkat(AT_FDCWD, DIRS[worker], AT_REMOVEDIR) != 0 {
            passed = false;
        }
    }
    if sync() < 0 {
        passed = false;
    }

    if passed {
        println!("[ext4_stage3_concurrency_test] PASS");
        0
    } else {
        println!("[ext4_stage3_concurrency_test] FAIL: worker or cleanup");
        3
    }
}
