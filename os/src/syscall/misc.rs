use core::mem::size_of;
use crate::mm::{get_free_memory, get_total_memory};
use crate::task::{
    block_current_and_run_next, current_process, current_task, current_user_token,
    exit_current_and_run_next, pid2process, suspend_current_and_run_next, num_processes,
};
use crate::timer::*;
use crate::trap::_set_sum_bit;
use alloc::string::String;
use crate::mm::copy_to_user;
use lazy_static::lazy_static;
use spin::Mutex;

lazy_static! {
    static ref RNG_STATE: Mutex<u64> = Mutex::new(0);
}

fn xorshift64(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// getrandom: fill user buffer with pseudo-random bytes.
/// Since Kairix has no hardware RNG, we use a simple xorshift64 PRNG.
pub fn sys_getrandom(buf: *mut u8, buflen: usize, _flags: u32) -> isize {
    if buflen == 0 {
        return 0;
    }
    let token = current_user_token();
    let mut state = RNG_STATE.lock();
    // Initialize seed on first use using time and hart id
    if *state == 0 {
        *state = (get_time_us() as u64)
            .wrapping_add(crate::sbi::get_tp() as u64)
            .wrapping_add(0x9e3779b97f4a7c15);
    }
    let mut written = 0usize;
    while written < buflen {
        let byte = xorshift64(&mut *state) as u8;
        let dst = unsafe { buf.add(written) } as *const u8;
        copy_to_user(token, dst, &[byte]);
        written += 1;
    }
    buflen as isize
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SysInfo {
    pub uptime: i64,
    pub loads: [u64; 3],
    pub totalram: u64,
    pub freeram: u64,
    pub sharedram: u64,
    pub bufferram: u64,
    pub totalswap: u64,
    pub freeswap: u64,
    pub procs: u16,
    pub pad: u16,
    pub totalhigh: u64,
    pub freehigh: u64,
    pub mem_unit: u32,
    pub _f: [u8; 4],
}

impl SysInfo {
    pub fn new() -> Self {
        Self {
            uptime: 0,
            loads: [0; 3],
            totalram: 0,
            freeram: 0,
            sharedram: 0,
            bufferram: 0,
            totalswap: 0,
            freeswap: 0,
            procs: 0,
            pad: 0,
            totalhigh: 0,
            freehigh: 0,
            mem_unit: 1,
            _f: [0; 4],
        }
    }
}

pub fn sys_sysinfo(info: *mut SysInfo) -> isize {
    const EFAULT: isize = -14;
    if info.is_null() {
        return EFAULT;
    }
    _set_sum_bit();
    let token = current_user_token();
    let mut sysinfo = SysInfo::new();
    sysinfo.uptime = (get_time_us() / 1_000_000) as i64;
    sysinfo.totalram = get_total_memory() as u64;
    sysinfo.freeram = get_free_memory() as u64;
    sysinfo.procs = num_processes() as u16;
    sysinfo.mem_unit = 1;

    let src_bytes = unsafe { core::slice::from_raw_parts(&sysinfo as *const _ as *const u8, size_of::<SysInfo>()) };
    copy_to_user(token, info as *const u8, src_bytes);
    0
}
