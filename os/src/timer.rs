//! Timer-related functionality using polyhal

use core::time::Duration;

#[allow(unused)]
const TICKS_PER_SEC: usize = 100;
const MICRO_PER_SEC: usize = 1_000_000;

/// get current time in ticks
pub fn get_time() -> usize {
    polyhal::timer::get_ticks() as usize
}

/// get current time in microseconds
pub fn get_time_us() -> usize {
    let ticks = polyhal::timer::get_ticks();
    let freq = polyhal::timer::get_freq();
    (ticks * MICRO_PER_SEC as u64 / freq) as usize
}

/// set the next timer interrupt
pub fn set_next_trigger() {
    // 启用 S 态时钟中断
    // unsafe {
    //     riscv::register::sie::set_stimer();
    // }
    // let interval = Duration::from_millis((1000 / TICKS_PER_SEC) as u64);
    // polyhal::timer::set_next_timer(interval);
    polyhal_trap::trap::init_timer();
}
