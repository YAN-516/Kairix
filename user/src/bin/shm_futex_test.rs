#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::arch::asm;
use core::sync::atomic::{AtomicU32, Ordering};
use user_lib::{exit, fork, waitpid};

const PAGE_SIZE: usize = 4096;
const WORKERS: usize = 4;
const SEGMENT_ROUNDS: usize = 8;
const BARRIER_ROUNDS: usize = 64;
const CHILD_OK: i32 = 42;

const SYS_FUTEX: usize = 98;
const SYS_SHMGET: usize = 194;
const SYS_SHMCTL: usize = 195;
const SYS_SHMAT: usize = 196;
const SYS_SHMDT: usize = 197;

const FUTEX_WAIT: usize = 0;
const FUTEX_WAKE: usize = 1;
const IPC_CREAT: usize = 0o1000;
const IPC_RMID: usize = 0;

#[repr(C, align(64))]
struct SharedBarrier {
    generation: AtomicU32,
    arrived: AtomicU32,
    failures: AtomicU32,
    values: [AtomicU32; WORKERS],
}

#[cfg(target_arch = "riscv64")]
#[inline(always)]
unsafe fn raw_syscall(id: usize, args: [usize; 6]) -> isize {
    let mut ret: isize;
    unsafe {
        asm!(
            "ecall",
            inlateout("x10") args[0] => ret,
            in("x11") args[1],
            in("x12") args[2],
            in("x13") args[3],
            in("x14") args[4],
            in("x15") args[5],
            in("x17") id,
        );
    }
    ret
}

#[cfg(target_arch = "loongarch64")]
#[inline(always)]
unsafe fn raw_syscall(id: usize, args: [usize; 6]) -> isize {
    let ret: isize;
    unsafe {
        asm!(
            "syscall 0",
            inlateout("$a0") args[0] => ret,
            in("$a1") args[1],
            in("$a2") args[2],
            in("$a3") args[3],
            in("$a4") args[4],
            in("$a5") args[5],
            in("$a7") id,
        );
    }
    ret
}

fn shmget(size: usize) -> isize {
    unsafe { raw_syscall(SYS_SHMGET, [0, size, IPC_CREAT | 0o600, 0, 0, 0]) }
}

fn shmat(shmid: usize) -> isize {
    unsafe { raw_syscall(SYS_SHMAT, [shmid, 0, 0, 0, 0, 0]) }
}

fn shmctl_rmid(shmid: usize) -> isize {
    unsafe { raw_syscall(SYS_SHMCTL, [shmid, IPC_RMID, 0, 0, 0, 0]) }
}

fn shmdt(addr: usize) -> isize {
    unsafe { raw_syscall(SYS_SHMDT, [addr, 0, 0, 0, 0, 0]) }
}

fn futex(addr: *const AtomicU32, op: usize, val: u32) -> isize {
    unsafe { raw_syscall(SYS_FUTEX, [addr as usize, op, val as usize, 0, 0, 0]) }
}

fn wait_at_barrier(barrier: &SharedBarrier) -> bool {
    let generation = barrier.generation.load(Ordering::Acquire);
    let arrived = barrier.arrived.fetch_add(1, Ordering::AcqRel) + 1;
    if arrived == WORKERS as u32 {
        barrier.arrived.store(0, Ordering::Relaxed);
        barrier.generation.fetch_add(1, Ordering::Release);
        return futex(&barrier.generation, FUTEX_WAKE, u32::MAX) >= 0;
    }

    while barrier.generation.load(Ordering::Acquire) == generation {
        let ret = futex(&barrier.generation, FUTEX_WAIT, generation);
        if ret < 0 && ret != -4 && ret != -11 {
            return false;
        }
    }
    true
}

fn child_main(worker: usize, barrier: &SharedBarrier) -> ! {
    for round in 0..BARRIER_ROUNDS {
        barrier.values[worker].store((round + 1) as u32, Ordering::Release);
        if !wait_at_barrier(barrier) {
            barrier.failures.fetch_add(1, Ordering::Relaxed);
            exit(2);
        }
        for value in barrier.values.iter() {
            if value.load(Ordering::Acquire) != (round + 1) as u32 {
                barrier.failures.fetch_add(1, Ordering::Relaxed);
                exit(3);
            }
        }
        if !wait_at_barrier(barrier) {
            barrier.failures.fetch_add(1, Ordering::Relaxed);
            exit(4);
        }
    }
    exit(CHILD_OK);
}

fn page_is_zero(addr: usize) -> bool {
    let bytes = unsafe { core::slice::from_raw_parts(addr as *const u8, PAGE_SIZE) };
    bytes.iter().all(|byte| *byte == 0)
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!(
        "[shm_futex_test] start: segment_rounds={}, workers={}, barrier_rounds={}",
        SEGMENT_ROUNDS, WORKERS, BARRIER_ROUNDS
    );

    for segment_round in 0..SEGMENT_ROUNDS {
        let shmid = shmget(PAGE_SIZE);
        if shmid < 0 {
            println!(
                "[shm_futex_test] FAIL: round={} shmget={}",
                segment_round, shmid
            );
            return 1;
        }
        let addr = shmat(shmid as usize);
        if addr < 0 {
            println!(
                "[shm_futex_test] FAIL: round={} shmat={}",
                segment_round, addr
            );
            return 2;
        }
        let addr = addr as usize;
        if !page_is_zero(addr) {
            println!(
                "[shm_futex_test] FAIL: round={} new segment not zeroed",
                segment_round
            );
            return 3;
        }
        if shmctl_rmid(shmid as usize) < 0 {
            println!("[shm_futex_test] FAIL: round={} IPC_RMID", segment_round);
            return 4;
        }

        let barrier = unsafe { &*(addr as *const SharedBarrier) };
        let mut pids = [-1isize; WORKERS];
        for worker in 0..WORKERS {
            let pid = fork();
            if pid == 0 {
                child_main(worker, barrier);
            }
            if pid < 0 {
                println!(
                    "[shm_futex_test] FAIL: round={} fork worker={} ret={}",
                    segment_round, worker, pid
                );
                return 5;
            }
            pids[worker] = pid;
        }

        for pid in pids {
            let mut status = 0i32;
            let waited = waitpid(pid as usize, &mut status);
            let exit_code = (status >> 8) & 0xff;
            if waited != pid || (status & 0x7f) != 0 || exit_code != CHILD_OK {
                println!(
                    "[shm_futex_test] FAIL: round={} pid={} waited={} status={} exit={}",
                    segment_round, pid, waited, status, exit_code
                );
                return 6;
            }
        }
        if barrier.failures.load(Ordering::Acquire) != 0 {
            println!(
                "[shm_futex_test] FAIL: round={} shared failures={}",
                segment_round,
                barrier.failures.load(Ordering::Relaxed)
            );
            return 7;
        }

        // Make stale allocator contents deterministic.  The next segment must
        // still be zero-filled even if it receives this same physical page.
        unsafe { core::ptr::write_bytes(addr as *mut u8, 0xa5, PAGE_SIZE) };
        if shmdt(addr) < 0 {
            println!("[shm_futex_test] FAIL: round={} shmdt", segment_round);
            return 8;
        }
        println!("[shm_futex_test] round={} pass", segment_round + 1);
    }

    println!("[shm_futex_test] PASS");
    0
}
