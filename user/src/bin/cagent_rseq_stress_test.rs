#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::arch::asm;
use core::ptr::read_volatile;
use user_lib::{exit, fork, waitpid, yield_};

const SYS_RSEQ: usize = 293;
const RSEQ_FLAG_UNREGISTER: usize = 1;
const RSEQ_SIGNATURE: u32 = 0x5305_3053;
const WORKERS: usize = 16;
const ROUNDS: usize = 8;
const YIELDS_PER_CHILD: usize = 2048;

#[repr(C, align(32))]
struct Rseq {
    cpu_id_start: u32,
    cpu_id: u32,
    rseq_cs: u64,
    flags: u32,
    node_id: u32,
    mm_cid: u32,
    padding: u32,
}

#[cfg(target_arch = "riscv64")]
#[inline(always)]
unsafe fn raw_syscall(id: usize, args: [usize; 6]) -> isize {
    let ret: isize;
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

fn child_work(round: usize, slot: usize) -> i32 {
    let mut area = Rseq {
        cpu_id_start: u32::MAX,
        cpu_id: u32::MAX,
        rseq_cs: 0,
        flags: 0,
        node_id: u32::MAX,
        mm_cid: u32::MAX,
        padding: 0,
    };
    let address = &mut area as *mut Rseq as usize;
    let len = core::mem::size_of::<Rseq>();
    let registered =
        unsafe { raw_syscall(SYS_RSEQ, [address, len, 0, RSEQ_SIGNATURE as usize, 0, 0]) };
    if registered != 0 {
        println!(
            "[cagent_rseq_stress_test] register FAIL round={} slot={} ret={}",
            round, slot, registered
        );
        return 10;
    }

    let mut checksum = 0u64;
    for iteration in 0..YIELDS_PER_CHILD {
        let _ = yield_();
        let cpu = unsafe { read_volatile(&raw const area.cpu_id) };
        let cpu_start = unsafe { read_volatile(&raw const area.cpu_id_start) };
        let mm_cid = unsafe { read_volatile(&raw const area.mm_cid) };
        let rseq_cs = unsafe { read_volatile(&raw const area.rseq_cs) };
        if cpu == u32::MAX || cpu_start == u32::MAX || mm_cid == u32::MAX || rseq_cs != 0 {
            println!(
                "[cagent_rseq_stress_test] state FAIL round={} slot={} iteration={} start={} cpu={} mm_cid={} cs={:#x}",
                round, slot, iteration, cpu_start, cpu, mm_cid, rseq_cs
            );
            return 11;
        }
        checksum = checksum.wrapping_add(cpu as u64).rotate_left(1) ^ mm_cid as u64;
    }

    let unregistered = unsafe {
        raw_syscall(
            SYS_RSEQ,
            [
                address,
                len,
                RSEQ_FLAG_UNREGISTER,
                RSEQ_SIGNATURE as usize,
                0,
                0,
            ],
        )
    };
    core::hint::black_box(checksum);
    if unregistered != 0 || area.cpu_id != u32::MAX {
        println!(
            "[cagent_rseq_stress_test] unregister FAIL round={} slot={} ret={} cpu={}",
            round, slot, unregistered, area.cpu_id
        );
        return 12;
    }
    0
}

fn run_round(round: usize) -> bool {
    let mut children = [-1isize; WORKERS];
    let mut created = 0usize;
    for slot in 0..WORKERS {
        let child = fork();
        if child == 0 {
            exit(child_work(round, slot));
        }
        if child < 0 {
            println!(
                "[cagent_rseq_stress_test] fork FAIL round={} slot={} ret={}",
                round, slot, child
            );
            break;
        }
        children[slot] = child;
        created += 1;
    }

    let mut passed = created == WORKERS;
    for &child in &children[..created] {
        let mut status = -1;
        let waited = waitpid(child as usize, &mut status);
        if waited != child || status != 0 {
            println!(
                "[cagent_rseq_stress_test] child FAIL round={} pid={} waited={} status={:#x} signal={}",
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
        "[cagent_rseq_stress_test] start workers={} rounds={} yields={}",
        WORKERS, ROUNDS, YIELDS_PER_CHILD
    );
    for round in 1..=ROUNDS {
        if !run_round(round) {
            println!("[cagent_rseq_stress_test] FAIL round={}", round);
            return 1;
        }
        println!("[cagent_rseq_stress_test] round={} PASS", round);
    }
    println!("[cagent_rseq_stress_test] PASS");
    0
}
