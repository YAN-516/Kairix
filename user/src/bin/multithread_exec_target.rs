#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{getpid, gettid};

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    let pid = getpid();
    let tid = gettid();
    println!(
        "[multithread_exec_target] after exec: pid={} tid={}",
        pid, tid
    );
    if pid == tid && pid > 0 {
        println!("[multithread_exec_target] PASS");
        0
    } else {
        println!("[multithread_exec_target] FAIL");
        1
    }
}
