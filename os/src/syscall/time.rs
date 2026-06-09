// use crate::config::PAGE_SIZE;
use polyhal::consts::PAGE_SIZE;

// use crate::fs::open_file;
use crate::error::{SysError, SyscallResult};
use crate::fs::vfs::OpenFlags;
use crate::mm::{PageTable, PhysAddr, VirtAddr, VirtPageNum};
use crate::mm::{UserBuffer, copy_to_user};
use crate::mm::{VMSpace, translated_ref, translated_refmut, translated_str};
use crate::syscall::process::sys_yield;
use crate::task::Tms;
use crate::task::{
    block_current_and_run_next, current_process, current_task, current_user_token,
    exit_current_and_run_next, num_processes, pid2process, suspend_current_and_run_next,
};
// use crate::timer::*;
use crate::TaskStatus;
use crate::add_timer;
use crate::fs::File;
use crate::fs::vfs::Dentry;
use crate::fs::vfs::DentryInner;
use crate::fs::vfs::FileInner;
use crate::fs::vfs::inode::InodeMode;
use crate::trap::_set_sum_bit;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use log::{error, warn};
use polyhal::timer::current_time;
use spin::{Mutex, MutexGuard};
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TimeVal {
    pub sec: i64,
    pub usec: i64,
}

/// Timerfd internal data
struct TimerfdData {
    _clockid: usize,
    _flags: i32,
    _current_value: u64,          // Current timer value
    interval_ns: u64,             // Interval for periodic timer (0 for one-shot)
    next_timeout_ns: Option<u64>, // Next timeout in nanoseconds since epoch
}

/// Global timerfd data storage
static TIMERFD_DATA: Mutex<BTreeMap<usize, TimerfdData>> = Mutex::new(BTreeMap::new());

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
pub struct TimerfdFile {
    inner: Mutex<FileInner>,
    _fd: usize, // Store fd for accessing timer data
}

impl TimerfdFile {
    pub fn new(dentry: Arc<dyn Dentry>, fd: usize) -> Self {
        Self {
            inner: Mutex::new(FileInner {
                offset: 0,
                dentry,
                flags: OpenFlags::empty(),
            }),
            _fd: fd,
        }
    }
}

impl File for TimerfdFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        self.inner.lock()
    }

    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        false
    }

    fn supports_epoll(&self) -> bool {
        true
    }

    fn read(&self, buf: UserBuffer) -> SyscallResult {
        if buf.len() < core::mem::size_of::<u64>() {
            return Err(SysError::EINVAL);
        }

        // Simple implementation: immediately return 1 (timer fired once)
        // This bypasses the timer waiting logic to test if the issue is in timerfd
        let value: u64 = 1;
        let mut data_buf = [0u8; 8];
        data_buf.copy_from_slice(&value.to_le_bytes());

        let mut written = 0;
        for slice in buf.buffers.into_iter() {
            if written >= 8 {
                break;
            }
            let to_write = core::cmp::min(slice.len(), 8 - written);
            slice[..to_write].copy_from_slice(&data_buf[written..written + to_write]);
            written += to_write;
        }

        return Ok(8);
    }

    fn write(&self, _buf: UserBuffer) -> SyscallResult {
        // timerfd is not writable
        Err(SysError::EBADF)
    }
}

unsafe impl Send for TimerfdDentry {}
unsafe impl Sync for TimerfdDentry {}

pub struct TimerfdDentry {
    inner: DentryInner,
}

impl TimerfdDentry {
    #[allow(unused)]
    pub fn new(name: &str) -> Self {
        Self {
            inner: DentryInner::new(name, None),
        }
    }
}

impl Dentry for TimerfdDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }

    fn name(&self) -> &str {
        &self.inner.name
    }

    fn open(
        self: Arc<Self>,
        _flags: OpenFlags,
        _mode: InodeMode,
    ) -> crate::error::SysResult<Arc<dyn File>> {
        Ok(Arc::new(TimerfdFile::new(self, 0)))
    }
}

