#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    AT_FDCWD, OpenFlags, close, execve, exit, fork, open, pipe, read, sync, unlinkat, waitpid,
    write,
};

const WORKERS: usize = 16;
const ROUNDS: usize = 32;
const TARGET: &str = "/cagent_exec_stress_target";
const SCRIPT: &str = "/cagent_exec_stress_script.sh";
const SCRIPT_DATA: &[u8] = b"#!/bin/busybox sh\nexit 0\n";

fn create_script() -> bool {
    let fd = open(
        AT_FDCWD,
        SCRIPT,
        OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::WRONLY,
        0o755,
    );
    if fd < 0 {
        return false;
    }
    let mut done = 0usize;
    while done < SCRIPT_DATA.len() {
        let written = write(fd as usize, &SCRIPT_DATA[done..]);
        if written <= 0 {
            let _ = close(fd as usize);
            return false;
        }
        done += written as usize;
    }
    close(fd as usize) == 0 && sync() == 0
}

fn child_wait_and_exec(read_fd: i32, write_fd: i32, script: bool) -> ! {
    let _ = close(write_fd as usize);
    let mut token = [0u8; 1];
    if read(read_fd as usize, &mut token) != 1 {
        exit(120);
    }
    let _ = close(read_fd as usize);
    let ret = if script {
        execve(SCRIPT, &["cagent_exec_stress_script.sh"], &[])
    } else {
        execve(TARGET, &["cagent_exec_stress_target"], &[])
    };
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
            child_wait_and_exec(barrier[0], barrier[1], slot % 2 != 0);
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
    if !create_script() {
        println!("[cagent_exec_stress_test] FAIL create script");
        return 1;
    }
    for round in 1..=ROUNDS {
        if !run_round(round) {
            let _ = unlinkat(AT_FDCWD, SCRIPT, 0);
            println!("[cagent_exec_stress_test] FAIL round={}", round);
            return 1;
        }
        if round % 4 == 0 {
            println!("[cagent_exec_stress_test] round={} PASS", round);
        }
    }
    let _ = unlinkat(AT_FDCWD, SCRIPT, 0);
    println!("[cagent_exec_stress_test] PASS");
    0
}
