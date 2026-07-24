#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering};
use user_lib::{
    SIG_BLOCK, SIG_UNBLOCK, SIGUSR1, SigAction, SignalSet, exit, getpid, gettid, kill, sigaction,
    sigprocmask, tgkill, thread_create, waittid, yield_,
};

const SIGRT64: i32 = 64;
static MAIN_TID: AtomicIsize = AtomicIsize::new(0);
static TGID: AtomicIsize = AtomicIsize::new(0);
static SEND_DONE: AtomicBool = AtomicBool::new(false);
static USR1_COUNT: AtomicUsize = AtomicUsize::new(0);
static USR1_HANDLER_TID: AtomicIsize = AtomicIsize::new(0);
static RT64_COUNT: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn usr1_handler(_: i32) {
    USR1_HANDLER_TID.store(gettid(), Ordering::Release);
    USR1_COUNT.fetch_add(1, Ordering::AcqRel);
}

unsafe extern "C" fn rt64_handler(_: i32) {
    RT64_COUNT.fetch_add(1, Ordering::AcqRel);
}

extern "C" fn sender(_: usize) -> ! {
    let ret = tgkill(
        TGID.load(Ordering::Acquire),
        MAIN_TID.load(Ordering::Acquire),
        SIGUSR1,
    );
    println!(
        "[signal_thread_queue_test] tgkill blocked target => {}",
        ret
    );
    SEND_DONE.store(true, Ordering::Release);
    exit(if ret == 0 { 0 } else { 2 })
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
    println!("[signal_thread_queue_test] start");
    MAIN_TID.store(gettid(), Ordering::Release);
    TGID.store(getpid(), Ordering::Release);

    let usr1_action = SigAction::custom(usr1_handler);
    let rt64_action = SigAction::custom(rt64_handler);
    if sigaction(SIGUSR1, Some(&usr1_action), None) < 0
        || sigaction(SIGRT64, Some(&rt64_action), None) < 0
    {
        println!("[signal_thread_queue_test] FAIL: sigaction");
        return 1;
    }

    let mut mask = SignalSet::empty();
    mask.add(SIGUSR1);
    mask.add(SIGRT64);
    if sigprocmask(SIG_BLOCK, Some(&mask), None) < 0 {
        return 1;
    }

    let sender_tid = thread_create(sender, 0);
    if sender_tid < 0 {
        return 1;
    }
    while !SEND_DONE.load(Ordering::Acquire) {
        yield_();
    }
    for _ in 0..16 {
        yield_();
    }
    let stayed_pending = USR1_COUNT.load(Ordering::Acquire) == 0;

    let pid = getpid();
    let rt_send1 = kill(pid, SIGRT64 as usize);
    let rt_send2 = kill(pid, SIGRT64 as usize);
    let unblock = sigprocmask(SIG_UNBLOCK, Some(&mask), None);
    for _ in 0..256 {
        if USR1_COUNT.load(Ordering::Acquire) == 1 && RT64_COUNT.load(Ordering::Acquire) == 2 {
            break;
        }
        yield_();
    }

    let joined = join(sender_tid);
    let targeted = USR1_COUNT.load(Ordering::Acquire) == 1
        && USR1_HANDLER_TID.load(Ordering::Acquire) == MAIN_TID.load(Ordering::Acquire);
    let realtime_queued = RT64_COUNT.load(Ordering::Acquire) == 2;
    println!(
        "[signal_thread_queue_test] pending={} targeted={} rt_count={} joined={}",
        stayed_pending,
        targeted,
        RT64_COUNT.load(Ordering::Acquire),
        joined
    );
    if stayed_pending
        && targeted
        && realtime_queued
        && rt_send1 == 0
        && rt_send2 == 0
        && unblock == 0
        && joined == 0
    {
        println!("[signal_thread_queue_test] PASS");
        0
    } else {
        println!("[signal_thread_queue_test] FAIL");
        1
    }
}
