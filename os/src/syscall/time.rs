// use crate::config::PAGE_SIZE;
use polyhal::consts::PAGE_SIZE;

// use crate::fs::open_file;
use crate::error::{SysError, SyscallResult};
use crate::fs::vfs::OpenFlags;
use crate::mm::{PageTable, PhysAddr, VirtAddr, VirtPageNum};
use crate::mm::{VMSpace, translated_ref, translated_refmut, translated_str};
use crate::syscall::process::sys_yield;
use crate::task::Tms;
use crate::task::{
    block_current_and_run_next, current_process, current_task, current_user_token,
    exit_current_and_run_next, num_processes, pid2process, suspend_current_and_run_next,
};
// use crate::timer::*;
use crate::trap::_set_sum_bit;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use log::{error, warn};
use polyhal::timer::current_time;
#[repr(C)]
#[derive(Debug, Clone, Copy)]
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

pub fn sys_times(_ts: *mut Tms) -> SyscallResult {
    _set_sum_bit();
    let time = current_process().inner_exclusive_access().time;
    unsafe {
        *(_ts) = time;
    }
    Ok(0)
}

const RUSAGE_SELF: i32 = 0;
const RUSAGE_CHILDREN: i32 = -1;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Rusage {
    pub ru_utime: TimeVal,
    pub ru_stime: TimeVal,
    pub ru_maxrss: isize,
    pub ru_ixrss: isize,
    pub ru_idrss: isize,
    pub ru_isrss: isize,
    pub ru_minflt: isize,
    pub ru_majflt: isize,
    pub ru_nswap: isize,
    pub ru_inblock: isize,
    pub ru_oublock: isize,
    pub ru_msgsnd: isize,
    pub ru_msgrcv: isize,
    pub ru_nsignals: isize,
    pub ru_nvcsw: isize,
    pub ru_nivcsw: isize,
}

pub fn sys_getrusage(who: i32, usage: *mut Rusage) -> SyscallResult {
    if usage.is_null() {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();

    let mut rusage = Rusage {
        ru_utime: TimeVal { sec: 0, usec: 0 },
        ru_stime: TimeVal { sec: 0, usec: 0 },
        ru_maxrss: 0,
        ru_ixrss: 0,
        ru_idrss: 0,
        ru_isrss: 0,
        ru_minflt: 0,
        ru_majflt: 0,
        ru_nswap: 0,
        ru_inblock: 0,
        ru_oublock: 0,
        ru_msgsnd: 0,
        ru_msgrcv: 0,
        ru_nsignals: 0,
        ru_nvcsw: 0,
        ru_nivcsw: 0,
    };

    match who {
        RUSAGE_SELF => {
            let process = current_process();
            let inner = process.inner_exclusive_access();
            let elapsed_us = current_time().as_micros().saturating_sub(inner.kstart as u128);
            rusage.ru_utime.sec = (elapsed_us / 1_000_000) as i64;
            rusage.ru_utime.usec = (elapsed_us % 1_000_000) as i64;
        }
        RUSAGE_CHILDREN => {
            // 当前未维护子进程累计时间，返回全 0
        }
        _ => return Err(SysError::EINVAL),
    }

    *translated_refmut(token, usage) = rusage;
    Ok(0)
}

// pub fn sys_get_time(_ts: *mut TimeVal, _tz: usize) -> isize {
//     const EFAULT: isize = -14;
//     if _ts.is_null() {
//         return EFAULT;
//     }
//     _set_sum_bit();
//     let _us = current_time().as_micros() as usize;
//     let token = current_user_token();
//     *translated_refmut(token, _ts) = TimeVal {
//         sec: (_us / 1_000_000) as i64,
//         usec: (_us % 1_000_000) as i64,
//     };
//     0
// }

use core::i32;
pub fn sys_sleep(_req: *mut TimeVal, _rem: *mut TimeVal) -> SyscallResult {
    let token = current_user_token();
    if _req.is_null() {
        return Err(SysError::EFAULT);
    }
    _set_sum_bit();
    let time_start = current_time().as_micros() as usize;
    let mut sleep_time;
    sleep_time = unsafe { (*_req).sec as i128 * 1_000_000 + (*_req).usec as i128 };
    if sleep_time < 0 {
        return Err(SysError::EINVAL);
    }

    loop {
        let time_now = current_time().as_micros() as usize;
        let time_has_sleep = time_now - time_start;
        sleep_time -= time_has_sleep as i128;
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
            return Ok(0);
        } else {
            //println!("{}", sleep_time);
            sys_yield()?;
        }
    }
}

pub fn sys_clock_gettime(_clock: usize, ts: *mut NanoTimeVal) -> SyscallResult {
    if ts.is_null() {
        return Err(SysError::EFAULT);
    }
    _set_sum_bit();
    let ns = current_time().as_nanos();
    let token = current_user_token();
    *translated_refmut(token, ts) = NanoTimeVal {
        sec: (ns / 1_000_000_000) as i64,
        nsec: (ns % 1_000_000_000) as i64,
    };
    Ok(0)
}

pub fn sys_clock_nanosleep(
    clock_id: usize,
    flags: usize,
    req: *const TimeSpec,
    rem: *mut TimeSpec,
) -> SyscallResult {
    const CLOCK_REALTIME: usize = 0;
    const CLOCK_MONOTONIC: usize = 1;
    const TIMER_ABSTIME: usize = 1;

    if req.is_null() {
        return Err(SysError::EFAULT);
    }
    if clock_id != CLOCK_REALTIME && clock_id != CLOCK_MONOTONIC {
        return Err(SysError::EINVAL);
    }

    let token = current_user_token();
    let req_ts = *translated_ref(token, req);
    if req_ts.tv_sec < 0 || req_ts.tv_nsec < 0 || req_ts.tv_nsec >= 1_000_000_000 {
        return Err(SysError::EINVAL);
    }

    let now_us = current_time().as_micros() as i128;
    let req_us = req_ts.tv_sec as i128 * 1_000_000 + req_ts.tv_nsec as i128 / 1_000;
    let deadline_us = if (flags & TIMER_ABSTIME) != 0 {
        req_us
    } else {
        now_us + req_us
    };

    while (current_time().as_micros() as i128) < deadline_us {
        sys_yield()?;
    }

    if !rem.is_null() {
        *translated_refmut(token, rem) = TimeSpec {
            tv_sec: 0,
            tv_nsec: 0,
        };
    }
    Ok(0)
}

/// gettimeofday 风格的时间获取。
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> SyscallResult {
    _set_sum_bit();
    let ns = current_time().as_nanos() as u128;
    unsafe {
        *ts = TimeVal {
            sec: (ns / 1_000_000_000) as i64,
            usec: ((ns / 1_000) % 1_000_000) as i64,
        };
    }
    Ok(0)
}
