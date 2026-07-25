#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::ptr::null_mut;
use user_lib::{mmap, munmap, shmat, shmctl, shmget};

const PAGE_SIZE: usize = 4096;
const PROT_READ: usize = 0x1;
const PROT_WRITE: usize = 0x2;
const MAP_PRIVATE: usize = 0x02;
const MAP_FIXED: usize = 0x10;
const MAP_ANONYMOUS: usize = 0x20;
const IPC_CREAT: i32 = 0o1000;
const IPC_RMID: i32 = 0;
const IPC_SET: i32 = 1;
const IPC_STAT: i32 = 2;

#[cfg(target_arch = "riscv64")]
const KERNEL_ADDRESS: usize = 0xffff_ffc2_7fe0_0000;
#[cfg(target_arch = "loongarch64")]
const KERNEL_ADDRESS: usize = 0x9000_0002_7fe0_0000;

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[kernel_half_mmap_guard_test] start");

    let mapped = mmap(
        KERNEL_ADDRESS,
        PAGE_SIZE,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_FIXED | MAP_ANONYMOUS,
        -1,
        0,
    );
    if mapped != -12 {
        println!(
            "[kernel_half_mmap_guard_test] FAIL mmap ret={} expected=-12",
            mapped
        );
        return 1;
    }

    let unmapped = munmap(KERNEL_ADDRESS, PAGE_SIZE);
    if unmapped != -22 {
        println!(
            "[kernel_half_mmap_guard_test] FAIL munmap ret={} expected=-22",
            unmapped
        );
        return 2;
    }

    let shmid = shmget(0, PAGE_SIZE, IPC_CREAT | 0o600);
    if shmid < 0 {
        println!("[kernel_half_mmap_guard_test] FAIL shmget ret={}", shmid);
        return 3;
    }
    let kernel_ptr = KERNEL_ADDRESS as *mut u8;
    let stat = shmctl(shmid as usize, IPC_STAT, kernel_ptr);
    let set = shmctl(shmid as usize, IPC_SET, kernel_ptr);
    if stat != -14 || set != -14 {
        let cleanup = shmctl(shmid as usize, IPC_RMID, null_mut());
        println!(
            "[kernel_half_mmap_guard_test] FAIL shmctl stat={} set={} expected=-14 cleanup={}",
            stat, set, cleanup
        );
        return 4;
    }
    let attached = shmat(shmid as usize, KERNEL_ADDRESS, 0);
    let cleanup = shmctl(shmid as usize, IPC_RMID, null_mut());
    if attached != -22 || cleanup != 0 {
        println!(
            "[kernel_half_mmap_guard_test] FAIL shmat ret={} expected=-22 cleanup={}",
            attached, cleanup
        );
        return 5;
    }

    println!("[kernel_half_mmap_guard_test] PASS");
    0
}
