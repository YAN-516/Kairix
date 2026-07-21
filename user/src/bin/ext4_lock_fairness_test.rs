#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    AT_FDCWD, OpenFlags, close, exit, fork, get_time, open, unlinkat, waitpid, write, yield_,
};

const PATH: &str = "/ext4_lock_fairness_test.bin";
const WORKERS: usize = 8;
const WORKER_OPENS: usize = 512;
const VICTIM_OPENS: usize = 64;
const MAX_ACCEPTABLE_WAIT_MS: isize = 30_000;

fn open_once() -> bool {
    let fd = open(AT_FDCWD, PATH, OpenFlags::RDONLY, 0);
    fd >= 0 && close(fd as usize) == 0
}

fn worker_main() -> ! {
    for round in 0..WORKER_OPENS {
        if !open_once() {
            exit(2);
        }
        if round & 7 == 0 {
            yield_();
        }
    }
    exit(0)
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!(
        "[ext4_lock_fairness_test] start workers={} worker_opens={} victim_opens={}",
        WORKERS, WORKER_OPENS, VICTIM_OPENS
    );
    let _ = unlinkat(AT_FDCWD, PATH, 0);
    let fd = open(
        AT_FDCWD,
        PATH,
        OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::RDWR,
        0o600,
    );
    let setup_ok = if fd >= 0 {
        let write_ret = write(fd as usize, b"fairness\n");
        let close_ret = close(fd as usize);
        write_ret == 9 && close_ret == 0
    } else {
        false
    };
    if !setup_ok {
        println!("[ext4_lock_fairness_test] FAIL: setup fd={}", fd);
        let _ = unlinkat(AT_FDCWD, PATH, 0);
        return 1;
    }

    let mut children = [0usize; WORKERS];
    for slot in children.iter_mut() {
        let child = fork();
        if child == 0 {
            worker_main();
        }
        if child < 0 {
            println!("[ext4_lock_fairness_test] FAIL: fork={}", child);
            let _ = unlinkat(AT_FDCWD, PATH, 0);
            return 2;
        }
        *slot = child as usize;
    }

    let mut max_wait_ms = 0isize;
    for _ in 0..VICTIM_OPENS {
        let started = get_time();
        if !open_once() {
            println!("[ext4_lock_fairness_test] FAIL: victim open");
            let _ = unlinkat(AT_FDCWD, PATH, 0);
            return 3;
        }
        let finished = get_time();
        if started < 0 || finished < 0 {
            println!("[ext4_lock_fairness_test] FAIL: clock");
            let _ = unlinkat(AT_FDCWD, PATH, 0);
            return 3;
        }
        let elapsed = finished.saturating_sub(started);
        max_wait_ms = max_wait_ms.max(elapsed);
        yield_();
    }

    let mut children_ok = true;
    for child in children {
        let mut status = -1;
        if waitpid(child, &mut status) != child as isize || status != 0 {
            children_ok = false;
        }
    }
    let _ = unlinkat(AT_FDCWD, PATH, 0);

    println!(
        "[ext4_lock_fairness_test] max_wait_ms={} children_ok={}",
        max_wait_ms, children_ok
    );
    if children_ok && max_wait_ms < MAX_ACCEPTABLE_WAIT_MS {
        println!("[ext4_lock_fairness_test] PASS");
        0
    } else {
        println!("[ext4_lock_fairness_test] FAIL: lock starvation");
        4
    }
}
