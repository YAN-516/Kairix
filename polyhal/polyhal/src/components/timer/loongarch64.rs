use core::time::Duration;

use crate::timer::TICKS_PER_SEC;
use loongArch64::register::ecfg::{self, LineBasedInterrupt};
use loongArch64::register::{tcfg, ticlr, tval};
/// Returns the current clock time in hardware ticks.
use loongArch64::time::{Time, get_timer_freq};
use spin::Lazy;

use crate::timer::current_time;

// static mut FREQ: usize = 0;
static FREQ: Lazy<u64> = Lazy::new(|| get_timer_freq() as _);

/// Get ticks from system clock
///
/// # Return
///
/// - [u64] clock ticks
#[inline]
pub fn get_ticks() -> u64 {
    Time::read() as _
}

/// Get frequency of the system clock
///
/// # Return
///
/// - [u64] n ticks per second
#[inline]
pub fn get_freq() -> u64 {
    *FREQ
}

/// Set the next timer
///
/// # parameters
///
/// - next [Duration] next time from system boot#[inline]
pub fn set_next_timer(next: Duration) -> (u64, u64, usize) {
    let current = get_ticks();
    let raw_ticks =
        next.as_secs() * get_freq() + next.subsec_nanos() as u64 * get_freq() / 1_000_000_000;
    // TCFG.InitVal is expressed in multiples of four. Round up so a requested
    // interval is never shortened, and keep a non-zero minimum for one-shot
    // operation.
    let ticks = raw_ticks.max(4).saturating_add(3) & !3;

    let config = tcfg::read();
    let already_running = config.raw() & 0b11 == 0b11
        && config.init_val() == ticks as usize
        && tval::read().time_val() != 0;

    if !already_running {
        // The scheduler tick is periodic. Configure it once and let hardware
        // reload TVAL at zero; repeatedly stopping a per-CPU one-shot timer in
        // its interrupt handler creates a window in which that CPU can lose
        // its only source of preemption.
        tcfg::set_en(false);
        tcfg::set_periodic(true);
        tcfg::set_init_val(ticks as _);
        ticlr::clear_timer_interrupt();
        tcfg::set_en(true);
    }

    let armed = tcfg::read().raw() & 0b11 == 0b11 && tval::read().time_val() != 0;
    (
        current,
        current.saturating_add(ticks),
        if armed { 0 } else { 1 },
    )
}

pub fn init() {
    // Leave the timer stopped until a valid non-zero deadline has been loaded.
    // Enabling InitVal=0 can leave a pending timer event racing with the first
    // real programming operation.
    tcfg::set_en(false);
    tcfg::set_periodic(true);
    ticlr::clear_timer_interrupt();

    let inter = LineBasedInterrupt::TIMER
        | LineBasedInterrupt::SWI0
        | LineBasedInterrupt::SWI1
        | LineBasedInterrupt::HWI0;
    ecfg::set_lie(inter);
    let interval = Duration::from_millis((1000 / TICKS_PER_SEC) as u64);
    set_next_timer(interval);
}

pub fn enable_timer_interrupt() {
    let current = ecfg::read().lie();
    ecfg::set_lie(current | ecfg::LineBasedInterrupt::TIMER);
}

/// 关闭定时器中断
pub fn disable_timer_interrupt() {
    let current = ecfg::read().lie();
    ecfg::set_lie(current & !ecfg::LineBasedInterrupt::TIMER);
}
