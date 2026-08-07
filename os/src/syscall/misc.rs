use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::devfs::urandom::fill_random;
use crate::fs::vfs::{File, FileInner};
use crate::mm::copy_to_user;
use crate::mm::{UserBuffer, get_free_memory, get_total_memory, translated_ref, write_user_value};
use crate::syscall::signal::consume_pending_signal;
use crate::task::signal::{SigInfo, Signal, SignalSet};
use crate::task::{
    ProcessControlBlock, TaskControlBlock, block_current_and_run_next, current_process,
    current_task, current_user_token, num_processes, pid2process, wakeup_task,
};
use polyhal::timer::current_time;

#[cfg(target_arch = "riscv64")]
use crate::timer::*;
use crate::trap::_set_sum_bit;
use alloc::collections::VecDeque;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::mem::size_of;
use spin::{Mutex, MutexGuard};

const LINUX_CAPABILITY_VERSION_3: u32 = 0x20080522;
const O_CLOEXEC: i32 = 0o2000000;
const O_NONBLOCK: u32 = 0o0004000;
const SIGNALFD_SIGINFO_SIZE: usize = 128;
const SIGNALFD_SSI_ADDR_OFFSET: usize = 72;
const _: () = assert!(SIGNALFD_SSI_ADDR_OFFSET + size_of::<u64>() <= SIGNALFD_SIGINFO_SIZE);
struct AnonFdFile {
    name: &'static str,
    status_flags: Mutex<u32>,
}

const EVENTFD_COUNTER_MAX: u64 = u64::MAX - 1;

struct EventFdState {
    counter: u64,
    read_waiters: VecDeque<Weak<TaskControlBlock>>,
    write_waiters: VecDeque<Weak<TaskControlBlock>>,
    poll_waiters: VecDeque<Weak<TaskControlBlock>>,
}

struct EventFdFile {
    state: Mutex<EventFdState>,
    semaphore: bool,
    status_flags: Mutex<u32>,
}

struct SignalFdFile {
    mask: Mutex<SignalSet>,
    status_flags: Mutex<u32>,
}

struct SignalFdWaiter {
    process: Weak<ProcessControlBlock>,
    task: Weak<TaskControlBlock>,
    mask: u64,
}

lazy_static::lazy_static! {
    static ref SIGNALFD_WAITERS: Mutex<Vec<SignalFdWaiter>> = Mutex::new(Vec::new());
}

pub(crate) fn wake_signalfd_waiters(process: &Arc<ProcessControlBlock>, signal: Signal) {
    let bit = 1u64 << (signal.as_i32() - 1);
    let mut wake = Vec::new();
    SIGNALFD_WAITERS.lock().retain(|waiter| {
        let Some(waiter_process) = waiter.process.upgrade() else {
            return false;
        };
        let Some(task) = waiter.task.upgrade() else {
            return false;
        };
        if Arc::ptr_eq(&waiter_process, process) && waiter.mask & bit != 0 {
            wake.push(task);
            false
        } else {
            true
        }
    });
    for task in wake {
        wakeup_task(task);
    }
}

fn wake_signalfd_mask_update(process: &Arc<ProcessControlBlock>) {
    let mut wake = Vec::new();
    SIGNALFD_WAITERS.lock().retain(|waiter| {
        let Some(waiter_process) = waiter.process.upgrade() else {
            return false;
        };
        let Some(task) = waiter.task.upgrade() else {
            return false;
        };
        if Arc::ptr_eq(&waiter_process, process) {
            wake.push(task);
            false
        } else {
            true
        }
    });
    for task in wake {
        wakeup_task(task);
    }
}

impl SignalFdFile {
    fn new(mask: SignalSet, status_flags: u32) -> Self {
        Self {
            mask: Mutex::new(mask),
            status_flags: Mutex::new(status_flags),
        }
    }

    fn nonblock(&self) -> bool {
        *self.status_flags.lock() & O_NONBLOCK != 0
    }

