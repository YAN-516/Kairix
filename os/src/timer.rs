//! Timer-related functionality using polyhal

use core::time::Duration;
use polyhal::consts::VIRT_ADDR_START;
use spin::Once;

#[allow(unused)]
pub(crate) const TICKS_PER_SEC: usize = 100;
const MICRO_PER_SEC: usize = 1_000_000;

#[derive(Clone, Copy)]
struct RealtimeAnchor {
    epoch_ns: u128,
    monotonic_ns: u128,
}

static REALTIME_ANCHOR: Once<RealtimeAnchor> = Once::new();

#[cfg(all(target_arch = "riscv64", board = "visionfive2"))]
const VF2_REALTIME_FLOOR_NS: u128 = 1_786_099_500_000_000_000;

/// 2K1000 实板RTC无效或落后时采用的证书校验时间下限。
#[cfg(all(target_arch = "loongarch64", board = "2k1000"))]
const LS2K_REALTIME_FLOOR_NS: u128 = 1_786_147_200_000_000_000;

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

#[cfg(any(
    target_arch = "loongarch64",
    all(target_arch = "riscv64", board = "visionfive2")
))]
fn is_leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

#[cfg(any(
    target_arch = "loongarch64",
    all(target_arch = "riscv64", board = "visionfive2")
))]
fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if is_leap_year(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

#[cfg(any(
    target_arch = "loongarch64",
    all(target_arch = "riscv64", board = "visionfive2")
))]
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
    const AON_CRG_BASE: usize = 0x1700_0000;
    const RTC_BASE: usize = 0x1704_0000;

    const RTC_APB_CLK: usize = 0x28;
    const RTC_CAL_CLK: usize = 0x34;
    const AON_RESET_ASSERT: usize = 0x38;
    const CLK_ENABLE: u32 = 1 << 31;
    const RTC_RESET_MASK: u32 = (1 << 5) | (1 << 6) | (1 << 7);

    const RTC_CFG: usize = 0x00;
    const RTC_TIME: usize = 0x3c;
    const RTC_DATE: usize = 0x40;
    const RTC_ENABLE: u32 = 1 << 0;
    const RTC_24_HOUR_MODE: u32 = 1 << 3;

    #[inline]
    unsafe fn read32(address: usize) -> u32 {
        unsafe { (address as *const u32).read_volatile() }
    }

    #[inline]
    unsafe fn write32(address: usize, value: u32) {
        unsafe { (address as *mut u32).write_volatile(value) }
    }

    #[inline]
    fn bcd_to_bin(value: u32) -> Option<i64> {
        let low = value & 0xf;
        let high = (value >> 4) & 0xf;
        (low <= 9 && high <= 9).then_some((high * 10 + low) as i64)
    }

    let aon = AON_CRG_BASE + VIRT_ADDR_START;
    let rtc = RTC_BASE + VIRT_ADDR_START;

    unsafe {
        write32(aon + RTC_APB_CLK, read32(aon + RTC_APB_CLK) | CLK_ENABLE);
        write32(aon + RTC_CAL_CLK, read32(aon + RTC_CAL_CLK) | CLK_ENABLE);
        write32(
            aon + AON_RESET_ASSERT,
            read32(aon + AON_RESET_ASSERT) & !RTC_RESET_MASK,
        );
        write32(
            rtc + RTC_CFG,
            read32(rtc + RTC_CFG) | RTC_ENABLE | RTC_24_HOUR_MODE,
        );
    }

    // Read until date and time are from the same one-second interval. This
    // avoids combining yesterday's date with midnight after a rollover.
    let (time, date) = (0..4).find_map(|_| {
        let date_before = unsafe { read32(rtc + RTC_DATE) };
        let time_before = unsafe { read32(rtc + RTC_TIME) };
        let time_after = unsafe { read32(rtc + RTC_TIME) };
        let date_after = unsafe { read32(rtc + RTC_DATE) };
        (date_before == date_after && time_before == time_after).then_some((time_after, date_after))
    })?;

    let second = bcd_to_bin(time & 0x7f)?;
    let minute = bcd_to_bin((time >> 7) & 0x7f)?;
    let hour = bcd_to_bin((time >> 14) & 0x7f)?;
    let day = bcd_to_bin(date & 0x3f)?;
    let month = bcd_to_bin((date >> 6) & 0x1f)?;
    let year = bcd_to_bin((date >> 11) & 0xff)? + 2000;

    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    calendar_to_epoch_ns(year, month, day, hour, minute, second)
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
    // Keep the year and packed calendar fields from one stable RTC interval.
    // This avoids combining the new year/day with the previous second during
    // a register rollover.
    let (value, year_raw) = (0..4).find_map(|_| {
        let year_before = unsafe { ((base + SYS_TOYREAD1) as *const u32).read_volatile() };
        let value_before = unsafe { ((base + SYS_TOYREAD0) as *const u32).read_volatile() };
        let value_after = unsafe { ((base + SYS_TOYREAD0) as *const u32).read_volatile() };
        let year_after = unsafe { ((base + SYS_TOYREAD1) as *const u32).read_volatile() };
        (year_before == year_after && value_before == value_after)
            .then_some((value_after, year_after))
    })?;
    let year = year_raw as i64 + 1900;
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
        // The tested VisionFive 2 firmware restores a stale RTC value during
        // a warm reset. Keep later calibrated values, but never expose a date
        // older than the build's known-good certificate-validation baseline.
        #[cfg(all(target_arch = "riscv64", board = "visionfive2"))]
        let rtc_ns = Some(
            rtc_ns
                .unwrap_or(VF2_REALTIME_FLOOR_NS)
                .max(VF2_REALTIME_FLOOR_NS),
        );
        // The LS7A RTC can be left uninitialized or stale after firmware/board
        // resets. Preserve valid later values, but keep TLS certificate checks
        // from observing a date older than this build's known-good baseline.
        #[cfg(all(target_arch = "loongarch64", board = "2k1000"))]
        let rtc_ns = Some(
            rtc_ns
                .unwrap_or(LS2K_REALTIME_FLOOR_NS)
                .max(LS2K_REALTIME_FLOOR_NS),
        );
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

/// Current Unix epoch time split into seconds and nanoseconds for inode metadata.
pub fn realtime_timespec() -> (i64, i64) {
    let ns = realtime_ns();
    (
        (ns / 1_000_000_000).min(i64::MAX as u128) as i64,
        (ns % 1_000_000_000) as i64,
    )
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
