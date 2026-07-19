#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::hint::black_box;
use user_lib::get_time;

const SAMPLES: usize = 200_000;

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    let start_ms = get_time();
    if start_ms < 0 {
        println!("[realtime_precision_test] FAIL: initial gettimeofday");
        return 1;
    }

    let mut end_ms = start_ms;
    for sample in 0..SAMPLES {
        black_box(sample);
        if sample & 0xff == 0 {
            end_ms = get_time();
            if end_ms < 0 {
                println!("[realtime_precision_test] FAIL: gettimeofday sample");
                return 2;
            }
            if end_ms != start_ms && end_ms % 1_000 != 0 {
                println!(
                    "[realtime_precision_test] PASS: start_ms={} current_ms={}",
                    start_ms, end_ms
                );
                return 0;
            }
        }
    }

    println!(
        "[realtime_precision_test] FAIL: realtime stayed at whole-second precision start_ms={} end_ms={}",
        start_ms, end_ms
    );
    3
}
