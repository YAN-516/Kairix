#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use user_lib::{execve, exit, thread_create, yield_};

const WORKERS: usize = 4;
static STARTED: AtomicUsize = AtomicUsize::new(0);
static START_EXEC: AtomicBool = AtomicBool::new(false);

extern "C" fn worker(index: usize) -> ! {
    STARTED.fetch_add(1, Ordering::Release);
    while !START_EXEC.load(Ordering::Acquire) {
        yield_();
    }
    if index == 0 {
        let argv = ["multithread_exec_target"];
        let ret = execve("/multithread_exec_target", &argv, &[]);
        println!("[nonleader_exec_test] FAIL: execve returned {}", ret);
        exit(2);
    }
    loop {
        yield_();
    }
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[nonleader_exec_test] start: workers={}", WORKERS);
    for index in 0..WORKERS {
        let tid = thread_create(worker, index);
        if tid < 0 {
            println!(
                "[nonleader_exec_test] FAIL: worker={} create_ret={}",
                index, tid
            );
            return 1;
        }
    }
    while STARTED.load(Ordering::Acquire) != WORKERS {
        yield_();
    }
    println!("[nonleader_exec_test] worker 0 will exec");
    START_EXEC.store(true, Ordering::Release);
    loop {
        yield_();
    }
}