    fn register_waiter(&self, task: &Arc<TaskControlBlock>) {
        let Some(process) = task.process.upgrade() else {
            return;
        };
        let mask = self.mask.lock().bits();
        let mut waiters = SIGNALFD_WAITERS.lock();
        waiters
            .retain(|waiter| waiter.task.upgrade().is_some() && waiter.process.upgrade().is_some());
        if let Some(waiter) = waiters.iter_mut().find(|waiter| {
            waiter
                .task
                .upgrade()
                .is_some_and(|existing| Arc::ptr_eq(&existing, task))
        }) {
            waiter.mask |= mask;
            return;
        }
        waiters.push(SignalFdWaiter {
            process: Arc::downgrade(&process),
            task: Arc::downgrade(task),
            mask,
        });
    }

    fn clear_waiter(task: &Arc<TaskControlBlock>) {
        SIGNALFD_WAITERS.lock().retain(|waiter| {
            waiter
                .task
                .upgrade()
                .is_some_and(|existing| !Arc::ptr_eq(&existing, task))
        });
    }

    fn pending_bits(&self) -> u64 {
        let mask = self.mask.lock().bits();
        let Some(task) = current_task() else {
            return 0;
        };
        let process = current_process();
        let process_pending = process.inner_exclusive_access().pending_signals.bits();
        let task_pending = task.inner_exclusive_access().pending_signals.bits();
        (process_pending | task_pending) & mask
    }

    fn take_one(&self) -> Option<SigInfo> {
        let mask = self.mask.lock().bits();
        let task = current_task()?;
        let process = current_process();
        let mut process_inner = process.inner_exclusive_access();
        let mut task_inner = task.inner_exclusive_access();
        let matched =
            (process_inner.pending_signals.bits() | task_inner.pending_signals.bits()) & mask;
        let signal = Signal::from_i32(matched.trailing_zeros().checked_add(1)? as i32)?;
        let info = if task_inner.pending_signals.contains(signal) {
            let inner = &mut *task_inner;
            consume_pending_signal(
                &mut inner.pending_signals,
                &mut inner.pending_signal_queue,
                signal,
            )
        } else {
            let inner = &mut *process_inner;
            consume_pending_signal(
                &mut inner.pending_signals,
                &mut inner.pending_signal_queue,
                signal,
            )
        };
        task_inner.need_signal_handle =
            (task_inner.pending_signals.bits() & !task_inner.blocked_signals.bits()) != 0;
        process_inner.need_signal_handle =
            (process_inner.pending_signals.bits() & !task_inner.blocked_signals.bits()) != 0;
        Some(info.unwrap_or(SigInfo {
            si_signo: signal.as_i32(),
            si_errno: 0,
            si_code: 0,
            si_pid: 0,
            si_uid: 0,
            si_value: 0,
            si_addr: None,
        }))
    }

    fn encode(info: SigInfo) -> [u8; SIGNALFD_SIGINFO_SIZE] {
        let mut record = [0u8; SIGNALFD_SIGINFO_SIZE];
        record[0..4].copy_from_slice(&(info.si_signo as u32).to_ne_bytes());
        record[4..8].copy_from_slice(&info.si_errno.to_ne_bytes());
        record[8..12].copy_from_slice(&info.si_code.to_ne_bytes());
        record[12..16].copy_from_slice(&(info.si_pid as u32).to_ne_bytes());
        record[16..20].copy_from_slice(&info.si_uid.to_ne_bytes());
        record[44..48].copy_from_slice(&info.si_value.to_ne_bytes());
        if let Some(address) = info.si_addr {
            record[SIGNALFD_SSI_ADDR_OFFSET..SIGNALFD_SSI_ADDR_OFFSET + size_of::<u64>()]
                .copy_from_slice(&(address as u64).to_ne_bytes());
        }
        record
    }

    fn collect(&self, max_records: usize) -> SysResult<Vec<[u8; SIGNALFD_SIGINFO_SIZE]>> {
        let mut records = Vec::new();
        loop {
            while records.len() < max_records {
                let Some(info) = self.take_one() else {
                    break;
                };
                records.push(Self::encode(info));
            }
            if !records.is_empty() {
                if let Some(task) = current_task() {
                    Self::clear_waiter(&task);
                }
                return Ok(records);
            }
            if self.nonblock() {
                return Err(SysError::EAGAIN);
            }
            let task = current_task().ok_or(SysError::ESRCH)?;
            self.register_waiter(&task);
            // Close the pending-check/register race before blocking.  A wake
            // arriving after this check is recorded by wakeup_task even while
            // the task is still running.
            if self.pending_bits() != 0 {
                Self::clear_waiter(&task);
                continue;
            }
            block_current_and_run_next();
            if EventFdFile::interrupted_after_block() {
                Self::clear_waiter(&task);
                return Err(SysError::EINTR);
            }
        }
    }
}

