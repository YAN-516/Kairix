#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::sync::atomic::{AtomicUsize, Ordering};
use user_lib::{execve, thread_create, yield_};

const WORKERS: usize = 8;
static STARTED: AtomicUsize = AtomicUsize::new(0);

extern "C" fn parked_worker(_arg: usize) -> ! {
    STARTED.fetch_add(1, Ordering::Release);
    loop {
        yield_();
    }
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[multithread_exec_test] start: workers={}", WORKERS);
    for worker in 0..WORKERS {
        let tid = thread_create(parked_worker, worker);
        if tid < 0 {
            println!(
                "[multithread_exec_test] FAIL: worker={} create_ret={}",
                worker, tid
            );
            return 1;
        }
    }
    while STARTED.load(Ordering::Acquire) != WORKERS {
        yield_();
    }
    println!("[multithread_exec_test] all workers started; exec");
    let argv = ["multithread_exec_target"];
    let ret = execve("/multithread_exec_target", &argv, &[]);
    println!("[multithread_exec_test] FAIL: execve returned {}", ret);
    1
}
