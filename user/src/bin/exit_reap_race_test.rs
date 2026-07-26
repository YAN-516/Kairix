#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{exit, fork, waitpid, yield_};

const ROUNDS: usize = 32;
const CHILDREN_PER_ROUND: usize = 8;

fn reap_created_children(pids: &[isize], created: usize) -> bool {
    let mut ok = true;
    for &pid in &pids[..created] {
        let mut status = -1;
        let waited = waitpid(pid as usize, &mut status);
        if waited != pid || status != 0 {
            println!(
                "[exit_reap_race_test] wait mismatch: pid={} waited={} status={}",
                pid, waited, status
            );
            ok = false;
        }
    }
    ok
}

fn reap_delayed_child(round: usize) -> bool {
    let pid = fork();
    if pid == 0 {
        // Let the parent publish its blocking wait before exit reaches the
        // deferred idle-stack cleanup phase.
        for _ in 0..16 {
            let _ = yield_();
        }
        exit(0);
    }
    if pid < 0 {
        println!(
            "[exit_reap_race_test] delayed fork failed: round={} ret={}",
            round, pid
        );
        return false;
    }

    let mut status = -1;
    let waited = waitpid(pid as usize, &mut status);
    if waited != pid || status != 0 {
        println!(
            "[exit_reap_race_test] delayed wait mismatch: round={} pid={} waited={} status={}",
            round, pid, waited, status
        );
        return false;
    }
    true
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!(
        "[exit_reap_race_test] start: rounds={} children_per_round={}",
        ROUNDS, CHILDREN_PER_ROUND
    );

    for round in 0..ROUNDS {
        let mut pids = [-1isize; CHILDREN_PER_ROUND];
        let mut created = 0usize;
        for slot in 0..CHILDREN_PER_ROUND {
            let pid = fork();
            if pid == 0 {
                exit(0);
            }
            if pid < 0 {
                println!(
                    "[exit_reap_race_test] fork failed: round={} slot={} ret={}",
                    round, slot, pid
                );
                let _ = reap_created_children(&pids, created);
                return 1;
            }
            pids[slot] = pid;
            created += 1;
        }

        if !reap_created_children(&pids, created) {
            return 1;
        }
        if !reap_delayed_child(round) {
            return 1;
        }
    }

    println!("[exit_reap_race_test] PASS");
    0
}
