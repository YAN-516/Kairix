use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::vfs::{File, FileInner};
use crate::mm::UserBuffer;
use crate::syscall::signal::{install_temporary_signal_mask, pending_signal_interrupts_wait};
use crate::task::signal::SignalSet;
use crate::task::{
    TaskControlBlock, block_current_and_run_next, current_process, current_task,
    current_user_token, suspend_current_and_run_next, wakeup_task,
};
use alloc::collections::{BTreeMap, VecDeque};
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use polyhal::timer::current_time;
use spin::Mutex;
use spin::MutexGuard;

const EPOLL_CLOEXEC: i32 = 0o2000000;
const EPOLL_CTL_ADD: i32 = 1;
const EPOLL_CTL_DEL: i32 = 2;
const EPOLL_CTL_MOD: i32 = 3;

const EPOLLIN: u32 = 0x001;
const EPOLLPRI: u32 = 0x002;
const EPOLLOUT: u32 = 0x004;
const EPOLLERR: u32 = 0x008;
const EPOLLHUP: u32 = 0x010;
const EPOLLRDHUP: u32 = 0x2000;
const EPOLLWAKEUP: u32 = 1 << 29;
const EPOLLONESHOT: u32 = 1 << 30;
const EPOLLET: u32 = 1 << 31;
const EPOLL_CTL_MAX_NESTING: usize = 5;
const EPOLL_USER_EVENTS: u32 = EPOLLIN
    | EPOLLPRI
    | EPOLLOUT
    | EPOLLERR
    | EPOLLHUP
    | EPOLLRDHUP
    | EPOLLWAKEUP
    | EPOLLONESHOT
    | EPOLLET;

static NEXT_EPOLL_ID: Mutex<usize> = Mutex::new(1);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct EpollEvent {
    events: u32,
    data: u64,
}

const EPOLL_EVENT_SIZE: usize = core::mem::size_of::<EpollEvent>();
const EPOLL_EVENT_DATA_OFFSET: usize = EPOLL_EVENT_SIZE - core::mem::size_of::<u64>();

#[derive(Clone)]
struct EpollInterest {
    file: Arc<dyn File + Send + Sync>,
    event: EpollEvent,
}

struct EpollState {
    interests: BTreeMap<i32, EpollInterest>,
    waiters: VecDeque<Weak<TaskControlBlock>>,
    has_nested_epoll: bool,
}

struct EpollFile {
    id: usize,
    state: Mutex<EpollState>,
    status_flags: u32,
}

impl EpollFile {
    fn new(status_flags: u32) -> Self {
        let mut next_id = NEXT_EPOLL_ID.lock();
        let id = *next_id;
        *next_id = next_id.saturating_add(1).max(1);
        Self {
            id,
            state: Mutex::new(EpollState {
                interests: BTreeMap::new(),
                waiters: VecDeque::new(),
                has_nested_epoll: false,
            }),
            status_flags,
        }
    }

    fn add(&self, fd: i32, file: Arc<dyn File + Send + Sync>, event: EpollEvent) -> SyscallResult {
        let mut state = self.state.lock();
        if state.interests.contains_key(&fd) {
            return Err(SysError::EEXIST);
        }
        let is_nested_epoll = file.is_epoll();
        state.interests.insert(fd, EpollInterest { file, event });
        if is_nested_epoll {
            state.has_nested_epoll = true;
        }
        self.wake_waiters_locked(&mut state);
        Ok(0)
    }

    fn modify(&self, fd: i32, event: EpollEvent) -> SyscallResult {
        let mut state = self.state.lock();
        let Some(interest) = state.interests.get_mut(&fd) else {
            return Err(SysError::ENOENT);
        };
        interest.event = event;
        self.wake_waiters_locked(&mut state);
        Ok(0)
    }

    fn delete(&self, fd: i32) -> SyscallResult {
        let mut state = self.state.lock();
        if state.interests.remove(&fd).is_none() {
            return Err(SysError::ENOENT);
        }
        state.has_nested_epoll = state
            .interests
            .values()
            .any(|interest| interest.file.is_epoll());
        self.wake_waiters_locked(&mut state);
        Ok(0)
    }

    fn ready_events(&self, maxevents: usize) -> Vec<EpollEvent> {
        let state = self.state.lock();
        let mut ready = Vec::new();
        for interest in state.interests.values() {
            if let Some(event) = ready_epoll_event(interest) {
                ready.push(event);
                if ready.len() == maxevents {
                    break;
                }
            }
        }
        ready
    }

    fn register_interest_wakers(&self, task: Arc<TaskControlBlock>) {
        let state = self.state.lock();
        for interest in state.interests.values() {
            interest.file.register_poll_waker(task.clone());
        }
    }

