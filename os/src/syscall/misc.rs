use crate::error::{SysError, SyscallResult};
use crate::fs::devfs::urandom::fill_random;
use crate::mm::copy_to_user;
use crate::mm::{get_free_memory, get_total_memory, translated_refmut};
use crate::task::{
    block_current_and_run_next, current_process, current_task, current_user_token,
    exit_current_and_run_next, num_processes, pid2process, suspend_current_and_run_next,
};
use polyhal::timer::current_time;

#[cfg(target_arch = "riscv64")]
use crate::timer::*;
use crate::trap::_set_sum_bit;
use alloc::string::String;
use alloc::vec::Vec;
use core::mem::size_of;

const LINUX_CAPABILITY_VERSION_3: u32 = 0x20080522;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct CapUserHeader {
    version: u32,
    pid: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct CapUserData {
    effective: u32,
    permitted: u32,
    inheritable: u32,
}

/// capget: get process capabilities.
/// For now, all processes are treated as having full capabilities (root).
pub fn sys_capget(hdrp: usize, datap: usize) -> SyscallResult {
    if hdrp == 0 || datap == 0 {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let header = translated_refmut(token, hdrp as *mut CapUserHeader)?;

    if header.version != LINUX_CAPABILITY_VERSION_3 {
        header.version = LINUX_CAPABILITY_VERSION_3;
        return Err(SysError::EINVAL);
    }

    let pid = header.pid;
    if pid < 0 {
        return Err(SysError::EINVAL);
    }
    if pid != 0 {
        let current_pid = current_task()
            .and_then(|t| t.process.upgrade().map(|p| p.getpid() as i32))
            .unwrap_or(0);
        if pid != current_pid {
            return Err(SysError::ESRCH);
        }
    }

    // V3 requires two CapUserData structs (64 capabilities)
    let data0 = translated_refmut(token, datap as *mut CapUserData)?;
    data0.effective = !0u32;
    data0.permitted = !0u32;
    data0.inheritable = !0u32;

    let data1 = translated_refmut(token, unsafe { (datap as *mut CapUserData).add(1) })?;
    data1.effective = !0u32;
    data1.permitted = !0u32;
    data1.inheritable = !0u32;

    Ok(0)
}

/// capset: set process capabilities.
/// For now, accepts but ignores the request (stub implementation).
pub fn sys_capset(hdrp: usize, _datap: usize) -> SyscallResult {
    if hdrp == 0 {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let header = translated_refmut(token, hdrp as *mut CapUserHeader)?;

    if header.version != LINUX_CAPABILITY_VERSION_3 {
        header.version = LINUX_CAPABILITY_VERSION_3;
        return Err(SysError::EINVAL);
    }

    let pid = header.pid;
    if pid < 0 {
        return Err(SysError::EINVAL);
    }
    if pid != 0 {
        let current_pid = current_task()
            .and_then(|t| t.process.upgrade().map(|p| p.getpid() as i32))
            .unwrap_or(0);
        if pid != current_pid {
            return Err(SysError::EPERM);
        }
    }

    // Stub: ignore actual capability changes.
    Ok(0)
}

/// getrandom: fill user buffer with pseudo-random bytes.
/// Since Kairix has no hardware RNG, we use a simple xorshift64 PRNG.
/// 现在复用 /dev/urandom 的 fill_random 实现，避免逐字节拷贝。
pub fn sys_getrandom(buf: *mut u8, buflen: usize, _flags: u32) -> SyscallResult {
    if buflen == 0 {
        return Ok(0);
    }
    if buf.is_null() {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let mut local_buf = Vec::with_capacity(buflen);
    local_buf.resize(buflen, 0u8);
    fill_random(&mut local_buf);
    copy_to_user(token, buf as *const u8, &local_buf);
    Ok(buflen)
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

pub fn sys_sysinfo(info: *mut SysInfo) -> SyscallResult {
    if info.is_null() {
        return Err(SysError::EFAULT);
    }
    _set_sum_bit();
    let token = current_user_token();
    let mut sysinfo = SysInfo::new();
    sysinfo.uptime = (current_time().as_micros() / 1_000_000) as i64;
    sysinfo.totalram = get_total_memory() as u64;
    sysinfo.freeram = get_free_memory() as u64;
    sysinfo.procs = num_processes() as u16;
    sysinfo.mem_unit = 1;

    let src_bytes = unsafe {
        core::slice::from_raw_parts(&sysinfo as *const _ as *const u8, size_of::<SysInfo>())
    };
    copy_to_user(token, info as *const u8, src_bytes);
    Ok(0)
}
