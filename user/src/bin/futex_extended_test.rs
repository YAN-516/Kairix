#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use user_lib::{exit, futex, futex_waitv, thread_create, waittid, yield_};

const FUTEX_WAKE: i32 = 1;
const FUTEX_WAKE_OP: i32 = 5;
const FUTEX_LOCK_PI: i32 = 6;
const FUTEX_UNLOCK_PI: i32 = 7;
const FUTEX_PRIVATE: i32 = 128;
const FUTEX_32: u32 = 2;

#[repr(C)]
struct FutexWaitv {
    val: u64,
    uaddr: u64,
    flags: u32,
    reserved: u32,
}

static WAITV0: AtomicU32 = AtomicU32::new(0);
static WAITV1: AtomicU32 = AtomicU32::new(0);
static WAITV_READY: AtomicBool = AtomicBool::new(false);
static WAITV_RESULT: AtomicIsize = AtomicIsize::new(-999);

static PI_WORD: AtomicU32 = AtomicU32::new(0);
static PI_READY: AtomicBool = AtomicBool::new(false);
static PI_ACQUIRED: AtomicBool = AtomicBool::new(false);
static PI_RESULT: AtomicIsize = AtomicIsize::new(-999);

fn ptr(word: &AtomicU32) -> *mut u32 {
    word.as_ptr()
}

extern "C" fn waitv_worker(_: usize) -> ! {
    let entries = [
        FutexWaitv {
            val: 0,
            uaddr: ptr(&WAITV0) as usize as u64,
            flags: FUTEX_32 | FUTEX_PRIVATE as u32,
            reserved: 0,
        },
        FutexWaitv {
            val: 0,
            uaddr: ptr(&WAITV1) as usize as u64,
            flags: FUTEX_32 | FUTEX_PRIVATE as u32,
            reserved: 0,
        },
    ];
    WAITV_READY.store(true, Ordering::Release);
    let ret = futex_waitv(entries.as_ptr() as *const u8, entries.len(), 0, 0, 1);
    WAITV_RESULT.store(ret, Ordering::Release);
    exit(if ret == 1 { 0 } else { 2 })
}

extern "C" fn pi_worker(_: usize) -> ! {
    PI_READY.store(true, Ordering::Release);
    let lock_ret = futex(
        ptr(&PI_WORD),
        FUTEX_LOCK_PI | FUTEX_PRIVATE,
        0,
        0,
        core::ptr::null_mut(),
        0,
    );
    PI_RESULT.store(lock_ret, Ordering::Release);
    if lock_ret == 0 {
        PI_ACQUIRED.store(true, Ordering::Release);
        let _ = futex(
            ptr(&PI_WORD),
            FUTEX_UNLOCK_PI | FUTEX_PRIVATE,
            0,
            0,
            core::ptr::null_mut(),
            0,
        );
        exit(0)
    }
    exit(3)
}

fn join(tid: isize) -> isize {
    loop {
        let ret = waittid(tid as usize);
        if ret != -11 {
            return ret;
        }
        yield_();
    }
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[futex_extended_test] start");

    let wake_word = AtomicU32::new(0);
    let op_word = AtomicU32::new(0);
    // FUTEX_OP_ADD(1), compare old value == 0.
    let encoded = (1u32 << 28) | (1u32 << 12);
    let wake_op_ret = futex(
        ptr(&wake_word),
        FUTEX_WAKE_OP | FUTEX_PRIVATE,
        0,
        0,
        ptr(&op_word),
        encoded,
    );
    let wake_op_ok = wake_op_ret == 0 && op_word.load(Ordering::Acquire) == 1;

    let waitv_tid = thread_create(waitv_worker, 0);
    if waitv_tid < 0 {
        return 1;
    }
    while !WAITV_READY.load(Ordering::Acquire) {
        yield_();
    }
    for _ in 0..64 {
        yield_();
    }
    WAITV1.store(1, Ordering::Release);
    let waitv_wake = futex(
        ptr(&WAITV1),
        FUTEX_WAKE | FUTEX_PRIVATE,
        1,
        0,
        core::ptr::null_mut(),
        0,
    );
    let waitv_join = join(waitv_tid);
    let waitv_ok = waitv_wake == 1 && WAITV_RESULT.load(Ordering::Acquire) == 1 && waitv_join == 0;

    let main_pi_lock = futex(
        ptr(&PI_WORD),
        FUTEX_LOCK_PI | FUTEX_PRIVATE,
        0,
        0,
        core::ptr::null_mut(),
        0,
    );
    let pi_tid = thread_create(pi_worker, 0);
    if main_pi_lock != 0 || pi_tid < 0 {
        return 1;
    }
    while !PI_READY.load(Ordering::Acquire) {
        yield_();
    }
    for _ in 0..64 {
        yield_();
    }
    let main_pi_unlock = futex(
        ptr(&PI_WORD),
        FUTEX_UNLOCK_PI | FUTEX_PRIVATE,
        0,
        0,
        core::ptr::null_mut(),
        0,
    );
    let pi_join = join(pi_tid);
    let pi_ok = main_pi_unlock == 0
        && PI_RESULT.load(Ordering::Acquire) == 0
        && PI_ACQUIRED.load(Ordering::Acquire)
        && pi_join == 0;

    println!(
        "[futex_extended_test] wake_op={} waitv={} pi={}",
        wake_op_ok, waitv_ok, pi_ok
    );
    if wake_op_ok && waitv_ok && pi_ok {
        println!("[futex_extended_test] PASS");
        0
    } else {
        println!("[futex_extended_test] FAIL");
        1
    }
}