    fn clear_interest_wakers(&self, task: &Arc<TaskControlBlock>) {
        let state = self.state.lock();
        for interest in state.interests.values() {
            interest.file.clear_poll_waker(task);
        }
    }

    fn register_waiter(&self, task: Arc<TaskControlBlock>) {
        let mut state = self.state.lock();
        let mut queued = false;
        state.waiters.retain(|waiter| {
            if let Some(waiter) = waiter.upgrade() {
                if Arc::ptr_eq(&waiter, &task) {
                    queued = true;
                }
                true
            } else {
                false
            }
        });
        if !queued {
            state.waiters.push_back(Arc::downgrade(&task));
        }
    }

    fn clear_waiter(&self, task: &Arc<TaskControlBlock>) {
        let mut state = self.state.lock();
        state.waiters.retain(|waiter| {
            waiter
                .upgrade()
                .is_some_and(|waiter| !Arc::ptr_eq(&waiter, task))
        });
    }

    fn wake_waiters_locked(&self, state: &mut EpollState) {
        while let Some(waiter) = state.waiters.pop_front() {
            if let Some(task) = waiter.upgrade() {
                wakeup_task(task);
            }
        }
    }

    fn wake_waiters(&self) {
        let mut state = self.state.lock();
        self.wake_waiters_locked(&mut state);
    }

    fn nesting_depth(&self) -> usize {
        let state = self.state.lock();
        state
            .interests
            .values()
            .filter(|interest| interest.file.is_epoll())
            .map(|interest| 1 + interest.file.epoll_nesting_depth())
            .max()
            .unwrap_or(0)
    }

    fn contains_epoll_id(&self, id: usize) -> bool {
        if self.id == id {
            return true;
        }
        let state = self.state.lock();
        state
            .interests
            .values()
            .any(|interest| interest.file.epoll_contains_id(id))
    }
}

impl File for EpollFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        panic!("epoll fd has no FileInner")
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
        Err(SysError::EINVAL)
    }

    fn write(&self, _buf: UserBuffer) -> Result<usize, SysError> {
        Err(SysError::EINVAL)
    }

    fn status_flags(&self) -> u32 {
        self.status_flags
    }

    fn is_epoll(&self) -> bool {
        true
    }

    fn supports_epoll(&self) -> bool {
        true
    }

    fn epoll_id(&self) -> Option<usize> {
        Some(self.id)
    }

    fn epoll_watches_epoll(&self) -> bool {
        self.state.lock().has_nested_epoll
    }

    fn epoll_nesting_depth(&self) -> usize {
        self.nesting_depth()
    }

    fn epoll_contains_id(&self, id: usize) -> bool {
        self.contains_epoll_id(id)
    }

    fn read_ready(&self) -> Option<bool> {
        Some(!self.ready_events(1).is_empty())
    }

    fn register_poll_waker(&self, task: Arc<TaskControlBlock>) {
        self.register_waiter(task);
    }

    fn clear_poll_waker(&self, task: &Arc<TaskControlBlock>) {
        self.clear_waiter(task);
    }

    fn wake_poll_waiters(&self) {
        self.wake_waiters();
    }

    fn epoll_add(
        &self,
        fd: i32,
        file: Arc<dyn File + Send + Sync>,
        events: u32,
        data: u64,
    ) -> SyscallResult {
        self.add(fd, file, EpollEvent { events, data })
    }

    fn epoll_modify(&self, fd: i32, events: u32, data: u64) -> SyscallResult {
        self.modify(fd, EpollEvent { events, data })
    }

    fn epoll_delete(&self, fd: i32) -> SyscallResult {
        self.delete(fd)
    }

    fn epoll_ready_events(&self, maxevents: usize) -> Vec<(u32, u64)> {
        self.ready_events(maxevents)
            .into_iter()
            .map(|event| (event.events, event.data))
            .collect()
    }

    fn epoll_register_interest_wakers(&self, task: Arc<TaskControlBlock>) {
        self.register_interest_wakers(task);
    }

    fn epoll_clear_interest_wakers(&self, task: &Arc<TaskControlBlock>) {
        self.clear_interest_wakers(task);
    }
}

fn get_fd_file(fd: usize) -> SysResult<Arc<dyn File + Send + Sync>> {
    let process = current_process();
    let inner = process.inner_exclusive_access();
    inner
        .fd_table
        .get(fd)
        .and_then(|file| file.as_ref())
        .cloned()
        .ok_or(SysError::EBADF)
}

fn ready_epoll_event(interest: &EpollInterest) -> Option<EpollEvent> {
    let (readable, writable) = epoll_file_ready(&interest.file);
    let wanted = interest.event.events;
    let mut events = 0;

    if (wanted & EPOLLIN) != 0 && readable {
        events |= EPOLLIN;
    }
    if (wanted & EPOLLPRI) != 0 && readable {
        events |= EPOLLPRI;
    }
    if (wanted & EPOLLOUT) != 0 && writable {
        events |= EPOLLOUT;
    }

    if events != 0 {
        Some(EpollEvent {
            events: (interest.event.events & !EPOLL_USER_EVENTS) | events,
            data: interest.event.data,
        })
    } else {
        None
    }
}

