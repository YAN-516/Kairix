#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use user_lib::{exit, fork, getpid, gettid, thread_create, waitpid, yield_};

const WORKERS: usize = 4;
static STARTED: AtomicUsize = AtomicUsize::new(0);
static START_FORK: AtomicBool = AtomicBool::new(false);
static CHILD_PID: AtomicUsize = AtomicUsize::new(0);

extern "C" fn worker(index: usize) -> ! {
    STARTED.fetch_add(1, Ordering::Release);
    while !START_FORK.load(Ordering::Acquire) {
        yield_();
    }

    if index == 0 {
        let child = fork();
        if child == 0 {
            let pid = getpid();
            let tid = gettid();
            println!(
                "[multithread_fork_test] child after fork: pid={} tid={}",
                pid, tid
            );
            exit(if pid == tid && pid > 0 { 0 } else { 3 });
        }
        if child < 0 {
            CHILD_PID.store(usize::MAX, Ordering::Release);
        } else {
            CHILD_PID.store(child as usize, Ordering::Release);
        }
    }

    loop {
        yield_();
    }
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[multithread_fork_test] start: workers={}", WORKERS);
    for index in 0..WORKERS {
        let tid = thread_create(worker, index);
        if tid < 0 {
            println!(
                "[multithread_fork_test] FAIL: worker={} create_ret={}",
                index, tid
            );
            return 1;
        }
    }

    while STARTED.load(Ordering::Acquire) != WORKERS {
        yield_();
    }
    START_FORK.store(true, Ordering::Release);

    let child = loop {
        let child = CHILD_PID.load(Ordering::Acquire);
        if child != 0 {
            break child;
        }
        yield_();
    };
    if child == usize::MAX {
        println!("[multithread_fork_test] FAIL: fork returned an error");
        return 1;
    }

    let mut status = 0;
    let waited = waitpid(child, &mut status);
    if waited == child as isize && status == 0 {
        println!("[multithread_fork_test] PASS");
        0
    } else {
        println!(
            "[multithread_fork_test] FAIL: child={} waited={} status={}",
            child, waited, status
        );
        1
    }
}
