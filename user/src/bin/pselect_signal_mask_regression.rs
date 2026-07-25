#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::arch::asm;
use core::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};
use user_lib::{
    SIG_BLOCK, SIGUSR1, SigAction, SignalSet, exit, getpid, gettid, sigaction, sigprocmask, tgkill,
    thread_create, waittid, yield_,
};

#[repr(C)]
struct Timespec {
    sec: i64,
    nsec: i64,
}

#[repr(C)]
struct PselectSigmaskArg {
    mask: *const SignalSet,
    size: usize,
}

static TGID: AtomicIsize = AtomicIsize::new(0);
static MAIN_TID: AtomicIsize = AtomicIsize::new(0);
static HANDLER_COUNT: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn usr1_handler(_: i32) {
    HANDLER_COUNT.fetch_add(1, Ordering::AcqRel);
}

extern "C" fn sender(_: usize) -> ! {
    // Give the main thread enough scheduling points to install pselect's
    // temporary mask and publish its waiter before delivering the signal.
    for _ in 0..64 {
        yield_();
    }
    let result = tgkill(
        TGID.load(Ordering::Acquire),
        MAIN_TID.load(Ordering::Acquire),
        SIGUSR1,
    );
    exit(if result == 0 { 0 } else { 2 })
}

#[cfg(target_arch = "riscv64")]
fn pselect_wait(timeout: *const Timespec, sigarg: *const PselectSigmaskArg) -> isize {
    let result: isize;
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") 0usize => result,
            in("a1") 0usize,
            in("a2") 0usize,
            in("a3") 0usize,
            in("a4") timeout,
            in("a5") sigarg,
            in("a7") 72usize,
        );
    }
    result
}

#[cfg(target_arch = "loongarch64")]
fn pselect_wait(timeout: *const Timespec, sigarg: *const PselectSigmaskArg) -> isize {
    let result: isize;
    unsafe {
        asm!(
            "syscall 0",
            inlateout("$a0") 0usize => result,
            in("$a1") 0usize,
            in("$a2") 0usize,
            in("$a3") 0usize,
            in("$a4") timeout,
            in("$a5") sigarg,
            in("$a7") 72usize,
        );
    }
    result
}

fn join(tid: isize) -> isize {
    loop {
        let result = waittid(tid as usize);
        if result != -11 {
            return result;
        }
        yield_();
    }
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[pselect_signal_mask_regression] start");
    TGID.store(getpid(), Ordering::Release);
    MAIN_TID.store(gettid(), Ordering::Release);

    if sigaction(SIGUSR1, Some(&SigAction::custom(usr1_handler)), None) != 0 {
        println!("[pselect_signal_mask_regression] FAIL sigaction");
        return 1;
    }
    let mut blocked = SignalSet::empty();
    blocked.add(SIGUSR1);
    if sigprocmask(SIG_BLOCK, Some(&blocked), None) != 0 {
        println!("[pselect_signal_mask_regression] FAIL block");
        return 2;
    }

    let sender_tid = thread_create(sender, 0);
    if sender_tid < 0 {
        println!(
            "[pselect_signal_mask_regression] FAIL thread={}",
            sender_tid
        );
        return 3;
    }

    let temporary_mask = SignalSet::empty();
    let sigarg = PselectSigmaskArg {
        mask: &temporary_mask,
        size: core::mem::size_of::<SignalSet>(),
    };
    let timeout = Timespec { sec: 2, nsec: 0 };
    let selected = pselect_wait(&timeout, &sigarg);
    let joined = join(sender_tid);

    let mut restored_mask = SignalSet::empty();
    let query = sigprocmask(SIG_BLOCK, None, Some(&mut restored_mask));
    let mask_restored = restored_mask.bits() & (1u64 << (SIGUSR1 - 1)) != 0;
    let handled = HANDLER_COUNT.load(Ordering::Acquire) == 1;

    if selected == -4 && joined == 0 && query == 0 && mask_restored && handled {
        println!("[pselect_signal_mask_regression] PASS");
        0
    } else {
        println!(
            "[pselect_signal_mask_regression] FAIL select={} joined={} query={} mask={:#x} handled={}",
            selected,
            joined,
            query,
            restored_mask.bits(),
            HANDLER_COUNT.load(Ordering::Acquire),
        );
        1
    }
}