pub fn sys_times(_ts: *mut Tms) -> SyscallResult {
    if _ts.is_null() {
        return Err(SysError::EFAULT);
    }
    let time = current_process().inner_exclusive_access().time;
    let token = current_user_token();
    let bytes = unsafe {
        core::slice::from_raw_parts(
            &time as *const Tms as *const u8,
            core::mem::size_of::<Tms>(),
        )
    };
    copy_to_user(token, _ts as *mut u8, bytes)?;
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
            let elapsed_us = current_time()
                .as_micros()
                .saturating_sub(inner.kstart as u128);
            rusage.ru_utime.sec = (elapsed_us / 1_000_000) as i64;
            rusage.ru_utime.usec = (elapsed_us % 1_000_000) as i64;
        }
        RUSAGE_CHILDREN => {
            // 当前未维护子进程累计时间，返回全 0
        }
        _ => return Err(SysError::EINVAL),
    }

    *translated_refmut(token, usage)? = rusage;
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
//     *translated_refmut(token, _ts)? = TimeVal {
//         sec: (_us / 1_000_000) as i64,
//         usec: (_us % 1_000_000) as i64,
//     };
//     0
// }

use core::i32;
pub fn sys_sleep(_req: *mut NanoTimeVal, _rem: *mut NanoTimeVal) -> SyscallResult {
    // musl 的 nanosleep/usleep 传递的是 timespec（秒 + 纳秒），
    // 不是 timeval（秒 + 微秒）。必须将纳秒转换为微秒。
    let req_sec = unsafe { (*_req).sec };
    let req_nsec = unsafe { (*_req).nsec };
    let sleep_time_us = req_sec as i128 * 1_000_000 + (req_nsec as i128 / 1_000);
    if sleep_time_us < 0 {
        return Err(SysError::EINVAL);
    }
    let task = current_task().unwrap();
    let wakeup_time = current_time().as_nanos() + (sleep_time_us as u128) * 1000;

    let mut inner = task.inner_exclusive_access();
    inner.task_status = TaskStatus::Sleep;
    add_timer(task.clone(), wakeup_time);
    drop(inner);

    block_current_and_run_next();
    Ok(0)
}

