#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::sync::atomic::{AtomicUsize, Ordering};
use user_lib::{
    AT_FDCWD, OpenFlags, close, execve, exit, fork, open, read, thread_create, unlinkat, waitpid,
    write, yield_,
};

const PATH: &str = "/ext4_invalidation_exec_test.bin";
const TARGET: &str = "/multithread_exec_target";
const WORKERS: usize = 4;
const PAGE_SIZE: usize = 4096;
static STARTED: AtomicUsize = AtomicUsize::new(0);

fn write_all(fd: usize, data: &[u8]) -> bool {
    let mut written = 0usize;
    while written < data.len() {
        let count = write(fd, &data[written..]);
        if count <= 0 {
            return false;
        }
        written += count as usize;
    }
    true
}

extern "C" fn truncate_worker(index: usize) -> ! {
    STARTED.fetch_add(1, Ordering::Release);
    let page = [(index as u8).wrapping_mul(37).wrapping_add(0x21); PAGE_SIZE];
    loop {
        let fd = open(
            AT_FDCWD,
            PATH,
            OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::RDWR,
            0o600,
        );
        if fd >= 0 {
            let fd = fd as usize;
            let _ = write_all(fd, &page);
            let _ = close(fd);
        }
        yield_();
    }
}

fn exec_while_invalidating() -> ! {
    for worker in 0..WORKERS {
        if thread_create(truncate_worker, worker) < 0 {
            exit(101);
        }
    }
    while STARTED.load(Ordering::Acquire) != WORKERS {
        yield_();
    }
    for _ in 0..256 {
        yield_();
    }
    let result = execve(TARGET, &["multithread_exec_target"], &[]);
    println!(
        "[ext4_invalidation_exec_test] execve returned unexpectedly: {}",
        result
    );
    exit(102);
}

fn verify_post_exec_io() -> bool {
    let expected = [0xa7u8; PAGE_SIZE];
    let writer = open(
        AT_FDCWD,
        PATH,
        OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::RDWR,
        0o600,
    );
    if writer < 0 {
        return false;
    }
    let writer = writer as usize;
    if !write_all(writer, &expected) || close(writer) != 0 {
        return false;
    }

    let reader = open(AT_FDCWD, PATH, OpenFlags::RDONLY, 0);
    if reader < 0 {
        return false;
    }
    let reader = reader as usize;
    let mut actual = [0u8; PAGE_SIZE];
    let mut received = 0usize;
    while received < actual.len() {
        let count = read(reader, &mut actual[received..]);
        if count <= 0 {
            break;
        }
        received += count as usize;
    }
    received == PAGE_SIZE && close(reader) == 0 && actual == expected
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[ext4_invalidation_exec_test] start workers={}", WORKERS);
    let _ = unlinkat(AT_FDCWD, PATH, 0);
    let child = fork();
    if child == 0 {
        exec_while_invalidating();
    }
    if child < 0 {
        println!("[ext4_invalidation_exec_test] FAIL: fork={}", child);
        return 1;
    }

    let mut status = -1;
    let waited = waitpid(child as usize, &mut status);
    let io_ok = waited == child && status == 0 && verify_post_exec_io();
    let _ = unlinkat(AT_FDCWD, PATH, 0);
    if io_ok {
        println!("[ext4_invalidation_exec_test] PASS");
        0
    } else {
        println!(
            "[ext4_invalidation_exec_test] FAIL: child={} waited={} status={:#x} io_ok={}",
            child, waited, status, io_ok
        );
        1
    }
}
