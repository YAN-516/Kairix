use crate::config::PAGE_SIZE;
use crate::fs::open_file;
use crate::fs::vfs::OpenFlags;
use crate::mm::{PageTable, PhysAddr, VirtAddr, VirtPageNum};
use crate::mm::{VMSpace, translated_ref, translated_refmut, translated_str};
use crate::syscall::process::sys_yield;
use crate::task::Tms;
use crate::task::{
    block_current_and_run_next, current_process, current_task, current_user_token,
    exit_current_and_run_next, pid2process, suspend_current_and_run_next,
};
use crate::timer::*;
use crate::trap::_set_sum_bit;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use log::{error, warn};
#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
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
    _set_sum_bit();
    let _us = get_time_us();
    unsafe {
        *(_ts) = TimeVal {
            sec: _us / 1_000_000,
            usec: _us % 1_000_000,
        };
    }
    0
}

use core::i32;
pub fn sys_sleep(_req: *mut TimeVal, _rem: *mut TimeVal) -> isize {
    _set_sum_bit();
    let time_start = get_time_us();
    let mut sleep_time;
    unsafe {
        sleep_time = (*(_req)).sec * 1_000_000 + (*(_req)).usec;
    }

    loop {
        let time_now = get_time_us();
        let time_has_sleep = time_now - time_start;
        sleep_time -= time_has_sleep;
        //println!("{} {}", sleep_time, time_has_sleep);
        if sleep_time <= 0 || sleep_time > i32::MAX as usize {
            sleep_time = 0;
        }
        unsafe {
            *(_rem) = TimeVal {
                sec: sleep_time / 1_000_000,
                usec: sleep_time % 1_000_000,
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