fn epoll_file_ready(file: &Arc<dyn File + Send + Sync>) -> (bool, bool) {
    if file.is_socket() {
        let process = current_process();
        let pid = process.getpid();
        let manager = crate::socket::SOCKET_MANAGER.lock();
        if let Some(sock) = manager
            .sockets
            .iter()
            .find(|socket| socket.pid == pid && socket.fd == socket_fd_from_file(file))
        {
            return crate::socket::socket_ready(&sock.inner);
        }
    }

    if file.is_pipe() {
        let readable =
            file.readable() && (file.pipe_has_data() || file.pipe_all_write_ends_closed());
        let writable =
            file.writable() && file.pipe_has_space() && !file.pipe_all_read_ends_closed();
        return (readable, writable);
    }

    if let Some(is_read_ready) = file.read_ready() {
        let writable = file
            .write_ready()
            .map(|is_write_ready| file.writable() && is_write_ready)
            .unwrap_or_else(|| file.writable());
        return (file.readable() && is_read_ready, writable);
    }

    (file.readable(), file.writable())
}

fn socket_fd_from_file(file: &Arc<dyn File + Send + Sync>) -> usize {
    let process = current_process();
    let inner = process.inner_exclusive_access();
    for (fd, candidate) in inner.fd_table.iter().enumerate() {
        if let Some(candidate) = candidate {
            if Arc::ptr_eq(candidate, file) {
                return fd;
            }
        }
    }
    usize::MAX
}

fn read_epoll_event(token: usize, event_ptr: usize) -> SysResult<EpollEvent> {
    if event_ptr == 0 {
        return Err(SysError::EFAULT);
    }
    let bytes = read_user_bytes(token, event_ptr as *const u8, EPOLL_EVENT_SIZE)?;
    let events = u32::from_ne_bytes(bytes[0..4].try_into().map_err(|_| SysError::EFAULT)?);
    let data = u64::from_ne_bytes(
        bytes[EPOLL_EVENT_DATA_OFFSET..EPOLL_EVENT_DATA_OFFSET + core::mem::size_of::<u64>()]
            .try_into()
            .map_err(|_| SysError::EFAULT)?,
    );
    Ok(EpollEvent { events, data })
}

fn write_epoll_events(token: usize, events_ptr: usize, events: &[EpollEvent]) -> SysResult<()> {
    if events.is_empty() {
        return Ok(());
    }
    if events_ptr == 0 {
        return Err(SysError::EFAULT);
    }
    for (idx, event) in events.iter().enumerate() {
        let mut event_raw = [0u8; EPOLL_EVENT_SIZE];
        event_raw[0..4].copy_from_slice(&event.events.to_ne_bytes());
        event_raw[EPOLL_EVENT_DATA_OFFSET..EPOLL_EVENT_DATA_OFFSET + core::mem::size_of::<u64>()]
            .copy_from_slice(&event.data.to_ne_bytes());
        write_user_bytes(
            token,
            (events_ptr + idx * EPOLL_EVENT_SIZE) as *mut u8,
            &event_raw,
        )?;
    }
    Ok(())
}

fn read_user_bytes(token: usize, ptr: *const u8, len: usize) -> SysResult<Vec<u8>> {
    let mut out = Vec::with_capacity(len);
    if len == 0 {
        return Ok(out);
    }
    let parts = crate::mm::translated_byte_buffer(token, ptr, len)?;
    for part in parts {
        out.extend_from_slice(part);
    }
    Ok(out)
}

fn write_user_bytes(token: usize, ptr: *mut u8, src: &[u8]) -> SysResult<()> {
    if src.is_empty() {
        return Ok(());
    }
    let mut copied = 0usize;
    let parts = crate::mm::translated_byte_buffer_for_write(token, ptr, src.len())?;
    for part in parts {
        let n = part.len();
        part.copy_from_slice(&src[copied..copied + n]);
        copied += n;
    }
    Ok(())
}

fn get_epoll_file(epfd: usize) -> SysResult<Arc<dyn File + Send + Sync>> {
    let epoll = get_fd_file(epfd)?;
    if !epoll.is_epoll() {
        return Err(SysError::EINVAL);
    }
    Ok(epoll)
}

pub fn sys_epoll_create1(flags: i32) -> SyscallResult {
    if flags < 0 {
        return Err(SysError::EINVAL);
    }
    if flags & !EPOLL_CLOEXEC != 0 {
        return Err(SysError::EINVAL);
    }

    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    let fd = inner.alloc_fd()?;
    inner.fd_table[fd] = Some(Arc::new(EpollFile::new(0)));
    if flags & EPOLL_CLOEXEC != 0 && fd < inner.fd_flags.len() {
        inner.fd_flags[fd] |= 1;
    }
    Ok(fd)
}

