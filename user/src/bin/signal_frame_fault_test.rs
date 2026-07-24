#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    SA_ONSTACK, SIGUSR1, SigAction, StackT, exit, fork, getpid, kill, mmap, munmap, sigaction,
    sigaltstack, waitpid,
};

const PAGE_SIZE: usize = 4096;
const ALT_STACK_SIZE: usize = 4 * PAGE_SIZE;
const PROT_READ: usize = 0x1;
const PROT_WRITE: usize = 0x2;
const MAP_PRIVATE: usize = 0x02;
const MAP_ANONYMOUS: usize = 0x20;
const SIGSEGV: i32 = 11;

unsafe extern "C" fn unreachable_handler(_signal: i32) {
    exit(90);
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("[signal_frame_fault_test] start");
    let child = fork();
    if child < 0 {
        println!("[signal_frame_fault_test] FAIL: fork={}", child);
        return 1;
    }
    if child == 0 {
        let mapping = mmap(
            0,
            ALT_STACK_SIZE,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS,
            -1,
            0,
        );
        if mapping < 0 {
            exit(10);
        }
        let stack = StackT {
            ss_sp: mapping as usize,
            ss_flags: 0,
            ss_size: ALT_STACK_SIZE,
        };
        if sigaltstack(Some(&stack), None) != 0 {
            exit(11);
        }
        if munmap(mapping as usize, ALT_STACK_SIZE) != 0 {
            exit(12);
        }
        let mut action = SigAction::custom(unreachable_handler);
        action.sa_flags = SA_ONSTACK;
        if sigaction(SIGUSR1, Some(&action), None) != 0 {
            exit(13);
        }
        if kill(getpid(), SIGUSR1 as usize) != 0 {
            exit(14);
        }
        exit(15);
    }

    let mut status = 0;
    let waited = waitpid(child as usize, &mut status);
    let terminating_signal = status & 0x7f;
    println!(
        "[signal_frame_fault_test] child={} waited={} status={:#x} signal={}",
        child, waited, status, terminating_signal,
    );
    if waited == child && terminating_signal == SIGSEGV {
        println!("[signal_frame_fault_test] PASS");
        0
    } else {
        println!("[signal_frame_fault_test] FAIL");
        1
    }
}
