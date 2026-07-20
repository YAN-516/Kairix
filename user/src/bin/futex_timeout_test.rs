#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::arch::asm;

const SYS_FUTEX: usize = 98;
const SYS_CLOCK_GETTIME: usize = 113;
const FUTEX_WAIT: usize = 0;
const FUTEX_WAIT_BITSET: usize = 9;
const FUTEX_PRIVATE_FLAG: usize = 128;
const FUTEX_CLOCK_REALTIME: usize = 256;
const FUTEX_BITSET_MATCH_ANY: usize = u32::MAX as usize;
const CLOCK_REALTIME: usize = 0;
const CLOCK_MONOTONIC: usize = 1;
const EAGAIN: isize = -11;
const ETIMEDOUT: isize = -110;
const TEST_TIMEOUT_NS: u64 = 20_000_000;
const MAX_EXPECTED_ELAPSED_NS: u64 = 1_000_000_000;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TimeSpec {
    tv_sec: i64,
    tv_nsec: i64,
}

#[cfg(target_arch = "riscv64")]
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

fn clock_gettime(clock: usize) -> Option<TimeSpec> {
    let mut value = TimeSpec::default();
    let ret = unsafe {
        raw_syscall(SYS_CLOCK_GETTIME, [
            clock,
            &mut value as *mut TimeSpec as usize,
            0,
            0,
            0,
            0,
        ])
    };
    (ret == 0).then_some(value)
}

fn to_ns(value: TimeSpec) -> u64 {
    (value.tv_sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(value.tv_nsec as u64)
}

fn from_ns(value: u64) -> TimeSpec {
    TimeSpec {
        tv_sec: (value / 1_000_000_000) as i64,
        tv_nsec: (value % 1_000_000_000) as i64,
    }
}

fn futex_wait(word: &u32, op: usize, timeout: &TimeSpec, bitset: usize) -> isize {
    unsafe {
        raw_syscall(SYS_FUTEX, [
            word as *const u32 as usize,
            op,
            *word as usize,
            timeout as *const TimeSpec as usize,
            0,
            bitset,
        ])
    }
}

fn elapsed_monotonic(start: TimeSpec) -> Option<u64> {
    clock_gettime(CLOCK_MONOTONIC).map(|end| to_ns(end).saturating_sub(to_ns(start)))
}

fn test_relative(word: &u32) -> bool {
    let Some(start) = clock_gettime(CLOCK_MONOTONIC) else {
        return false;
    };
    let timeout = from_ns(TEST_TIMEOUT_NS);
    let ret = futex_wait(
        word,
        FUTEX_WAIT | FUTEX_PRIVATE_FLAG,
        &timeout,
        FUTEX_BITSET_MATCH_ANY,
    );
    let elapsed = elapsed_monotonic(start).unwrap_or(u64::MAX);
    println!(
        "[futex_timeout_test] relative ret={} elapsed_ns={}",
        ret, elapsed
    );
    ret == ETIMEDOUT && elapsed < MAX_EXPECTED_ELAPSED_NS
}

fn test_absolute(word: &u32, clock: usize, realtime_flag: usize, label: &str) -> bool {
    let Some(clock_now) = clock_gettime(clock) else {
        return false;
    };
    let Some(monotonic_start) = clock_gettime(CLOCK_MONOTONIC) else {
        return false;
    };
    let deadline = from_ns(to_ns(clock_now).saturating_add(TEST_TIMEOUT_NS));
    let ret = futex_wait(
        word,
        FUTEX_WAIT_BITSET | FUTEX_PRIVATE_FLAG | realtime_flag,
        &deadline,
        FUTEX_BITSET_MATCH_ANY,
    );
    let elapsed = elapsed_monotonic(monotonic_start).unwrap_or(u64::MAX);
    println!(
        "[futex_timeout_test] {} ret={} elapsed_ns={}",
        label, ret, elapsed
    );
    ret == ETIMEDOUT && elapsed < MAX_EXPECTED_ELAPSED_NS
}

fn test_mismatch(word: &u32) -> bool {
    let timeout = TimeSpec::default();
    let ret = unsafe {
        raw_syscall(SYS_FUTEX, [
            word as *const u32 as usize,
            FUTEX_WAIT | FUTEX_PRIVATE_FLAG,
            word.wrapping_add(1) as usize,
            &timeout as *const TimeSpec as usize,
            0,
            0,
        ])
    };
    println!("[futex_timeout_test] mismatch ret={}", ret);
    ret == EAGAIN
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[futex_timeout_test] start");
    let word = 0u32;
    let passed = test_mismatch(&word)
        && test_relative(&word)
        && test_absolute(&word, CLOCK_MONOTONIC, 0, "absolute_monotonic")
        && test_absolute(
            &word,
            CLOCK_REALTIME,
            FUTEX_CLOCK_REALTIME,
            "absolute_realtime",
        );

    if passed {
        println!("[futex_timeout_test] PASS");
        0
    } else {
        println!("[futex_timeout_test] FAIL");
        1
    }
}
