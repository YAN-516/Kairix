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
use crate::fs::File;
use crate::fs::vfs::Dentry;
use crate::fs::vfs::DentryInner;
use crate::fs::vfs::FileInner;
use crate::fs::vfs::inode::InodeMode;
use crate::trap::_set_sum_bit;
use crate::{add_timer, remove_task_from_timer_queue};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
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

const SIGEV_SIGNAL: i32 = 0;
const SIGEV_NONE: i32 = 1;
const SIGEV_THREAD_ID: i32 = 4;
const TIMER_ABSTIME: i32 = 1;

#[repr(C)]
#[derive(Clone, Copy)]
struct KernelSigEvent {
    value: usize,
    signo: i32,
    notify: i32,
    tid: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ItimerSpec {
    pub it_interval: TimeSpec,
    pub it_value: TimeSpec,
}

#[derive(Clone, Copy)]
enum PosixTimerTarget {
    None,
    Process(i32),
    Thread { tid: usize, signo: i32 },
}

struct PosixTimer {
    process: Weak<crate::task::ProcessControlBlock>,
    clock_id: i32,
    target: PosixTimerTarget,
    interval_ns: u128,
    deadline_ns: Option<u128>,
    overrun: i32,
}

static NEXT_POSIX_TIMER_ID: AtomicUsize = AtomicUsize::new(1);
static POSIX_TIMERS: Mutex<BTreeMap<(usize, usize), PosixTimer>> = Mutex::new(BTreeMap::new());

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TimeSpec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

fn timespec_to_ns(value: TimeSpec) -> Result<u128, SysError> {
    if value.tv_sec < 0 || value.tv_nsec < 0 || value.tv_nsec >= 1_000_000_000 {
        return Err(SysError::EINVAL);
    }
    Ok((value.tv_sec as u128)
        .saturating_mul(1_000_000_000)
        .saturating_add(value.tv_nsec as u128))
}

fn ns_to_timespec(value: u128) -> TimeSpec {
    TimeSpec {
        tv_sec: (value / 1_000_000_000).min(i64::MAX as u128) as i64,
        tv_nsec: (value % 1_000_000_000) as i64,
    }
}

fn current_clock_ns(clock_id: i32) -> u128 {
    if clock_id == 0 {
        crate::timer::realtime_ns()
    } else {
        current_time().as_nanos()
    }
}

pub fn sys_timer_create(clock_id: i32, event: usize, timer_id: *mut i32) -> SyscallResult {
    const CLOCK_REALTIME: i32 = 0;
    const CLOCK_MONOTONIC: i32 = 1;

    if timer_id.is_null() {
        return Err(SysError::EFAULT);
    }
    if !matches!(clock_id, CLOCK_REALTIME | CLOCK_MONOTONIC) {
        return Err(SysError::EINVAL);
    }

    let token = current_user_token();
    let target = if event == 0 {
        PosixTimerTarget::Process(crate::task::signal::Signal::SigAlrm.as_i32())
    } else {
        let event = *translated_ref(token, event as *const KernelSigEvent)?;
        match event.notify {
            SIGEV_NONE => PosixTimerTarget::None,
            SIGEV_SIGNAL => {
                if crate::task::signal::Signal::from_i32(event.signo).is_none() {
                    return Err(SysError::EINVAL);
                }
                PosixTimerTarget::Process(event.signo)
            }
            SIGEV_THREAD_ID => {
                if event.tid <= 0 || crate::task::signal::Signal::from_i32(event.signo).is_none() {
                    return Err(SysError::EINVAL);
                }
                PosixTimerTarget::Thread {
                    tid: event.tid as usize,
                    signo: event.signo,
                }
            }
            _ => return Err(SysError::EINVAL),
        }
    };

    let process = current_process();
    let pid = process.getpid();
    let id = NEXT_POSIX_TIMER_ID.fetch_add(1, Ordering::Relaxed);
    POSIX_TIMERS.lock().insert((pid, id), PosixTimer {
        process: Arc::downgrade(&process),
        clock_id,
        target,
        interval_ns: 0,
        deadline_ns: None,
        overrun: 0,
    });
    *translated_refmut(token, timer_id)? = id as i32;
    Ok(0)
}

pub fn sys_timer_settime(
    timer_id: usize,
    flags: i32,
    new_value: *const ItimerSpec,
    old_value: *mut ItimerSpec,
) -> SyscallResult {
    if new_value.is_null() {
        return Err(SysError::EFAULT);
    }
    if flags & !TIMER_ABSTIME != 0 {
        return Err(SysError::EINVAL);
    }

    let token = current_user_token();
    let new_value = *translated_ref(token, new_value)?;
    let value_ns = timespec_to_ns(new_value.it_value)?;
    let interval_ns = timespec_to_ns(new_value.it_interval)?;
    let pid = current_process().getpid();
    let mut timers = POSIX_TIMERS.lock();
    let timer = timers.get_mut(&(pid, timer_id)).ok_or(SysError::EINVAL)?;

    if !old_value.is_null() {
        let remaining = timer
            .deadline_ns
            .map(|deadline| deadline.saturating_sub(current_time().as_nanos()))
            .unwrap_or(0);
        *translated_refmut(token, old_value)? = ItimerSpec {
            it_interval: ns_to_timespec(timer.interval_ns),
            it_value: ns_to_timespec(remaining),
        };
    }

    timer.interval_ns = interval_ns;
    let monotonic_now = current_time().as_nanos();
    timer.deadline_ns = if value_ns == 0 {
        None
    } else if flags & TIMER_ABSTIME != 0 {
        Some(
            monotonic_now.saturating_add(value_ns.saturating_sub(current_clock_ns(timer.clock_id))),
        )
    } else {
        Some(monotonic_now.saturating_add(value_ns))
    };
    timer.overrun = 0;
    Ok(0)
}

pub fn sys_timer_gettime(timer_id: usize, value: *mut ItimerSpec) -> SyscallResult {
    if value.is_null() {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let pid = current_process().getpid();
    let timers = POSIX_TIMERS.lock();
    let timer = timers.get(&(pid, timer_id)).ok_or(SysError::EINVAL)?;
    let remaining = timer
        .deadline_ns
        .map(|deadline| deadline.saturating_sub(current_time().as_nanos()))
        .unwrap_or(0);
    *translated_refmut(token, value)? = ItimerSpec {
        it_interval: ns_to_timespec(timer.interval_ns),
        it_value: ns_to_timespec(remaining),
    };
    Ok(0)
}

pub fn sys_timer_getoverrun(timer_id: usize) -> SyscallResult {
    let pid = current_process().getpid();
    POSIX_TIMERS
        .lock()
        .get(&(pid, timer_id))
        .map(|timer| timer.overrun as usize)
        .ok_or(SysError::EINVAL)
}

pub fn sys_timer_delete(timer_id: usize) -> SyscallResult {
    let pid = current_process().getpid();
    POSIX_TIMERS
        .lock()
        .remove(&(pid, timer_id))
        .map(|_| 0)
        .ok_or(SysError::EINVAL)
}

pub(crate) fn check_posix_timers() {
    let now = current_time().as_nanos();
    let mut expired = Vec::new();
    {
        let mut timers = POSIX_TIMERS.lock();
        timers.retain(|_, timer| timer.process.strong_count() > 0);
        for timer in timers.values_mut() {
            let Some(deadline) = timer.deadline_ns else {
                continue;
            };
            if now < deadline {
                continue;
            }
            let Some(process) = timer.process.upgrade() else {
                continue;
            };
            let target = match timer.target {
                PosixTimerTarget::None => None,
                PosixTimerTarget::Process(signo) => Some((process, None, signo)),
                PosixTimerTarget::Thread { tid, signo } => Some((process, Some(tid), signo)),
            };
            if let Some(target) = target {
                expired.push(target);
            }
            if timer.interval_ns == 0 {
                timer.deadline_ns = None;
            } else {
                let elapsed = now.saturating_sub(deadline);
                let periods = elapsed / timer.interval_ns + 1;
                timer.overrun = periods.saturating_sub(1).min(i32::MAX as u128) as i32;
                timer.deadline_ns =
                    Some(deadline.saturating_add(periods.saturating_mul(timer.interval_ns)));
            }
        }
    }

    for (process, tid, signo) in expired {
        let Some(signal) = crate::task::signal::Signal::from_i32(signo) else {
            continue;
        };
        if let Some(tid) = tid {
            let Some(task) = crate::task::tid2task(tid) else {
                continue;
            };
            if task.process.upgrade().map(|owner| owner.getpid()) != Some(process.getpid()) {
                continue;
            }
            let blocked = {
                let mut inner = task.inner_exclusive_access();
                inner.pending_signals.add(signal);
                inner.need_signal_handle = true;
                inner.interrupted_by_signal = true;
                inner.task_status == crate::task::TaskStatus::Blocked
            };
            if blocked {
                crate::task::wakeup_task(task);
            }
        } else {
            crate::syscall::signal::deliver_signal(&process, signal);
        }
    }
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
    if _req.is_null() {
        return Err(SysError::EFAULT);
    }
    // musl 的 nanosleep/usleep 传递的是 timespec（秒 + 纳秒），
    // 不是 timeval（秒 + 微秒）。必须将纳秒转换为微秒。
    let token = current_user_token();
    let req = translated_ref(token, _req)?;
    let req_sec = req.sec;
    let req_nsec = req.nsec;
    let sleep_time_us = req_sec as i128 * 1_000_000 + (req_nsec as i128 / 1_000);
    if req_nsec < 0 || req_nsec >= 1_000_000_000 || sleep_time_us < 0 {
        return Err(SysError::EINVAL);
    }
    let task = current_task().unwrap();
    let start_ns = current_time().as_nanos();
    let sleep_time_ns = (sleep_time_us as u128) * 1000;
    let wakeup_time = start_ns + sleep_time_ns;

    let mut inner = task.inner_exclusive_access();
    inner.task_status = TaskStatus::Sleep;
    add_timer(task.clone(), wakeup_time);
    drop(inner);

    loop {
        block_current_and_run_next();
        let mut inner = task.inner_exclusive_access();
        inner.interrupted_by_signal = false;
        drop(inner);

        if crate::syscall::signal::should_interrupt_syscall() {
            remove_task_from_timer_queue(&task);
            if !_rem.is_null() {
                let now_ns = current_time().as_nanos();
                let rem_ns = wakeup_time.saturating_sub(now_ns);
                *translated_refmut(token, _rem)? = NanoTimeVal {
                    sec: (rem_ns / 1_000_000_000) as i64,
                    nsec: (rem_ns % 1_000_000_000) as i64,
                };
            }
            return Err(SysError::EINTR);
        }

        if current_time().as_nanos() >= wakeup_time {
            return Ok(0);
        }
    }
}

pub fn sys_clock_gettime(clock: usize, ts: *mut NanoTimeVal) -> SyscallResult {
    const CLOCK_REALTIME: usize = 0;

    if ts.is_null() {
        return Err(SysError::EFAULT);
    }
    _set_sum_bit();
    let ns = if clock == CLOCK_REALTIME {
        crate::timer::realtime_ns()
    } else {
        current_time().as_nanos()
    };
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

    let monotonic_now_ns = current_time().as_nanos() as i128;
    let clock_now_ns = if clock_id == CLOCK_REALTIME {
        crate::timer::realtime_ns() as i128
    } else {
        monotonic_now_ns
    };
    let req_ns = req_ts.tv_sec as i128 * 1_000_000_000 + req_ts.tv_nsec as i128;
    let duration_ns = if (flags & TIMER_ABSTIME) != 0 {
        req_ns.saturating_sub(clock_now_ns)
    } else {
        req_ns
    };
    if duration_ns <= 0 {
        return Ok(0);
    }
    let deadline_ns = monotonic_now_ns.saturating_add(duration_ns);
    let task = current_task().unwrap();
    let pid = task
        .process
        .upgrade()
        .map(|process| process.getpid())
        .unwrap_or(usize::MAX);
    let global_tid = task.inner_exclusive_access().global_tid;
    log::warn!(
        "[CLOCK_NANOSLEEP] enter cpu={} pid={} tid={} now_ns={} deadline_ns={} duration_ns={} flags={:#x}",
        polyhal::arch::hart_id(),
        pid,
        global_tid,
        monotonic_now_ns,
        deadline_ns,
        deadline_ns.saturating_sub(monotonic_now_ns),
        flags,
    );
    let mut inner = task.inner_exclusive_access();
    inner.task_status = TaskStatus::Sleep;
    drop(inner);
    add_timer(task.clone(), deadline_ns as u128);
    log::error!(
        "[CLOCK_NANOSLEEP_QUEUED_VISIBLE] cpu={} pid={} tid={} deadline_ns={}",
        polyhal::arch::hart_id(),
        pid,
        global_tid,
        deadline_ns,
    );
    log::warn!(
        "[CLOCK_NANOSLEEP] queued cpu={} pid={} tid={} deadline_ns={}",
        polyhal::arch::hart_id(),
        pid,
        global_tid,
        deadline_ns,
    );
    loop {
        log::warn!(
            "[CLOCK_NANOSLEEP] block cpu={} pid={} tid={} deadline_ns={}",
            polyhal::arch::hart_id(),
            pid,
            global_tid,
            deadline_ns,
        );
        block_current_and_run_next();
        let resumed_ns = current_time().as_nanos() as i128;
        log::error!(
            "[CLOCK_NANOSLEEP_RESUME_VISIBLE] cpu={} pid={} tid={} now_ns={} deadline_ns={}",
            polyhal::arch::hart_id(),
            pid,
            global_tid,
            resumed_ns,
            deadline_ns,
        );
        log::warn!(
            "[CLOCK_NANOSLEEP] resume cpu={} pid={} tid={} now_ns={} deadline_ns={}",
            polyhal::arch::hart_id(),
            pid,
            global_tid,
            resumed_ns,
            deadline_ns,
        );
        let mut inner = task.inner_exclusive_access();
        inner.interrupted_by_signal = false;
        drop(inner);

        if crate::syscall::signal::should_interrupt_syscall() {
            remove_task_from_timer_queue(&task);
            if !rem.is_null() {
                let now_ns = current_time().as_nanos() as i128;
                let rem_ns = deadline_ns.saturating_sub(now_ns).max(0);
                *translated_refmut(token, rem)? = TimeSpec {
                    tv_sec: (rem_ns / 1_000_000_000) as i64,
                    tv_nsec: (rem_ns % 1_000_000_000) as i64,
                };
            }
            return Err(SysError::EINTR);
        }

        if resumed_ns >= deadline_ns {
            break;
        }
    }

    if !rem.is_null() {
        *translated_refmut(token, rem)? = TimeSpec {
            tv_sec: 0,
            tv_nsec: 0,
        };
    }
    log::warn!(
        "[CLOCK_NANOSLEEP] done cpu={} pid={} tid={} deadline_ns={}",
        polyhal::arch::hart_id(),
        pid,
        global_tid,
        deadline_ns,
    );
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
        // Linux permits a null result pointer.
        return Ok(0);
    }

    let resolution_ns = crate::timer::clock_resolution_ns();
    let token = current_user_token();
    *translated_refmut(token, res)? = NanoTimeVal {
        sec: (resolution_ns / 1_000_000_000) as i64,
        nsec: (resolution_ns % 1_000_000_000) as i64,
    };
    Ok(0)
}
