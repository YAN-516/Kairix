#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use user_lib::{exit, fork, thread_create, waitpid, yield_};

static WORKERS_STARTED: AtomicUsize = AtomicUsize::new(0);
static WORKERS_AT_EXIT: AtomicUsize = AtomicUsize::new(0);
static LEADER_MAY_EXIT: AtomicBool = AtomicBool::new(false);

extern "C" fn survivor(exit_code: usize) -> ! {
    WORKERS_STARTED.fetch_add(1, Ordering::AcqRel);
    while !LEADER_MAY_EXIT.load(Ordering::Acquire) {
        yield_();
    }
    // Give the leader enough scheduling opportunities to execute SYS_exit(7)
    // before this final live thread exits with the process-visible status 0.
    for _ in 0..256 {
        yield_();
    }
    WORKERS_AT_EXIT.fetch_add(1, Ordering::AcqRel);
    while WORKERS_AT_EXIT.load(Ordering::Acquire) != 2 {
        yield_();
    }
    println!("[leader_exit_test] sibling survived leader SYS_exit");
    exit(exit_code as i32)
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[leader_exit_test] start");
    let child = fork();
    if child < 0 {
        println!("[leader_exit_test] FAIL: fork={}", child);
        return 1;
    }
    if child == 0 {
        let tid1 = thread_create(survivor, 0);
        let tid2 = thread_create(survivor, 8);
        if tid1 < 0 || tid2 < 0 {
            exit(2);
        }
        while WORKERS_STARTED.load(Ordering::Acquire) != 2 {
            yield_();
        }
        LEADER_MAY_EXIT.store(true, Ordering::Release);
        exit(7);
    }

    let mut status = -1;
    let waited = waitpid(child as usize, &mut status);
    if waited == child && (status == 0 || status == 8) {
        println!("[leader_exit_test] PASS");
        0
    } else {
        println!(
            "[leader_exit_test] FAIL: child={} waited={} status={}",
            child, waited, status
        );
        1
    }
}
