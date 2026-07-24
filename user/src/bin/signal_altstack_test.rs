#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::{
    arch::asm,
    sync::atomic::{AtomicIsize, AtomicUsize, Ordering},
};
use user_lib::{
    SA_ONSTACK, SA_RESETHAND, SIGUSR1, SS_DISABLE, SigAction, SigHandler, StackT, getpid, kill,
    sigaction, sigaltstack, yield_,
};

const ALT_STACK_SIZE: usize = 16 * 1024;

#[repr(align(16))]
struct AlignedStack([u8; ALT_STACK_SIZE]);

static mut ALT_STACK: AlignedStack = AlignedStack([0; ALT_STACK_SIZE]);
static HANDLER_COUNT: AtomicUsize = AtomicUsize::new(0);
static HANDLER_SP: AtomicUsize = AtomicUsize::new(0);
static DISABLE_FROM_HANDLER: AtomicIsize = AtomicIsize::new(0);

unsafe extern "C" fn usr1_handler(_sig: i32) {
    let sp: usize;
    #[cfg(target_arch = "riscv64")]
    unsafe {
        asm!("mv {}, sp", out(reg) sp, options(nomem, nostack, preserves_flags));
    }
    #[cfg(target_arch = "loongarch64")]
    unsafe {
        asm!("move {}, $sp", out(reg) sp, options(nomem, nostack, preserves_flags));
    }
    HANDLER_SP.store(sp, Ordering::SeqCst);
    HANDLER_COUNT.fetch_add(1, Ordering::SeqCst);
    let disabled = StackT::disabled();
    DISABLE_FROM_HANDLER.store(sigaltstack(Some(&disabled), None), Ordering::SeqCst);
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    let alt_base =
        unsafe { (&raw mut ALT_STACK.0) as *mut [u8; ALT_STACK_SIZE] as *mut u8 as usize };
    let configured = StackT {
        ss_sp: alt_base,
        ss_flags: 0,
        ss_size: ALT_STACK_SIZE,
    };
    let mut before = StackT::disabled();
    let install_ret = sigaltstack(Some(&configured), Some(&mut before));

    let mut action = SigAction::custom(usr1_handler);
    action.sa_flags = SA_ONSTACK | SA_RESETHAND;
    let action_ret = sigaction(SIGUSR1, Some(&action), None);
    let kill_ret = kill(getpid(), SIGUSR1 as usize);
    for _ in 0..8 {
        yield_();
    }

    let count = HANDLER_COUNT.load(Ordering::SeqCst);
    let handler_sp = HANDLER_SP.load(Ordering::SeqCst);
    let handler_on_alt = handler_sp >= alt_base && handler_sp < alt_base + ALT_STACK_SIZE;
    let disable_in_handler = DISABLE_FROM_HANDLER.load(Ordering::SeqCst);

    let mut queried_action = SigAction::ignore();
    let query_action_ret = sigaction(SIGUSR1, None, Some(&mut queried_action));
    let reset_to_default = matches!(queried_action.sa_handler, SigHandler::Default);

    let mut queried_stack = StackT::disabled();
    let query_stack_ret = sigaltstack(None, Some(&mut queried_stack));
    let stack_still_enabled = queried_stack.ss_flags == 0
        && queried_stack.ss_sp == alt_base
        && queried_stack.ss_size == ALT_STACK_SIZE;

    let disable_ret = sigaltstack(Some(&StackT::disabled()), None);
    let mut disabled_stack = configured;
    let query_disabled_ret = sigaltstack(None, Some(&mut disabled_stack));
    let stack_disabled = disabled_stack.ss_flags == SS_DISABLE;

    println!(
        "[signal_altstack_test] install={} old_flags={} action={} kill={} count={} sp={:#x} on_alt={}",
        install_ret, before.ss_flags, action_ret, kill_ret, count, handler_sp, handler_on_alt
    );
    println!(
        "[signal_altstack_test] handler_disable={} reset_default={} query_action={} stack_enabled={} query_stack={} disable={} disabled={} query_disabled={}",
        disable_in_handler,
        reset_to_default,
        query_action_ret,
        stack_still_enabled,
        query_stack_ret,
        disable_ret,
        stack_disabled,
        query_disabled_ret,
    );

    if install_ret == 0
        && before.ss_flags == SS_DISABLE
        && action_ret == 0
        && kill_ret == 0
        && count == 1
        && handler_on_alt
        && disable_in_handler == -1
        && query_action_ret == 0
        && reset_to_default
        && query_stack_ret == 0
        && stack_still_enabled
        && disable_ret == 0
        && query_disabled_ret == 0
        && stack_disabled
    {
        println!("[signal_altstack_test] PASS");
        0
    } else {
        println!("[signal_altstack_test] FAIL");
        1
    }
}