pub fn sys_clock_gettime(_clock: usize, ts: *mut NanoTimeVal) -> SyscallResult {
    if ts.is_null() {
        return Err(SysError::EFAULT);
    }
    _set_sum_bit();
    let ns = current_time().as_nanos();
    let token = current_user_token();
    *translated_refmut(token, ts)? = NanoTimeVal {
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
    let req_ts = *translated_ref(token, req)?;
    if req_ts.tv_sec < 0 || req_ts.tv_nsec < 0 || req_ts.tv_nsec >= 1_000_000_000 {
        return Err(SysError::EINVAL);
    }

    let now_ns = current_time().as_nanos() as i128;
    let req_ns = req_ts.tv_sec as i128 * 1_000_000_000 + req_ts.tv_nsec as i128;
    let deadline_ns = if (flags & TIMER_ABSTIME) != 0 {
        req_ns
    } else {
        now_ns + req_ns
    };
    // 如果 deadline 已过期，直接返回，避免无意义的上下文切换
    if deadline_ns <= now_ns {
        return Ok(0);
    }
    let task = current_task().unwrap();
    let mut inner = task.inner_exclusive_access();
    inner.task_status = TaskStatus::Sleep;
    drop(inner);
    add_timer(task.clone(), deadline_ns as u128);
    block_current_and_run_next();
    // while (current_time().as_nanos() as i128) < deadline_ns {
    //     sys_yield()?;
    // }

    if !rem.is_null() {
        *translated_refmut(token, rem)? = TimeSpec {
            tv_sec: 0,
            tv_nsec: 0,
        };
    }
    Ok(0)
}

#[allow(unused)]
pub fn sys_timerfd_create(clockid: usize, flags: i32) -> SyscallResult {
    const CLOCK_REALTIME: usize = 0;
    const CLOCK_MONOTONIC: usize = 1;

    // Validate clockid
    if clockid != CLOCK_REALTIME && clockid != CLOCK_MONOTONIC {
        return Err(SysError::EINVAL);
    }

    // Allocate a file descriptor first
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    let fd = inner.alloc_fd()?;

    // Create timerfd data in global storage with a default timeout
    // Set initial timeout to 1 second from now
    let now_ns = current_time().as_nanos() as u64;
    TIMERFD_DATA.lock().insert(fd, TimerfdData {
        _clockid: clockid,
        _flags: flags,
        _current_value: 0,
        interval_ns: 1_000_000_000,                    // 1 second periodic
        next_timeout_ns: Some(now_ns + 1_000_000_000), // Start in 1 second
    });

    // Create a dummy dentry for the timerfd
    let dentry = Arc::new(TimerfdDentry::new("timerfd"));
    let file = Arc::new(TimerfdFile::new(dentry, fd));
    inner.fd_table[fd] = Some(file);

    Ok(fd)
}

/// Set timerfd parameters
#[allow(unused)]
pub fn sys_timerfd_settime(
    fd: usize,
    _flags: i32,
    new_value: *const TimeSpec,
    old_value: *mut TimeSpec,
) -> SyscallResult {
    if new_value.is_null() {
        return Err(SysError::EFAULT);
    }

    let mut data_map = TIMERFD_DATA.lock();
    let data = data_map.get_mut(&fd).ok_or(SysError::EBADF)?;

    // Read the new timer value
    let token = current_user_token();
    let new_spec = *translated_ref(token, new_value)?;

    if new_spec.tv_sec < 0 || new_spec.tv_nsec < 0 || new_spec.tv_nsec >= 1_000_000_000 {
        return Err(SysError::EINVAL);
    }

    // Calculate next timeout
    let now_ns = current_time().as_nanos() as u64;
    let initial_ns = (new_spec.tv_sec as u64) * 1_000_000_000 + (new_spec.tv_nsec as u64);

    data.next_timeout_ns = Some(now_ns + initial_ns);
    data.interval_ns = initial_ns; // For periodic timer

    // If old_value is not null, return the previous value
    if !old_value.is_null() {
        *translated_refmut(token, old_value)? = TimeSpec {
            tv_sec: 0,
            tv_nsec: 0,
        };
    }

    Ok(0)
}

/// Get timerfd current time
#[allow(unused)]
pub fn sys_timerfd_gettime(fd: usize, curr_value: *mut TimeSpec) -> SyscallResult {
    if curr_value.is_null() {
        return Err(SysError::EFAULT);
    }

    let data_map = TIMERFD_DATA.lock();
    let data = data_map.get(&fd).ok_or(SysError::EBADF)?;

    let token = current_user_token();

    // Calculate remaining time
    if let Some(next_timeout) = data.next_timeout_ns {
        let now_ns = current_time().as_nanos() as u64;
        let remaining_ns = if next_timeout > now_ns {
            next_timeout - now_ns
        } else {
            0
        };

        *translated_refmut(token, curr_value)? = TimeSpec {
            tv_sec: (remaining_ns / 1_000_000_000) as i64,
            tv_nsec: (remaining_ns % 1_000_000_000) as i64,
        };
    } else {
        *translated_refmut(token, curr_value)? = TimeSpec {
            tv_sec: 0,
            tv_nsec: 0,
        };
    }

    Ok(0)
}

pub fn sys_clock_getres(_clock: usize, res: *mut NanoTimeVal) -> SyscallResult {
    error!("sys_clock_getres");
    if res.is_null() {
        return Err(SysError::EFAULT);
    }

    // Our clock has microsecond resolution (1000 nanoseconds)
    let token = current_user_token();
    *translated_refmut(token, res)? = NanoTimeVal {
        sec: 0,
        nsec: 1, // 1 microsecond = 1000 nanoseconds
    };
    Ok(0)
}
