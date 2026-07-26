#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{close, exit, fork, pipe, read, sleep, waitpid, write};

const ROUNDS: usize = 32;
const PAYLOAD: &[u8] = b"3628800\n";

fn run_round(round: usize) -> bool {
    let mut fds = [0i32; 2];
    if pipe(&mut fds) != 0 {
        println!("[pipe_sigchld_regression] round={} pipe failed", round);
        return false;
    }

    let signal_child = fork();
    if signal_child < 0 {
        println!(
            "[pipe_sigchld_regression] round={} fork failed: {}",
            round, signal_child
        );
        let _ = close(fds[0] as usize);
        let _ = close(fds[1] as usize);
        return false;
    }

    if signal_child == 0 {
        let _ = close(fds[0] as usize);
        let _ = close(fds[1] as usize);
        // Let the parent enter read(2), then generate a default-ignored
        // SIGCHLD while another process still owns the write end.
        sleep(1);
        exit(0);
    }

    let writer_child = fork();
    if writer_child < 0 {
        println!(
            "[pipe_sigchld_regression] round={} writer fork failed: {}",
            round, writer_child
        );
        let _ = close(fds[0] as usize);
        let _ = close(fds[1] as usize);
        let mut status = 0i32;
        let _ = waitpid(signal_child as usize, &mut status);
        return false;
    }

    if writer_child == 0 {
        let _ = close(fds[0] as usize);
        sleep(5);
        let written = write(fds[1] as usize, PAYLOAD);
        // Immediate exit makes pipe readiness race a second SIGCHLD, matching
        // a shell builtin executed through glibc popen(3).
        exit(if written == PAYLOAD.len() as isize {
            0
        } else {
            2
        });
    }

    let _ = close(fds[1] as usize);
    let mut received = [0u8; PAYLOAD.len()];
    let read_ret = read(fds[0] as usize, &mut received);
    let _ = close(fds[0] as usize);

    let mut signal_status = 0i32;
    let signal_waited = waitpid(signal_child as usize, &mut signal_status);
    let mut writer_status = 0i32;
    let writer_waited = waitpid(writer_child as usize, &mut writer_status);
    let passed = read_ret == PAYLOAD.len() as isize
        && received == PAYLOAD
        && signal_waited == signal_child
        && signal_status == 0
        && writer_waited == writer_child
        && writer_status == 0;
    if !passed {
        println!(
            "[pipe_sigchld_regression] round={} read={} received={:?} signal_waited={} signal_status={} writer_waited={} writer_status={}",
            round, read_ret, received, signal_waited, signal_status, writer_waited, writer_status
        );
    }
    passed
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[pipe_sigchld_regression] start rounds={}", ROUNDS);
    for round in 1..=ROUNDS {
        if !run_round(round) {
            println!("[pipe_sigchld_regression] FAIL");
            return 1;
        }
    }
    println!("[pipe_sigchld_regression] PASS");
    0
}
