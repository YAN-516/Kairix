#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::sync::atomic::{AtomicUsize, Ordering};
use user_lib::{
    SIG_BLOCK, SIG_SETMASK, SigAction, SignalSet, exit, fork, sigaction, sigprocmask, sigsuspend,
    waitpid, waitpid_options, yield_,
};

const SIGCHLD: i32 = 17;
const SA_RESTART: u32 = 0x1000_0000;
const WNOHANG: i32 = 1;
const WORKERS: usize = 16;
const ROUNDS: usize = 64;
static HANDLED: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn sigchld_handler(signal: i32) {
    if signal == SIGCHLD {
        HANDLED.fetch_add(1, Ordering::Relaxed);
    }
}

#[inline(never)]
fn touch_stack(seed: usize) -> usize {
    let mut stack = [0u8; 64 * 1024];
    let mut checksum = 0usize;
    for page in 0..16 {
        let offset = page * 4096;
        stack[offset] = seed.wrapping_add(page * 29) as u8;
        checksum = checksum.wrapping_add(stack[offset] as usize);
    }
    core::hint::black_box(checksum)
}

fn run_timeout_style_wait(round: usize) -> bool {
    let mut chld_set = SignalSet::empty();
    chld_set.add(SIGCHLD);
    let mut old_set = SignalSet::empty();
    if sigprocmask(SIG_BLOCK, Some(&chld_set), Some(&mut old_set)) != 0 {
        println!(
            "[cagent_sigchld_stress_test] timeout-style sigprocmask FAIL round={}",
            round
        );
        return false;
    }

    let child = fork();
    if child == 0 {
        let _ = sigprocmask(SIG_SETMASK, Some(&old_set), None);
        let _ = touch_stack(round);
        exit(0);
    }
    if child < 0 {
        let _ = sigprocmask(SIG_SETMASK, Some(&old_set), None);
        println!(
            "[cagent_sigchld_stress_test] timeout-style fork FAIL round={} ret={}",
            round, child
        );
        return false;
    }

    let mut status = -1;
    let passed = loop {
        let waited = waitpid_options(child, &mut status, WNOHANG);
        if waited == child {
            break status == 0;
        }
        if waited != 0 {
            println!(
                "[cagent_sigchld_stress_test] timeout-style wait FAIL round={} pid={} waited={}",
                round, child, waited
            );
            break false;
        }
        let suspended = sigsuspend(&old_set);
        if suspended != -4 {
            println!(
                "[cagent_sigchld_stress_test] timeout-style suspend FAIL round={} ret={}",
                round, suspended
            );
            break false;
        }
    };
    let _ = sigprocmask(SIG_SETMASK, Some(&old_set), None);
    if !passed {
        let _ = waitpid(child as usize, &mut status);
    }
    passed
}

fn run_round(round: usize) -> bool {
    let mut children = [-1isize; WORKERS];
    let mut created = 0usize;
    for slot in 0..WORKERS {
        let child = fork();
        if child == 0 {
            let checksum = touch_stack(round ^ slot);
            for _ in 0..8 {
                let _ = yield_();
            }
            exit(if checksum == usize::MAX { 2 } else { 0 });
        }
        if child < 0 {
            println!(
                "[cagent_sigchld_stress_test] fork FAIL round={} slot={} ret={}",
                round, slot, child
            );
            break;
        }
        children[slot] = child;
        created += 1;
    }

    let _ = touch_stack(round);
    for _ in 0..32 {
        let _ = yield_();
    }

    let mut passed = created == WORKERS;
    for &child in &children[..created] {
        let mut status = -1;
        let waited = waitpid(child as usize, &mut status);
        if waited != child || status != 0 {
            println!(
                "[cagent_sigchld_stress_test] child FAIL round={} pid={} waited={} status={:#x} signal={}",
                round,
                child,
                waited,
                status,
                status & 0x7f
            );
            passed = false;
        }
    }
    passed
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!(
        "[cagent_sigchld_stress_test] start workers={} rounds={}",
        WORKERS, ROUNDS
    );
    let mut action = SigAction::custom(sigchld_handler);
    action.sa_flags = SA_RESTART;
    if sigaction(SIGCHLD, Some(&action), None) != 0 {
        println!("[cagent_sigchld_stress_test] FAIL sigaction");
        return 1;
    }

    for round in 1..=ROUNDS {
        if !run_timeout_style_wait(round) {
            println!(
                "[cagent_sigchld_stress_test] FAIL timeout-style round={}",
                round
            );
            return 2;
        }
        if !run_round(round) {
            println!("[cagent_sigchld_stress_test] FAIL round={}", round);
            return 2;
        }
        if round % 8 == 0 {
            println!(
                "[cagent_sigchld_stress_test] round={} handled={}",
                round,
                HANDLED.load(Ordering::Relaxed)
            );
        }
    }

    let handled = HANDLED.load(Ordering::Relaxed);
    if handled == 0 {
        println!("[cagent_sigchld_stress_test] FAIL no signal handled");
        3
    } else {
        println!(
            "[cagent_sigchld_stress_test] PASS handled={} children={}",
            handled,
            WORKERS * ROUNDS
        );
        0
    }
}