impl EventFdFile {
    fn new(initval: u32, semaphore: bool, status_flags: u32) -> Self {
        Self {
            state: Mutex::new(EventFdState {
                counter: u64::from(initval),
                read_waiters: VecDeque::new(),
                write_waiters: VecDeque::new(),
                poll_waiters: VecDeque::new(),
            }),
            semaphore,
            status_flags: Mutex::new(status_flags),
        }
    }

    fn nonblock(&self) -> bool {
        *self.status_flags.lock() & O_NONBLOCK != 0
    }

    fn register_waiter(
        waiters: &mut VecDeque<Weak<TaskControlBlock>>,
        task: Arc<TaskControlBlock>,
    ) {
        let mut registered = false;
        waiters.retain(|waiter| {
            if let Some(waiter) = waiter.upgrade() {
                if Arc::ptr_eq(&waiter, &task) {
                    registered = true;
                }
                true
            } else {
                false
            }
        });
        if !registered {
            waiters.push_back(Arc::downgrade(&task));
        }
    }

    fn clear_waiter(waiters: &mut VecDeque<Weak<TaskControlBlock>>, task: &Arc<TaskControlBlock>) {
        waiters.retain(|waiter| {
            waiter
                .upgrade()
                .is_some_and(|waiter| !Arc::ptr_eq(&waiter, task))
        });
    }

    fn wake_waiters(mut waiters: VecDeque<Weak<TaskControlBlock>>) {
        while let Some(waiter) = waiters.pop_front() {
            if let Some(task) = waiter.upgrade() {
                wakeup_task(task);
            }
        }
    }

    fn interrupted_after_block() -> bool {
        current_process().inner_exclusive_access().is_zombie
            || crate::syscall::signal::should_interrupt_syscall()
    }

    fn copy_from_buffer(buf: UserBuffer, out: &mut [u8]) {
        let mut copied = 0;
        for slice in buf.buffers {
            if copied == out.len() {
                break;
            }
            let copy_len = slice.len().min(out.len() - copied);
            out[copied..copied + copy_len].copy_from_slice(&slice[..copy_len]);
            copied += copy_len;
        }
    }

    fn copy_to_buffer(mut buf: UserBuffer, src: &[u8]) {
        let mut copied = 0;
        for slice in buf.buffers.iter_mut() {
            if copied == src.len() {
                break;
            }
            let copy_len = slice.len().min(src.len() - copied);
            slice[..copy_len].copy_from_slice(&src[copied..copied + copy_len]);
            copied += copy_len;
        }
    }
}

