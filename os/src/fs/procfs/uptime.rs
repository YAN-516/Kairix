use alloc::format;
use alloc::string::String;

/// Generate Linux-compatible `/proc/uptime` contents from the real monotonic clock.
pub fn content() -> String {
    const NS_PER_CENTISECOND: usize = 10_000_000;

    let uptime_centiseconds =
        (polyhal::timer::current_time().as_nanos() as usize) / NS_PER_CENTISECOND;
    let idle_centiseconds = crate::task::processor::total_idle_time_ns() / NS_PER_CENTISECOND;
    format!(
        "{}.{:02} {}.{:02}\n",
        uptime_centiseconds / 100,
        uptime_centiseconds % 100,
        idle_centiseconds / 100,
        idle_centiseconds % 100
    )
}
