#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    exit, fork, membarrier, sched_getaffinity, sched_getscheduler, sched_setaffinity,
    sched_setscheduler, waitpid,
};

const MEMBARRIER_CMD_QUERY: i32 = 0;
const MEMBARRIER_CMD_PRIVATE_EXPEDITED: i32 = 8;
const MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED: i32 = 16;
const SCHED_NORMAL: i32 = 0;
const SCHED_FIFO: i32 = 1;
const SCHED_RESET_ON_FORK: i32 = 0x40000000;

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[sched_membarrier_test] start");

    let query = membarrier(MEMBARRIER_CMD_QUERY, 0);
    let before_register = membarrier(MEMBARRIER_CMD_PRIVATE_EXPEDITED, 0);
    let register = membarrier(MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED, 0);
    let barrier = membarrier(MEMBARRIER_CMD_PRIVATE_EXPEDITED, 0);
    let membarrier_ok =
        query & 0x18 == 0x18 && before_register == -1 && register == 0 && barrier == 0;

    let mut original_mask = 0u64;
    let get_original = sched_getaffinity(0, &mut original_mask);
    let one_cpu = original_mask & original_mask.wrapping_neg();
    let set_one = sched_setaffinity(0, &one_cpu);
    let mut observed_mask = 0u64;
    let get_one = sched_getaffinity(0, &mut observed_mask);
    let restore = sched_setaffinity(0, &original_mask);
    let affinity_ok = get_original == 8
        && original_mask != 0
        && set_one == 0
        && get_one == 8
        && observed_mask == one_cpu
        && restore == 0;

    let set_fifo = sched_setscheduler(0, SCHED_FIFO, 1);
    let fifo_policy = sched_getscheduler(0);
    let set_normal = sched_setscheduler(0, SCHED_NORMAL, 0);
    let normal_policy = sched_getscheduler(0);
    let realtime_ok = set_fifo == 0
        && fifo_policy == SCHED_FIFO as isize
        && set_normal == 0
        && normal_policy == SCHED_NORMAL as isize;

    let set_reset = sched_setscheduler(0, SCHED_FIFO | SCHED_RESET_ON_FORK, 1);
    let parent_reset_policy = sched_getscheduler(0);
    let child = fork();
    if child == 0 {
        let child_policy = sched_getscheduler(0);
        exit(if child_policy == SCHED_NORMAL as isize {
            0
        } else {
            1
        });
    }
    let mut child_status = -1;
    let waited = if child > 0 {
        waitpid(child as usize, &mut child_status)
    } else {
        -1
    };
    let reset_on_fork_ok = set_reset == 0
        && parent_reset_policy == (SCHED_FIFO | SCHED_RESET_ON_FORK) as isize
        && waited == child
        && child_status == 0;
    let _ = sched_setscheduler(0, SCHED_NORMAL, 0);

    println!(
        "[sched_membarrier_test] membarrier={} affinity={} realtime={} reset_on_fork={}",
        membarrier_ok, affinity_ok, realtime_ok, reset_on_fork_ok
    );
    if membarrier_ok && affinity_ok && realtime_ok && reset_on_fork_ok {
        println!("[sched_membarrier_test] PASS");
        0
    } else {
        println!("[sched_membarrier_test] FAIL");
        1
    }
}