impl File for EventFdFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        panic!("eventfd has no FileInner")
    }

    fn get_inode(&self) -> Option<Arc<dyn crate::fs::vfs::inode::Inode>> {
        None
    }

    fn get_offset(&self) -> usize {
        0
    }

    fn set_offset(&self, _new_offset: usize) {}

    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        true
    }

    fn read(&self, buf: UserBuffer) -> Result<usize, SysError> {
        if buf.len() < core::mem::size_of::<u64>() {
            return Err(SysError::EINVAL);
        }

        loop {
            let mut state = self.state.lock();
            if state.counter != 0 {
                let value = if self.semaphore { 1 } else { state.counter };
                state.counter -= value;
                let write_waiters = core::mem::take(&mut state.write_waiters);
                let poll_waiters = core::mem::take(&mut state.poll_waiters);
                drop(state);

                Self::copy_to_buffer(buf, &value.to_ne_bytes());
                Self::wake_waiters(write_waiters);
                Self::wake_waiters(poll_waiters);
                return Ok(core::mem::size_of::<u64>());
            }
            if self.nonblock() {
                return Err(SysError::EAGAIN);
            }
            let task = current_task().ok_or(SysError::ESRCH)?;
            Self::register_waiter(&mut state.read_waiters, task.clone());
            drop(state);
            block_current_and_run_next();
            if Self::interrupted_after_block() {
                Self::clear_waiter(&mut self.state.lock().read_waiters, &task);
                return Err(SysError::EINTR);
            }
        }
    }

    fn write(&self, buf: UserBuffer) -> Result<usize, SysError> {
        if buf.len() < core::mem::size_of::<u64>() {
            return Err(SysError::EINVAL);
        }
        let mut bytes = [0u8; core::mem::size_of::<u64>()];
        Self::copy_from_buffer(buf, &mut bytes);
        let value = u64::from_ne_bytes(bytes);
        if value == u64::MAX {
            return Err(SysError::EINVAL);
        }

        loop {
            let mut state = self.state.lock();
            if value <= EVENTFD_COUNTER_MAX - state.counter {
                state.counter += value;
                let read_waiters = core::mem::take(&mut state.read_waiters);
                let poll_waiters = core::mem::take(&mut state.poll_waiters);
                drop(state);

                Self::wake_waiters(read_waiters);
                Self::wake_waiters(poll_waiters);
                return Ok(core::mem::size_of::<u64>());
            }
            if self.nonblock() {
                return Err(SysError::EAGAIN);
            }
            let task = current_task().ok_or(SysError::ESRCH)?;
            Self::register_waiter(&mut state.write_waiters, task.clone());
            drop(state);
            block_current_and_run_next();
            if Self::interrupted_after_block() {
                Self::clear_waiter(&mut self.state.lock().write_waiters, &task);
                return Err(SysError::EINTR);
            }
        }
    }

    fn status_flags(&self) -> u32 {
        *self.status_flags.lock()
    }

    fn set_status_flags(&self, flags: u32) {
        let mut status_flags = self.status_flags.lock();
        *status_flags = (*status_flags & !O_NONBLOCK) | (flags & O_NONBLOCK);
    }

    fn supports_epoll(&self) -> bool {
        true
    }

    fn read_ready(&self) -> Option<bool> {
        Some(self.state.lock().counter != 0)
    }

    fn write_ready(&self) -> Option<bool> {
        Some(self.state.lock().counter < EVENTFD_COUNTER_MAX)
    }

    fn register_poll_waker(&self, task: Arc<TaskControlBlock>) {
        Self::register_waiter(&mut self.state.lock().poll_waiters, task);
    }

    fn clear_poll_waker(&self, task: &Arc<TaskControlBlock>) {
        Self::clear_waiter(&mut self.state.lock().poll_waiters, task);
    }

    fn wake_poll_waiters(&self) {
        let waiters = core::mem::take(&mut self.state.lock().poll_waiters);
        Self::wake_waiters(waiters);
    }
}

impl File for SignalFdFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        panic!("signalfd has no FileInner")
    }

    fn get_inode(&self) -> Option<Arc<dyn crate::fs::vfs::inode::Inode>> {
        None
    }

    fn get_offset(&self) -> usize {
        0
    }

    fn set_offset(&self, _new_offset: usize) {}

    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        false
    }

    fn read(&self, mut buf: UserBuffer) -> Result<usize, SysError> {
        const RECORD_SIZE: usize = 128;
        if buf.len() < RECORD_SIZE {
            return Err(SysError::EINVAL);
        }
        let records = self.collect(buf.len() / RECORD_SIZE)?;
        let mut destination_offset = 0usize;
        let total = records.len() * RECORD_SIZE;
        for destination in buf.buffers.iter_mut() {
            let mut slice_offset = 0usize;
            while slice_offset < destination.len() && destination_offset < total {
                let record_index = destination_offset / RECORD_SIZE;
                let record_offset = destination_offset % RECORD_SIZE;
                let copy_len = (RECORD_SIZE - record_offset)
                    .min(destination.len() - slice_offset)
                    .min(total - destination_offset);
                destination[slice_offset..slice_offset + copy_len].copy_from_slice(
                    &records[record_index][record_offset..record_offset + copy_len],
                );
                slice_offset += copy_len;
                destination_offset += copy_len;
            }
        }
        Ok(total)
    }

    fn read_user(&self, token: usize, buf: *mut u8, len: usize) -> Result<usize, SysError> {
        const RECORD_SIZE: usize = 128;
        if len < RECORD_SIZE {
            return Err(SysError::EINVAL);
        }
        let records = self.collect(len / RECORD_SIZE)?;
        let mut copied = 0usize;
        for record in records.iter() {
            copy_to_user(token, unsafe { buf.add(copied) }, record)?;
            copied += RECORD_SIZE;
        }
        Ok(copied)
    }

    fn write(&self, _buf: UserBuffer) -> Result<usize, SysError> {
        Err(SysError::EINVAL)
    }

    fn status_flags(&self) -> u32 {
        *self.status_flags.lock()
    }

    fn set_status_flags(&self, flags: u32) {
        let mut status_flags = self.status_flags.lock();
        *status_flags = (*status_flags & !O_NONBLOCK) | (flags & O_NONBLOCK);
    }

    fn is_signalfd(&self) -> bool {
        true
    }

    fn set_signalfd_mask(&self, mask: u64) -> SyscallResult {
        *self.mask.lock() = SignalSet::from_bits(mask).without_unblockable();
        wake_signalfd_mask_update(&current_process());
        Ok(0)
    }

    fn supports_epoll(&self) -> bool {
        true
    }

    fn read_ready(&self) -> Option<bool> {
        Some(self.pending_bits() != 0)
    }

    fn requires_active_poll(&self) -> bool {
        true
    }

    fn register_poll_waker(&self, task: Arc<TaskControlBlock>) {
        self.register_waiter(&task);
    }

    fn clear_poll_waker(&self, task: &Arc<TaskControlBlock>) {
        Self::clear_waiter(task);
    }
}

