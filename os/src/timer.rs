//! RISC-V timer-related functionality

use crate::config::_CLOCK_FREQ;
#[cfg(target_arch = "riscv64")]
use crate::sbi::set_timer;

#[cfg(target_arch = "riscv64")]
use riscv::register::time;

const TICKS_PER_SEC: usize = 100;
const MICRO_PER_SEC: usize = 1_000_000;
///get current time
pub fn get_time() -> usize {
    time::read()
}
/// get current time in microseconds
pub fn get_time_us() -> usize {
    time::read() / (_CLOCK_FREQ / MICRO_PER_SEC)
}
/// set the next timer interrupt
pub fn set_next_trigger() {
    set_timer(get_time() + _CLOCK_FREQ / TICKS_PER_SEC);
}