pub fn sys_epoll_ctl(epfd: usize, op: i32, fd: usize, event_ptr: usize) -> SyscallResult {
    if !matches!(op, EPOLL_CTL_ADD | EPOLL_CTL_DEL | EPOLL_CTL_MOD) {
        return Err(SysError::EINVAL);
    }

    let epoll = get_epoll_file(epfd)?;
    let target = get_fd_file(fd)?;

    if !target.supports_epoll() {
        return Err(SysError::EPERM);
    }

    if target.is_epoll() {
        let epoll_id = epoll.epoll_id().ok_or(SysError::EINVAL)?;
        if target.epoll_id() == Some(epoll_id) {
            return Err(SysError::EINVAL);
        }
        if op == EPOLL_CTL_ADD && target.epoll_contains_id(epoll_id) {
            return Err(SysError::ELOOP);
        }
        if op == EPOLL_CTL_ADD && 1 + target.epoll_nesting_depth() >= EPOLL_CTL_MAX_NESTING {
            return Err(SysError::EINVAL);
        }
    }

    if op != EPOLL_CTL_DEL && event_ptr == 0 {
        return Err(SysError::EFAULT);
    }

    let event = if op == EPOLL_CTL_DEL {
        EpollEvent::default()
    } else {
        read_epoll_event(current_user_token(), event_ptr)?
    };

    match op {
        EPOLL_CTL_ADD => epoll.epoll_add(fd as i32, target.clone(), event.events, event.data),
        EPOLL_CTL_MOD => epoll.epoll_modify(fd as i32, event.events, event.data),
        EPOLL_CTL_DEL => epoll.epoll_delete(fd as i32),
        _ => Err(SysError::EINVAL),
    }
}

pub fn sys_epoll_pwait(
    epfd: usize,
    events_ptr: usize,
    maxevents: i32,
    timeout_ms: i32,
    sigmask: usize,
    sigsetsize: usize,
) -> SyscallResult {
    if maxevents <= 0 {
        return Err(SysError::EINVAL);
    }
    let maxevents = maxevents as usize;
    if maxevents > 1024 {
        return Err(SysError::EINVAL);
    }
    if events_ptr == 0 {
        return Err(SysError::EFAULT);
    }

    let epoll = get_epoll_file(epfd)?;
    let token = current_user_token();
    write_user_bytes(token, events_ptr as *mut u8, &[0])?;
    let mut mask_guard = if sigmask != 0 {
        if sigsetsize != core::mem::size_of::<u64>() {
            return Err(SysError::EINVAL);
        }
        let raw = read_user_bytes(token, sigmask as *const u8, core::mem::size_of::<u64>())?;
        let bits = u64::from_ne_bytes(raw.try_into().map_err(|_| SysError::EFAULT)?);
        Some(install_temporary_signal_mask(SignalSet::from_bits(bits))?)
    } else {
        None
    };
    let deadline = if timeout_ms < 0 {
        None
    } else {
        Some(current_time().as_micros() as i128 + timeout_ms as i128 * 1_000)
    };

    loop {
        let ready_pairs = epoll.epoll_ready_events(maxevents);
        let ready = ready_pairs
            .iter()
            .map(|(events, data)| EpollEvent {
                events: *events,
                data: *data,
            })
            .collect::<Vec<_>>();
        if !ready.is_empty() {
            let count = ready.len();
            write_epoll_events(token, events_ptr, &ready)?;
            return Ok(count);
        }

        if let Some(deadline) = deadline {
            if (current_time().as_micros() as i128) >= deadline {
                return Ok(0);
            }
        }

        let current = current_task().unwrap();
        epoll.register_poll_waker(current.clone());
        epoll.epoll_register_interest_wakers(current.clone());

        if pending_signal_interrupts_wait() {
            epoll.clear_poll_waker(&current);
            epoll.epoll_clear_interest_wakers(&current);
            if let Some(guard) = mask_guard.as_mut() {
                guard.defer_restore_until_sigreturn();
            }
            return Err(SysError::EINTR);
        }

        if deadline.is_some() {
            suspend_current_and_run_next();
        } else {
            block_current_and_run_next();
        }

        epoll.clear_poll_waker(&current);
        epoll.epoll_clear_interest_wakers(&current);

        if current_process().inner_exclusive_access().is_zombie {
            return Err(SysError::EINTR);
        }
        if pending_signal_interrupts_wait() {
            if let Some(guard) = mask_guard.as_mut() {
                guard.defer_restore_until_sigreturn();
            }
            return Err(SysError::EINTR);
        }
    }
}