impl AnonFdFile {
    fn new(name: &'static str, status_flags: u32) -> Self {
        Self {
            name,
            status_flags: Mutex::new(status_flags),
        }
    }
}

impl File for AnonFdFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        panic!("anonymous fd has no FileInner")
    }

    fn get_inode(&self) -> Option<Arc<dyn crate::fs::vfs::inode::Inode>> {
        None
    }

    fn get_offset(&self) -> usize {
        0
    }

    fn set_offset(&self, _new_offset: usize) {}

    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        true
    }

    fn read(&self, _buf: UserBuffer) -> Result<usize, SysError> {
        Err(SysError::EBADF)
    }

    fn write(&self, _buf: UserBuffer) -> Result<usize, SysError> {
        Err(SysError::EBADF)
    }

    fn status_flags(&self) -> u32 {
        *self.status_flags.lock()
    }

    fn set_status_flags(&self, flags: u32) {
        let mut status_flags = self.status_flags.lock();
        *status_flags = (*status_flags & !O_NONBLOCK) | (flags & O_NONBLOCK);
    }

    fn is_open_tree_fd(&self) -> bool {
        self.name == "open_tree"
    }
}

pub(crate) fn alloc_anon_fd(name: &'static str, cloexec: bool, status_flags: u32) -> SyscallResult {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    let fd = inner.alloc_fd()?;
    inner.fd_table[fd] = Some(Arc::new(AnonFdFile::new(name, status_flags)));
    if cloexec && fd < inner.fd_flags.len() {
        inner.fd_flags[fd] |= 1;
    }
    Ok(fd)
}

fn cloexec_from_flags(flags: i32) -> bool {
    flags & O_CLOEXEC != 0
}

fn status_from_flags(flags: i32) -> u32 {
    if flags & O_NONBLOCK as i32 != 0 {
        O_NONBLOCK
    } else {
        0
    }
}

pub fn sys_eventfd2(initval: u32, flags: i32) -> SyscallResult {
    const EFD_SEMAPHORE: i32 = 1;
    if flags & !(EFD_SEMAPHORE | O_CLOEXEC | O_NONBLOCK as i32) != 0 {
        return Err(SysError::EINVAL);
    }
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    let fd = inner.alloc_fd()?;
    inner.fd_table[fd] = Some(Arc::new(EventFdFile::new(
        initval,
        flags & EFD_SEMAPHORE != 0,
        status_from_flags(flags),
    )));
    if cloexec_from_flags(flags) && fd < inner.fd_flags.len() {
        inner.fd_flags[fd] |= 1;
    }
    Ok(fd)
}

