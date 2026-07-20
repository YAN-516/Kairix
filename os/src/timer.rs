//! Timer-related functionality using polyhal

use core::time::Duration;
use polyhal::consts::VIRT_ADDR_START;
use spin::Once;

#[allow(unused)]
const TICKS_PER_SEC: usize = 100;
const MICRO_PER_SEC: usize = 1_000_000;

#[derive(Clone, Copy)]
struct RealtimeAnchor {
    epoch_ns: u128,
    monotonic_ns: u128,
}

static REALTIME_ANCHOR: Once<RealtimeAnchor> = Once::new();

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

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let adjusted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * adjusted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(target_arch = "loongarch64")]
fn is_leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

#[cfg(target_arch = "loongarch64")]
fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if is_leap_year(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

#[cfg(target_arch = "loongarch64")]
fn calendar_to_epoch_ns(
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
) -> Option<u128> {
    if year < 1970
        || !(1..=12).contains(&month)
        || day < 1
        || day > days_in_month(year, month)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=60).contains(&second)
    {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let seconds = days
        .checked_mul(86_400)?
        .checked_add(hour * 3_600 + minute * 60 + second)?;
    Some(seconds as u128 * 1_000_000_000)
}

#[cfg(all(target_arch = "riscv64", board = "visionfive2"))]
fn read_rtc_ns() -> Option<u128> {
    None
}

#[cfg(all(target_arch = "riscv64", not(board = "visionfive2")))]
fn read_rtc_ns() -> Option<u128> {
    const GOLDFISH_RTC_BASE: usize = 0x0010_1000;
    let base = GOLDFISH_RTC_BASE + VIRT_ADDR_START;
    let low = unsafe { (base as *const u32).read_volatile() } as u64;
    let high = unsafe { ((base + 4) as *const u32).read_volatile() } as u64;
    let ns = (high << 32) | low;
    (ns >= 1_577_836_800_000_000_000).then_some(ns as u128)
}

#[cfg(target_arch = "loongarch64")]
fn read_rtc_ns() -> Option<u128> {
    const LS7A_RTC_BASE: usize = 0x100d_0100;
    const SYS_TOYREAD0: usize = 0x2c;
    const SYS_TOYREAD1: usize = 0x30;
    const SYS_RTCCTRL: usize = 0x40;
    const RTC_CTRL_ENABLE_TOY: u32 = (1 << 11) | (1 << 8);

    let base = LS7A_RTC_BASE + VIRT_ADDR_START;
    let control = unsafe { ((base + SYS_RTCCTRL) as *const u32).read_volatile() };
    if control & RTC_CTRL_ENABLE_TOY != RTC_CTRL_ENABLE_TOY {
        unsafe {
            ((base + SYS_RTCCTRL) as *mut u32).write_volatile(control | RTC_CTRL_ENABLE_TOY);
        }
    }
    let value = unsafe { ((base + SYS_TOYREAD0) as *const u32).read_volatile() };
    let year = unsafe { ((base + SYS_TOYREAD1) as *const u32).read_volatile() } as i64 + 1900;
    let month = ((value >> 26) & 0x3f) as i64;
    let day = ((value >> 21) & 0x1f) as i64;
    let hour = ((value >> 16) & 0x1f) as i64;
    let minute = ((value >> 10) & 0x3f) as i64;
    let second = ((value >> 4) & 0x3f) as i64;
    calendar_to_epoch_ns(year, month, day, hour, minute, second)
}

/// Current wall-clock time as Unix epoch nanoseconds.
pub fn realtime_ns() -> u128 {
    let anchor = REALTIME_ANCHOR.call_once(|| {
        // The LS7A calendar RTC only advances in coarse wall-clock units. Use
        // it once to establish the Unix epoch, then advance CLOCK_REALTIME
        // with the high-resolution monotonic counter. Bracketing the MMIO read
        // limits the anchoring error to half of the RTC access latency.
        let before_ns = polyhal::timer::current_time().as_nanos();
        let rtc_ns = read_rtc_ns();
        let after_ns = polyhal::timer::current_time().as_nanos();
        let monotonic_ns = before_ns.saturating_add(after_ns.saturating_sub(before_ns) / 2);
        RealtimeAnchor {
            epoch_ns: rtc_ns.unwrap_or(monotonic_ns),
            monotonic_ns,
        }
    });
    let monotonic_ns = polyhal::timer::current_time().as_nanos();
    anchor
        .epoch_ns
        .saturating_add(monotonic_ns.saturating_sub(anchor.monotonic_ns))
}

/// Resolution of the hardware counter backing monotonic and realtime clocks.
pub fn clock_resolution_ns() -> u64 {
    let frequency = polyhal::timer::get_freq();
    if frequency == 0 {
        return 1_000_000_000;
    }
    1_000_000_000u64
        .saturating_add(frequency - 1)
        .checked_div(frequency)
        .unwrap_or(1)
        .max(1)
}

/// Calendar representation of the current UTC wall-clock time.
#[allow(missing_docs)]
pub struct CalendarTime {
    pub year: i32,
    pub month: i32,
    pub day: i32,
    pub hour: i32,
    pub minute: i32,
    pub second: i32,
    pub weekday: i32,
    pub yearday: i32,
}

/// Convert the hardware real-time clock to UTC calendar fields.
pub fn realtime_calendar() -> CalendarTime {
    let total_seconds = (realtime_ns() / 1_000_000_000).min(i64::MAX as u128) as i64;
    let days = total_seconds.div_euclid(86_400);
    let seconds_of_day = total_seconds.rem_euclid(86_400);

    let adjusted_days = days + 719_468;
    let era = adjusted_days.div_euclid(146_097);
    let day_of_era = adjusted_days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = month_index + if month_index < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);

    CalendarTime {
        year: year as i32,
        month: month as i32,
        day: day as i32,
        hour: (seconds_of_day / 3_600) as i32,
        minute: ((seconds_of_day / 60) % 60) as i32,
        second: (seconds_of_day % 60) as i32,
        weekday: (days + 4).rem_euclid(7) as i32,
        yearday: (days - days_from_civil(year, 1, 1)) as i32,
    }
}

/// set the next timer interrupt
pub fn set_next_trigger() {
    polyhal::timer::enable_timer_interrupt();
    let interval = Duration::from_millis((1000 / TICKS_PER_SEC) as u64);
    crate::interrupts::program_next_timer(interval);
}
