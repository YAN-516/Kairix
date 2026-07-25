#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::ptr::{read_unaligned, write_bytes, write_unaligned};
use user_lib::{fork, mmap, munmap, sched_getaffinity_raw, sched_setscheduler_raw, waitpid};

const PAGE_SIZE: usize = 4096;
const PROT_READ: usize = 1;
const PROT_WRITE: usize = 2;
const MAP_PRIVATE: usize = 2;
const MAP_ANONYMOUS: usize = 0x20;
const SCHED_OTHER: i32 = 0;

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[cross_page_user_struct_test] start");
    let mapping = mmap(
        0,
        PAGE_SIZE * 2,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0,
    );
    if mapping < 0 {
        println!("[cross_page_user_struct_test] FAIL mmap={}", mapping);
        return 1;
    }
    let base = mapping as usize;

    let param = (base + PAGE_SIZE - 2) as *mut i32;
    unsafe { write_unaligned(param, 0) };
    let input_ret = sched_setscheduler_raw(0, SCHED_OTHER, param);
    if input_ret != 0 {
        println!(
            "[cross_page_user_struct_test] FAIL cross-page input={}",
            input_ret
        );
        let _ = munmap(base, PAGE_SIZE * 2);
        return 2;
    }

    let mask = (base + PAGE_SIZE - 4) as *mut u64;
    unsafe { write_bytes(mask.cast::<u8>(), 0x5a, core::mem::size_of::<u64>()) };
    let child = fork();
    if child == 0 {
        let ret = sched_getaffinity_raw(0, mask);
        let value = unsafe { read_unaligned(mask) };
        if ret != 8 || value == 0 {
            println!(
                "[cross_page_user_struct_test] FAIL child output ret={} value={:#x}",
                ret, value
            );
            return 3;
        }
        return 0;
    }
    if child < 0 {
        println!("[cross_page_user_struct_test] FAIL fork={}", child);
        let _ = munmap(base, PAGE_SIZE * 2);
        return 4;
    }

    let mut status = -1;
    let waited = waitpid(child as usize, &mut status);
    let parent_value = unsafe { read_unaligned(mask) };
    let _ = munmap(base, PAGE_SIZE * 2);
    if waited != child || status != 0 || parent_value != 0x5a5a_5a5a_5a5a_5a5a {
        println!(
            "[cross_page_user_struct_test] FAIL COW wait={} status={} parent={:#x}",
            waited, status, parent_value
        );
        return 5;
    }

    println!("[cross_page_user_struct_test] PASS");
    0
}