pub fn sys_signalfd4(fd: isize, mask: usize, sizemask: usize, flags: i32) -> SyscallResult {
    if flags & !(O_CLOEXEC | O_NONBLOCK as i32) != 0 {
        return Err(SysError::EINVAL);
    }
    if sizemask != core::mem::size_of::<u64>() {
        return Err(SysError::EINVAL);
    }
    if mask == 0 {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let mask =
        SignalSet::from_bits(*translated_ref(token, mask as *const u64)?).without_unblockable();

    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    if fd == -1 {
        let new_fd = inner.alloc_fd()?;
        inner.fd_table[new_fd] = Some(Arc::new(SignalFdFile::new(mask, status_from_flags(flags))));
        if cloexec_from_flags(flags) && new_fd < inner.fd_flags.len() {
            inner.fd_flags[new_fd] |= 1;
        }
        return Ok(new_fd);
    }
    let fd = usize::try_from(fd).map_err(|_| SysError::EBADF)?;
    let file = inner
        .fd_table
        .get(fd)
        .and_then(|entry| entry.as_ref())
        .cloned()
        .ok_or(SysError::EBADF)?;
    drop(inner);
    if !file.is_signalfd() {
        return Err(SysError::EINVAL);
    }
    file.set_signalfd_mask(mask.bits())?;
    Ok(fd)
}

pub fn sys_pidfd_open(pid: usize, flags: u32) -> SyscallResult {
    if flags != 0 {
        return Err(SysError::EINVAL);
    }
    if pid2process(pid).is_none() {
        return Err(SysError::ESRCH);
    }
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    let fd = inner.alloc_fd()?;
    inner.fd_table[fd] = Some(Arc::new(crate::fs::pidfd::PidFdFile::new(pid)));
    Ok(fd)
}

pub fn sys_userfaultfd(flags: i32) -> SyscallResult {
    if flags & !(O_CLOEXEC | O_NONBLOCK as i32) != 0 {
        return Err(SysError::EINVAL);
    }
    Err(SysError::ENOSYS)
}

pub fn sys_perf_event_open(
    _attr: usize,
    _pid: isize,
    _cpu: isize,
    _group_fd: isize,
    flags: u32,
) -> SyscallResult {
    if flags & !O_CLOEXEC as u32 != 0 {
        return Err(SysError::EINVAL);
    }
    Err(SysError::ENOSYS)
}

pub fn sys_io_uring_setup(entries: u32, _params: usize) -> SyscallResult {
    if entries == 0 {
        return Err(SysError::EINVAL);
    }
    Err(SysError::ENOSYS)
}

pub fn sys_bpf(cmd: u32, _attr: usize, _size: u32) -> SyscallResult {
    const BPF_MAP_CREATE: u32 = 0;
    if cmd != BPF_MAP_CREATE {
        return Err(SysError::EINVAL);
    }
    Err(SysError::ENOSYS)
}

pub fn sys_memfd_secret(flags: u32) -> SyscallResult {
    if flags != 0 {
        return Err(SysError::EINVAL);
    }
    Err(SysError::ENOSYS)
}

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
    let mut header = *translated_ref(token, hdrp as *const CapUserHeader)?;

    if header.version != LINUX_CAPABILITY_VERSION_3 {
        header.version = LINUX_CAPABILITY_VERSION_3;
        write_user_value(token, hdrp as *mut CapUserHeader, &header)?;
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

    let has_cap_sys_admin = current_process().inner_exclusive_access().has_cap_sys_admin;
    let mut effective0 = !0u32;
    let mut permitted0 = !0u32;
    const CAP_SYS_ADMIN: u32 = 21;
    if !has_cap_sys_admin {
        effective0 &= !(1 << CAP_SYS_ADMIN);
        permitted0 &= !(1 << CAP_SYS_ADMIN);
    }

    // V3 requires two CapUserData structs (64 capabilities)
    let data0 = CapUserData {
        effective: effective0,
        permitted: permitted0,
        inheritable: !0u32,
    };
    write_user_value(token, datap as *mut CapUserData, &data0)?;

    let data1 = CapUserData {
        effective: !0u32,
        permitted: !0u32,
        inheritable: !0u32,
    };
    write_user_value(token, unsafe { (datap as *mut CapUserData).add(1) }, &data1)?;

    Ok(0)
}

/// capset: set process capabilities.
pub fn sys_capset(hdrp: usize, datap: usize) -> SyscallResult {
    if hdrp == 0 || datap == 0 {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let mut header = *translated_ref(token, hdrp as *const CapUserHeader)?;

    if header.version != LINUX_CAPABILITY_VERSION_3 {
        header.version = LINUX_CAPABILITY_VERSION_3;
        write_user_value(token, hdrp as *mut CapUserHeader, &header)?;
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

    const CAP_SYS_ADMIN: u32 = 21;
    let data0 = *translated_ref(token, datap as *const CapUserData)?;
    current_process().inner_exclusive_access().has_cap_sys_admin =
        data0.effective & (1 << CAP_SYS_ADMIN) != 0;
    Ok(0)
}

/// Fill userspace from the same ChaCha20 generator used by `/dev/urandom`.
pub fn sys_getrandom(buf: *mut u8, buflen: usize, flags: u32) -> SyscallResult {
    const GRND_NONBLOCK: u32 = 0x0001;
    const GRND_RANDOM: u32 = 0x0002;
    const GRND_INSECURE: u32 = 0x0004;
    const VALID_FLAGS: u32 = GRND_NONBLOCK | GRND_RANDOM | GRND_INSECURE;

    if flags & !VALID_FLAGS != 0
        || flags & (GRND_RANDOM | GRND_INSECURE) == (GRND_RANDOM | GRND_INSECURE)
    {
        return Err(SysError::EINVAL);
    }
    if buflen == 0 {
        return Ok(0);
    }
    if buf.is_null() {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let mut local_buf = [0u8; 256];
    let mut copied = 0usize;
    while copied < buflen {
        let chunk_len = local_buf.len().min(buflen - copied);
        fill_random(&mut local_buf[..chunk_len]);
        let destination = unsafe { buf.add(copied) };
        if let Err(err) = copy_to_user(token, destination, &local_buf[..chunk_len]) {
            return if copied == 0 { Err(err) } else { Ok(copied) };
        }
        copied += chunk_len;
    }
    Ok(copied)
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
    copy_to_user(token, info as *mut u8, src_bytes)?;
    Ok(0)
}

/// membarrier: execute a full memory barrier on every online CPU and wait for
/// acknowledgement. Targeting all online CPUs is a correct (if conservative)
/// implementation for both global and private expedited commands.
pub fn sys_membarrier(cmd: i32, flags: i32, _cpu_mask: *mut u64) -> SyscallResult {
    const MEMBARRIER_CMD_QUERY: i32 = 0;
    const MEMBARRIER_CMD_GLOBAL: i32 = 1;
    const MEMBARRIER_CMD_GLOBAL_EXPEDITED: i32 = 2;
    const MEMBARRIER_CMD_REGISTER_GLOBAL_EXPEDITED: i32 = 4;
    const MEMBARRIER_CMD_PRIVATE_EXPEDITED: i32 = 8;
    const MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED: i32 = 16;
    const SUPPORTED: i32 = MEMBARRIER_CMD_GLOBAL
        | MEMBARRIER_CMD_GLOBAL_EXPEDITED
        | MEMBARRIER_CMD_REGISTER_GLOBAL_EXPEDITED
        | MEMBARRIER_CMD_PRIVATE_EXPEDITED
        | MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED;

    // Check flags - only flag currently defined is MEMBARRIER_FLAG_CPU_MASK
    if flags != 0 {
        return Err(SysError::EINVAL);
    }

    match cmd {
        MEMBARRIER_CMD_QUERY => Ok(SUPPORTED as usize),
        MEMBARRIER_CMD_GLOBAL => synchronize_all_online_cpus(),
        MEMBARRIER_CMD_REGISTER_GLOBAL_EXPEDITED => {
            current_process()
                .vm_exclusive_access()
                .membarrier_registrations |= MEMBARRIER_CMD_GLOBAL_EXPEDITED as u32;
            Ok(0)
        }
        MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED => {
            current_process()
                .vm_exclusive_access()
                .membarrier_registrations |= MEMBARRIER_CMD_PRIVATE_EXPEDITED as u32;
            Ok(0)
        }
        MEMBARRIER_CMD_GLOBAL_EXPEDITED | MEMBARRIER_CMD_PRIVATE_EXPEDITED => {
            let registered = current_process()
                .vm_exclusive_access()
                .membarrier_registrations;
            if registered & cmd as u32 == 0 {
                return Err(SysError::EPERM);
            }
            synchronize_all_online_cpus()
        }
        _ => Err(SysError::EINVAL),
    }
}

fn synchronize_all_online_cpus() -> SyscallResult {
    let mask = crate::task::manager::online_cpu_mask();
    if polyhal::multicore::synchronize_memory_cpus(mask) {
        Ok(0)
    } else {
        Err(SysError::EIO)
    }
}
