use core::time::Duration;


// TODO: Get CLOCK_FREQUENCY CLOCK_FREQ
use riscv::register::{sie, time};

const CLOCK_FREQ: u64 = 12500000;

/// Get ticks from system clock
///
/// # Return
///
/// - [u64] clock ticks
#[inline]
pub fn get_ticks() -> u64 {
    time::read64()
}

/// Get frequency of the system clock
///
/// # Return
///
/// - [u64] n ticks per second
#[inline]
pub fn get_freq() -> u64 {
    CLOCK_FREQ
}

/// Set the next timer
///
/// # parameters
///
/// - next [Duration] interval from now#[inline]
pub fn set_next_timer(next: Duration) {
    let current = get_ticks();
    let ticks = next.as_secs() * CLOCK_FREQ
        + next.subsec_nanos() as u64 * CLOCK_FREQ / 1_000_000_000;
    sbi_rt::set_timer(current + ticks);
}

// Initialize the Timer
pub fn init() {
    // unsafe {
    //     sie::set_stimer();
    // }
    // set_next_timer(Duration::ZERO);
    // error!("initialize timer interrupt");
}
