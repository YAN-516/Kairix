#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::sync::atomic::{AtomicUsize, Ordering};
use user_lib::{
    SIG_BLOCK, SIG_UNBLOCK, SIGUSR1, SigAction, SigHandler, SignalSet, getpid, kill, sigaction,
    sigprocmask, yield_,
};

static HANDLER_CALLED: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn usr1_handler(sig: i32) {
    HANDLER_CALLED.fetch_add(1, Ordering::SeqCst);
    println!("[signal_test] custom handler called, sig={}", sig);
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    let pid = getpid();
    println!("[signal_test] pid={}", pid);

    let custom_act = SigAction::custom(usr1_handler);
    let mut observed_act = SigAction::default();
    let ret_set_custom = sigaction(SIGUSR1, Some(&custom_act), Some(&mut observed_act));
    println!(
        "[signal_test] sigaction(SIGUSR1, CUSTOM) => {}",
        ret_set_custom
    );

    let mut queried_act = SigAction::default();
    let ret_query = sigaction(SIGUSR1, None, Some(&mut queried_act));
    println!("[signal_test] sigaction(SIGUSR1, query) => {}", ret_query);

    let custom_roundtrip_ok = match queried_act.sa_handler {
        SigHandler::Custom(f) => (f as usize) == (usr1_handler as usize),
        _ => false,
    };
    println!(
        "[signal_test] custom handler round-trip => {}",
        custom_roundtrip_ok
    );

    let mut set = SignalSet::empty();
    set.add(SIGUSR1);

    let ret_block = sigprocmask(SIG_BLOCK, Some(&set), None);
    println!("[signal_test] sigprocmask(BLOCK, SIGUSR1) => {}", ret_block);

    let ret_kill_blocked = kill(pid, SIGUSR1 as usize);
    println!(
        "[signal_test] kill(self, SIGUSR1) when blocked => {}",
        ret_kill_blocked
    );

    let ret_unblock = sigprocmask(SIG_UNBLOCK, Some(&set), None);
    println!(
        "[signal_test] sigprocmask(UNBLOCK, SIGUSR1) => {}",
        ret_unblock
    );

    for _ in 0..8 {
        yield_();
    }
    let called = HANDLER_CALLED.load(Ordering::SeqCst);
    println!("[signal_test] handler_called_count => {}", called);
    println!("[signal_test] custom handler observed => {}", called > 0);

    let mut old_act = SigAction::default();
    let ignore_act = SigAction::ignore();
    let ret_sigaction = sigaction(SIGUSR1, Some(&ignore_act), Some(&mut old_act));
    println!("[signal_test] sigaction(SIGUSR1, IGN) => {}", ret_sigaction);

    let ret_kill0 = kill(pid, 0);
    println!("[signal_test] kill(self, 0) => {}", ret_kill0);

    if ret_sigaction >= 0
        && ret_set_custom >= 0
        && ret_query >= 0
        && custom_roundtrip_ok
        && ret_block >= 0
        && ret_kill_blocked >= 0
        && ret_unblock >= 0
        && ret_kill0 >= 0
    {
        println!("[signal_test] PASS");
        0
    } else {
        println!("[signal_test] FAIL");
        1
    }
}
