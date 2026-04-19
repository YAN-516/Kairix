use crate::config::PAGE_SIZE;
use crate::fs::vfs::OpenFlags;
use crate::mm::{PageTable, PhysAddr, VirtAddr, VirtPageNum};
use crate::mm::{VMSpace, translated_ref, translated_refmut, translated_str};
use crate::syscall::process::sys_yield;
use crate::task::Tms;
use crate::task::{
    block_current_and_run_next, current_process, current_task, current_user_token,
    exit_current_and_run_next, pid2process, suspend_current_and_run_next, num_processes,
};
use crate::timer::*;
use crate::trap::_set_sum_bit;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use log::{error, warn};
#[repr(C)]
#[derive(Debug)]
#[derive(Clone, Copy)]
pub struct TimeVal {
    pub sec: i64,
    pub usec: i64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TimeSpec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

#[allow(unused)]
#[repr(C)]
pub struct NanoTimeVal {
    pub sec: i64,
    pub nsec: i64,
}

pub fn sys_times(_ts: *mut Tms) -> isize {
    _set_sum_bit();
    let time = current_process().inner_exclusive_access().time;
    unsafe {
        *(_ts) = time;
    }
    0
}

pub fn sys_get_time(_ts: *mut TimeVal, _tz: usize) -> isize {
    const EFAULT: isize = -14;
    if _ts.is_null() {
        return EFAULT;
    }
    _set_sum_bit();
    let _us = get_time_us();
    let token = current_user_token();
    *translated_refmut(token, _ts) = TimeVal {
        sec: (_us / 1_000_000) as i64,
        usec: (_us % 1_000_000) as i64,
    };
    0
}

use core::i32;
pub fn sys_sleep(_req: *mut TimeVal, _rem: *mut TimeVal) -> isize {
    const EFAULT: isize = -14;
    if _req.is_null() {
        return EFAULT;
    }
    _set_sum_bit();
    let token = current_user_token();
    let req = *translated_ref(token, _req);
    let time_start = get_time_us();
    let mut sleep_time;
    sleep_time = req.sec as i128 * 1_000_000 + req.usec as i128;
    if sleep_time < 0 {
        return -22;
    }

    loop {
        let time_now = get_time_us();
        let time_has_sleep = (time_now - time_start) as i128;
        sleep_time -= time_has_sleep;
        //println!("{} {}", sleep_time, time_has_sleep);
        if sleep_time <= 0 || sleep_time > i32::MAX as i128 {
            sleep_time = 0;
        }
        if !_rem.is_null() {
            *translated_refmut(token, _rem) = TimeVal {
                sec: (sleep_time / 1_000_000) as i64,
                usec: (sleep_time % 1_000_000) as i64,
            };
        }
        if sleep_time == 0 {
            return 0;
        } else {
            //println!("{}", sleep_time);
            sys_yield();
        }
    }
}

pub fn sys_clock_gettime(_clock: usize, ts: *mut NanoTimeVal) -> isize {
    const EFAULT: isize = -14;
    if ts.is_null() {
        return EFAULT;
    }
    _set_sum_bit();
    // println!("{:?}", _ts);
    let us = get_time_us();
    let token = current_user_token();
    *translated_refmut(token, ts) = NanoTimeVal {
        sec: (us / 1_000_000) as i64,
        nsec: ((us % 1_000_000) * 1_000) as i64,
    };
    // println!("end get time");
    0
}

pub fn sys_clock_nanosleep(clock_id: usize, flags: usize, req: *const TimeSpec, rem: *mut TimeSpec) -> isize {
    const EINVAL: isize = -22;
    const EFAULT: isize = -14;
    const CLOCK_REALTIME: usize = 0;
    const CLOCK_MONOTONIC: usize = 1;
    const TIMER_ABSTIME: usize = 1;

    if req.is_null() {
        return EFAULT;
    }
    if clock_id != CLOCK_REALTIME && clock_id != CLOCK_MONOTONIC {
        return EINVAL;
    }

    let token = current_user_token();
    let req_ts = *translated_ref(token, req);
    if req_ts.tv_sec < 0 || req_ts.tv_nsec < 0 || req_ts.tv_nsec >= 1_000_000_000 {
        return EINVAL;
    }

    let now_us = get_time_us() as i128;
    let req_us = req_ts.tv_sec as i128 * 1_000_000 + req_ts.tv_nsec as i128 / 1_000;
    let deadline_us = if (flags & TIMER_ABSTIME) != 0 {
        req_us
    } else {
        now_us + req_us
    };

    while (get_time_us() as i128) < deadline_us {
        sys_yield();
    }

    if !rem.is_null() {
        *translated_refmut(token, rem) = TimeSpec { tv_sec: 0, tv_nsec: 0 };
    }
    0
}
