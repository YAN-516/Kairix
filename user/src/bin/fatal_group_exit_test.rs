#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::hint::spin_loop;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use user_lib::{exit, fork, futex, getpid, kill, thread_create, waitpid, yield_};

const FUTEX_WAIT: i32 = 0;
const SIGSEGV: i32 = 11;

static STARTED: AtomicUsize = AtomicUsize::new(0);
static FUTEX_WORD: AtomicU32 = AtomicU32::new(0);

extern "C" fn busy_sibling(_: usize) -> ! {
    STARTED.fetch_add(1, Ordering::Release);
    loop {
        spin_loop();
    }
}

extern "C" fn blocked_sibling(_: usize) -> ! {
    STARTED.fetch_add(1, Ordering::Release);
    let addr = &FUTEX_WORD as *const AtomicU32 as *mut u32;
    loop {
        let ret = futex(addr, FUTEX_WAIT, 0, 0, null_mut(), 0);
        if ret != 0 && ret != -4 && ret != -11 {
            exit(20);
        }
    }
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[fatal_group_exit_test] start");
    let child = fork();
    if child < 0 {
        println!("[fatal_group_exit_test] FAIL: fork={}", child);
        return 1;
    }
    if child == 0 {
        let busy_tid = thread_create(busy_sibling, 0);
        let blocked_tid = thread_create(blocked_sibling, 0);
        if busy_tid < 0 || blocked_tid < 0 {
            exit(10);
        }
        while STARTED.load(Ordering::Acquire) != 2 {
            yield_();
        }
        // Give the runnable sibling time to become current on another CPU and
        // the second sibling time to enter FUTEX_WAIT before group teardown.
        for _ in 0..128 {
            yield_();
        }
        if kill(getpid(), SIGSEGV as usize) != 0 {
            exit(11);
        }
        // A fatal signal must not return this thread group to userspace.
        exit(12);
    }

    let mut status = 0;
    let waited = waitpid(child as usize, &mut status);
    let terminating_signal = status & 0x7f;
    println!(
        "[fatal_group_exit_test] child={} waited={} status={:#x} signal={}",
        child, waited, status, terminating_signal,
    );
    if waited == child && terminating_signal == SIGSEGV {
        println!("[fatal_group_exit_test] PASS");
        0
    } else {
        println!("[fatal_group_exit_test] FAIL");
        1
    }
}
