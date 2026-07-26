#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{close, execve, exit, fork, pipe, read, waitpid, write};

const WORKERS: usize = 16;
const ROUNDS: usize = 32;
const TARGET: &str = "/cagent_exec_stress_target";

fn child_wait_and_exec(read_fd: i32, write_fd: i32) -> ! {
    let _ = close(write_fd as usize);
    let mut token = [0u8; 1];
    if read(read_fd as usize, &mut token) != 1 {
        exit(120);
    }
    let _ = close(read_fd as usize);
    let ret = execve(TARGET, &["cagent_exec_stress_target"], &[]);
    println!("[cagent_exec_stress_test] execve returned {}", ret);
    exit(121);
}

fn run_round(round: usize) -> bool {
    let mut barrier = [-1i32; 2];
    if pipe(&mut barrier) != 0 {
        println!("[cagent_exec_stress_test] round={} pipe failed", round);
        return false;
    }

    let mut children = [-1isize; WORKERS];
    let mut created = 0usize;
    for slot in 0..WORKERS {
        let child = fork();
        if child == 0 {
            child_wait_and_exec(barrier[0], barrier[1]);
        }
        if child < 0 {
            println!(
                "[cagent_exec_stress_test] round={} slot={} fork={}",
                round, slot, child
            );
            break;
        }
        children[slot] = child;
        created += 1;
    }

    let _ = close(barrier[0] as usize);
    let tokens = [0x5au8; WORKERS];
    let released = write(barrier[1] as usize, &tokens[..created]);
    let _ = close(barrier[1] as usize);
    let mut passed = released == created as isize && created == WORKERS;

    for &child in &children[..created] {
        let mut status = -1;
        let waited = waitpid(child as usize, &mut status);
        if waited != child || status != 0 {
            println!(
                "[cagent_exec_stress_test] round={} child={} waited={} status={:#x} signal={}",
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
        "[cagent_exec_stress_test] start workers={} rounds={}",
        WORKERS, ROUNDS
    );
    for round in 1..=ROUNDS {
        if !run_round(round) {
            println!("[cagent_exec_stress_test] FAIL round={}", round);
            return 1;
        }
        if round % 4 == 0 {
            println!("[cagent_exec_stress_test] round={} PASS", round);
        }
    }
    println!("[cagent_exec_stress_test] PASS");
    0
}
