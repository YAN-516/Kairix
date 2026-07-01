#![allow(missing_docs)]

use crate::error::{SysError, SysResult};
use crate::fs::vfs::inode::InodeMode;
use crate::fs::vfs::{Dentry, DentryInner, File, FileInner, OpenFlags};
use crate::mm::UserBuffer;
use crate::task::{
    TaskControlBlock, block_current_and_run_next, current_process, current_task, wakeup_task,
};
use alloc::collections::{BTreeMap, VecDeque};
use alloc::format;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use spin::{Mutex, MutexGuard};

const IN_CLOEXEC: i32 = 0o2000000;
const IN_NONBLOCK: i32 = 0o0004000;
const O_NONBLOCK: u32 = 0o0004000;

pub const IN_ACCESS: u32 = 0x0000_0001;
pub const IN_MODIFY: u32 = 0x0000_0002;
pub const IN_ATTRIB: u32 = 0x0000_0004;
pub const IN_CLOSE_WRITE: u32 = 0x0000_0008;
pub const IN_CLOSE_NOWRITE: u32 = 0x0000_0010;
pub const IN_OPEN: u32 = 0x0000_0020;
pub const IN_MOVED_FROM: u32 = 0x0000_0040;
pub const IN_MOVED_TO: u32 = 0x0000_0080;
pub const IN_CREATE: u32 = 0x0000_0100;
pub const IN_DELETE: u32 = 0x0000_0200;
pub const IN_DELETE_SELF: u32 = 0x0000_0400;
pub const IN_MOVE_SELF: u32 = 0x0000_0800;
pub const IN_UNMOUNT: u32 = 0x0000_2000;
pub const IN_Q_OVERFLOW: u32 = 0x0000_4000;
pub const IN_IGNORED: u32 = 0x0000_8000;
pub const IN_ONLYDIR: u32 = 0x0100_0000;
pub const IN_DONT_FOLLOW: u32 = 0x0200_0000;
pub const IN_EXCL_UNLINK: u32 = 0x0400_0000;
pub const IN_MASK_CREATE: u32 = 0x1000_0000;
pub const IN_MASK_ADD: u32 = 0x2000_0000;
pub const IN_ISDIR: u32 = 0x4000_0000;
pub const IN_ONESHOT: u32 = 0x8000_0000;

const INOTIFY_EVENT_SIZE: usize = 16;
const INOTIFY_NAME_ALIGN: usize = 16;
const INOTIFY_EVENT_MASK: u32 = 0x0000_0fff | IN_UNMOUNT | IN_Q_OVERFLOW | IN_IGNORED;
const INOTIFY_WATCH_FLAGS: u32 = IN_EXCL_UNLINK | IN_ONESHOT;
const INOTIFY_ADD_FLAGS: u32 =
    IN_ONLYDIR | IN_DONT_FOLLOW | IN_EXCL_UNLINK | IN_MASK_CREATE | IN_MASK_ADD | IN_ONESHOT;
const INOTIFY_DEFAULT_MAX_USER_INSTANCES: usize = 128;
const INOTIFY_DEFAULT_MAX_USER_WATCHES: usize = 8192;
const INOTIFY_DEFAULT_MAX_QUEUED_EVENTS: usize = 16_384;

static INOTIFY_MAX_USER_INSTANCES: AtomicUsize =
    AtomicUsize::new(INOTIFY_DEFAULT_MAX_USER_INSTANCES);
static INOTIFY_MAX_USER_WATCHES: AtomicUsize = AtomicUsize::new(INOTIFY_DEFAULT_MAX_USER_WATCHES);
static INOTIFY_MAX_QUEUED_EVENTS: AtomicUsize = AtomicUsize::new(INOTIFY_DEFAULT_MAX_QUEUED_EVENTS);

pub struct InotifyFile {
    inner: Mutex<FileInner>,
    status_flags: Mutex<u32>,
    state: Arc<Mutex<InotifyState>>,
}

struct InotifyWatch {
    wd: i32,
    path: String,
    ino: Option<usize>,
    aliases: Vec<String>,
    mask: u32,
    unlinked_children: Vec<String>,
}

struct InotifyEvent {
    wd: i32,
    mask: u32,
    cookie: u32,
    name: Vec<u8>,
}

struct InotifyState {
    next_wd: i32,
    watches: BTreeMap<i32, InotifyWatch>,
    events: VecDeque<InotifyEvent>,
    read_waiters: VecDeque<Weak<TaskControlBlock>>,
    poll_waiters: VecDeque<Weak<TaskControlBlock>>,
    overflowed: bool,
}

impl InotifyFile {
    fn new(dentry: Arc<dyn Dentry>, status_flags: u32) -> Self {
        Self {
            inner: Mutex::new(FileInner {
                offset: 0,
                dentry,
                flags: OpenFlags::empty(),
            }),
            status_flags: Mutex::new(status_flags),
            state: Arc::new(Mutex::new(InotifyState {
                next_wd: 1,
                watches: BTreeMap::new(),
                events: VecDeque::new(),
                read_waiters: VecDeque::new(),
                poll_waiters: VecDeque::new(),
                overflowed: false,
            })),
        }
    }

    fn add_watch(&self, path: String, ino: Option<usize>, mask: u32) -> SysResult<i32> {
        let mut state = self.state.lock();
        if let Some(watch) = state.watches.values_mut().find(|watch| {
            watch.ino.is_some() && watch.ino == ino || watch.ino.is_none() && watch.path == path
        }) {
            if mask & IN_MASK_CREATE != 0 {
                return Err(SysError::EEXIST);
            }
            if mask & IN_MASK_ADD != 0 {
                watch.mask |= sanitize_watch_mask(mask);
            } else {
                watch.mask = sanitize_watch_mask(mask);
                watch.path = path;
                watch.ino = ino;
                watch.unlinked_children.clear();
            }
            return Ok(watch.wd);
        }
        if live_inotify_watch_count_locked(&state) >= inotify_max_user_watches() {
            return Err(SysError::ENOSPC);
        }
        let wd = state.next_wd;
        state.next_wd += 1;
        state.watches.insert(wd, InotifyWatch {
            wd,
            path,
            ino,
            aliases: Vec::new(),
            mask: sanitize_watch_mask(mask),
            unlinked_children: Vec::new(),
        });
        Ok(wd)
    }

    fn remove_watch(&self, wd: i32) -> SysResult<()> {
        let mut state = self.state.lock();
        if state.watches.remove(&wd).is_some() {
            state.events.push_back(InotifyEvent {
                wd,
                mask: IN_IGNORED,
                cookie: 0,
                name: Vec::new(),
            });
            Ok(())
        } else {
            Err(SysError::EINVAL)
        }
    }

    fn queue_matching_event(
        &self,
        path: &str,
        ino: Option<usize>,
        parent_ino: Option<usize>,
        mask: u32,
    ) {
        let mut state = self.state.lock();
        let mut events = Vec::new();
        let mut ignored_wds = Vec::new();
        for watch in state.watches.values_mut() {
            let mut queued = false;
            if watch.matches_target(path, ino) {
                if mask_matches(watch.mask, mask) {
                    events.push(InotifyEvent {
                        wd: watch.wd,
                        mask,
                        cookie: 0,
                        name: Vec::new(),
                    });
                    queued = true;
                }
            } else if let Some((parent, name)) = parent_and_name(path) {
                if watch.matches_child_parent(parent, parent_ino) {
                    if mask & IN_CREATE != 0 {
                        watch.forget_unlinked_child(path);
                    }
                    if watch.excludes_unlinked_child(path) {
                        continue;
                    }
                    if mask_matches(watch.mask, mask) {
                        events.push(InotifyEvent {
                            wd: watch.wd,
                            mask,
                            cookie: 0,
                            name: event_name(name),
                        });
                        queued = true;
                    }
                }
            }
            if queued && watch.oneshot() {
                ignored_wds.push(watch.wd);
            }
        }
        for wd in ignored_wds {
            if state.watches.remove(&wd).is_some() {
                events.push(InotifyEvent {
                    wd,
                    mask: IN_IGNORED,
                    cookie: 0,
                    name: Vec::new(),
                });
            }
        }
        if push_events(&mut state, events) {
            wake_waiters(&mut state);
        }
    }

    fn queue_delete_event(
        &self,
        path: &str,
        ino: Option<usize>,
        parent_ino: Option<usize>,
        is_dir: bool,
        removed: bool,
    ) {
        let mut state = self.state.lock();
        let child_mask = IN_DELETE | if is_dir { IN_ISDIR } else { 0 };
        let mut events = Vec::new();
        let mut ignored_wds = Vec::new();
        for watch in state.watches.values_mut() {
            let mut queued = false;
            if let Some((parent, name)) = parent_and_name(path) {
                if watch.matches_child_parent(parent, parent_ino) {
                    watch.note_unlinked_child(path);
                    if mask_matches(watch.mask, child_mask) {
                        events.push(InotifyEvent {
                            wd: watch.wd,
                            mask: child_mask,
                            cookie: 0,
                            name: event_name(name),
                        });
                        queued = true;
                    }
                }
            }
            if watch.matches_target(path, ino) {
                if !is_dir && mask_matches(watch.mask, IN_ATTRIB) {
                    events.push(InotifyEvent {
                        wd: watch.wd,
                        mask: IN_ATTRIB,
                        cookie: 0,
                        name: Vec::new(),
                    });
                    queued = true;
                }
                if removed {
                    if mask_matches(watch.mask, IN_DELETE_SELF) {
                        events.push(InotifyEvent {
                            wd: watch.wd,
                            mask: IN_DELETE_SELF,
                            cookie: 0,
                            name: Vec::new(),
                        });
                        queued = true;
                    }
                    events.push(InotifyEvent {
                        wd: watch.wd,
                        mask: IN_IGNORED,
                        cookie: 0,
                        name: Vec::new(),
                    });
                    ignored_wds.push(watch.wd);
                }
            }
            if queued && watch.oneshot() {
                ignored_wds.push(watch.wd);
            }
        }
        for wd in ignored_wds {
            state.watches.remove(&wd);
        }
        if push_events(&mut state, events) {
            wake_waiters(&mut state);
        }
    }

    fn queue_move_event(
        &self,
        old_path: &str,
        new_path: &str,
        ino: Option<usize>,
        old_parent_ino: Option<usize>,
        new_parent_ino: Option<usize>,
        is_dir: bool,
        cookie: u32,
    ) {
        let mut state = self.state.lock();
        let moved_from = IN_MOVED_FROM | if is_dir { IN_ISDIR } else { 0 };
        let moved_to = IN_MOVED_TO | if is_dir { IN_ISDIR } else { 0 };
        let mut events = Vec::new();
        let mut ignored_wds = Vec::new();
        for watch in state.watches.values_mut() {
            let watches_old = watch.matches_target(old_path, ino);
            let watches_new = watch.matches_target(new_path, ino);
            let mut queued = false;

            if let Some((old_parent, old_name)) = parent_and_name(old_path) {
                if watch.matches_child_parent(old_parent, old_parent_ino)
                    && mask_matches(watch.mask, moved_from)
                {
                    events.push(InotifyEvent {
                        wd: watch.wd,
                        mask: moved_from,
                        cookie,
                        name: event_name(old_name),
                    });
                    queued = true;
                }
            }
            if let Some((new_parent, new_name)) = parent_and_name(new_path) {
                if watch.matches_child_parent(new_parent, new_parent_ino)
                    && mask_matches(watch.mask, moved_to)
                {
                    events.push(InotifyEvent {
                        wd: watch.wd,
                        mask: moved_to,
                        cookie,
                        name: event_name(new_name),
                    });
                    queued = true;
                }
            }

            if (watches_old || watches_new) && mask_matches(watch.mask, IN_MOVE_SELF) {
                events.push(InotifyEvent {
                    wd: watch.wd,
                    mask: IN_MOVE_SELF,
                    cookie: 0,
                    name: Vec::new(),
                });
                queued = true;
            }

            if watches_old {
                watch.add_alias(new_path);
            }
            if watches_new {
                watch.add_alias(old_path);
            }
            if queued && watch.oneshot() {
                ignored_wds.push(watch.wd);
            }
        }
        for wd in ignored_wds {
            if state.watches.remove(&wd).is_some() {
                events.push(InotifyEvent {
                    wd,
                    mask: IN_IGNORED,
                    cookie: 0,
                    name: Vec::new(),
                });
            }
        }
        if push_events(&mut state, events) {
            wake_waiters(&mut state);
        }
    }

    fn queue_unmount_events(&self, mount_path: &str) {
        let mut state = self.state.lock();
        let affected: Vec<i32> = state
            .watches
            .values()
            .filter(|watch| watch.is_under_mount(mount_path))
            .map(|watch| watch.wd)
            .collect();

        let mut events = Vec::new();
        for wd in affected {
            if state.watches.remove(&wd).is_some() {
                events.push(InotifyEvent {
                    wd,
                    mask: IN_UNMOUNT,
                    cookie: 0,
                    name: Vec::new(),
                });
                events.push(InotifyEvent {
                    wd,
                    mask: IN_IGNORED,
                    cookie: 0,
                    name: Vec::new(),
                });
            }
        }
        if push_events(&mut state, events) {
            wake_waiters(&mut state);
        }
    }

    fn has_events(&self) -> bool {
        !self.state.lock().events.is_empty()
    }

    fn fdinfo(&self) -> String {
        let state = self.state.lock();
        let mut info = String::new();
        for watch in state.watches.values() {
            info.push_str(&format!(
                "inotify wd:{} ino:0 sdev:0 mask:{:x}\n",
                watch.wd, watch.mask
            ));
        }
        info
    }
}

impl InotifyWatch {
    fn matches_path(&self, path: &str) -> bool {
        self.path == path || self.aliases.iter().any(|alias| alias == path)
    }

    fn matches_target(&self, path: &str, ino: Option<usize>) -> bool {
        self.ino.is_some() && self.ino == ino || self.matches_path(path)
    }

    fn matches_child_parent(&self, parent_path: &str, parent_ino: Option<usize>) -> bool {
        self.ino.is_some() && self.ino == parent_ino || self.matches_path(parent_path)
    }

    fn add_alias(&mut self, path: &str) {
        if self.path != path && !self.aliases.iter().any(|alias| alias == path) {
            self.aliases.push(String::from(path));
        }
    }

    fn oneshot(&self) -> bool {
        self.mask & IN_ONESHOT != 0
    }

    fn note_unlinked_child(&mut self, path: &str) {
        if self.mask & IN_EXCL_UNLINK != 0
            && !self
                .unlinked_children
                .iter()
                .any(|unlinked| unlinked == path)
        {
            self.unlinked_children.push(String::from(path));
        }
    }

    fn forget_unlinked_child(&mut self, path: &str) {
        self.unlinked_children.retain(|unlinked| unlinked != path);
    }

    fn excludes_unlinked_child(&self, path: &str) -> bool {
        self.mask & IN_EXCL_UNLINK != 0
            && self
                .unlinked_children
                .iter()
                .any(|unlinked| unlinked == path)
    }

    fn is_under_mount(&self, mount_path: &str) -> bool {
        path_is_at_or_below_mount(&self.path, mount_path)
            || self
                .aliases
                .iter()
                .any(|alias| path_is_at_or_below_mount(alias, mount_path))
    }
}

fn mask_matches(watch_mask: u32, event_mask: u32) -> bool {
    watch_mask & (event_mask & INOTIFY_EVENT_MASK) != 0
}

fn sanitize_watch_mask(mask: u32) -> u32 {
    mask & (INOTIFY_EVENT_MASK | INOTIFY_WATCH_FLAGS)
}

fn validate_watch_mask(mask: u32) -> SysResult<()> {
    if mask & !(INOTIFY_EVENT_MASK | INOTIFY_ADD_FLAGS) != 0 {
        return Err(SysError::EINVAL);
    }
    if mask & (IN_MASK_ADD | IN_MASK_CREATE) == (IN_MASK_ADD | IN_MASK_CREATE) {
        return Err(SysError::EINVAL);
    }
    if mask & INOTIFY_EVENT_MASK == 0 {
        return Err(SysError::EINVAL);
    }
    Ok(())
}

fn parent_and_name(path: &str) -> Option<(&str, &str)> {
    let path = path.trim_end_matches('/');
    if path.is_empty() || path == "/" {
        return None;
    }
    match path.rfind('/') {
        Some(0) => Some(("/", &path[1..])),
        Some(idx) => Some((&path[..idx], &path[idx + 1..])),
        None => Some((".", path)),
    }
}

fn parent_path(path: &str) -> Option<&str> {
    parent_and_name(path).map(|(parent, _)| parent)
}

fn live_inotify_watch_count_locked(state: &InotifyState) -> usize {
    state.watches.len()
}

fn path_is_at_or_below_mount(path: &str, mount_path: &str) -> bool {
    let mount_path = mount_path.trim_end_matches('/');
    if mount_path == "/" {
        return path.starts_with('/');
    }
    path == mount_path
        || path
            .strip_prefix(mount_path)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn event_name(name: &str) -> Vec<u8> {
    let mut bytes = Vec::from(name.as_bytes());
    bytes.push(0);
    while bytes.len() % INOTIFY_NAME_ALIGN != 0 {
        bytes.push(0);
    }
    bytes
}

fn register_waiter(waiters: &mut VecDeque<Weak<TaskControlBlock>>, task: Arc<TaskControlBlock>) {
    let mut queued = false;
    waiters.retain(|waiter| {
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

fn wake_waiter_queue(waiters: &mut VecDeque<Weak<TaskControlBlock>>) {
    while let Some(waiter) = waiters.pop_front() {
        if let Some(task) = waiter.upgrade() {
            wakeup_task(task);
        }
    }
}

fn wake_waiters(state: &mut InotifyState) {
    wake_waiter_queue(&mut state.read_waiters);
    wake_waiter_queue(&mut state.poll_waiters);
}

fn push_events(state: &mut InotifyState, events: Vec<InotifyEvent>) -> bool {
    let mut pushed = false;
    for event in events {
        if state.overflowed {
            continue;
        }
        if state.events.back().is_some_and(|last| {
            last.wd == event.wd
                && last.mask == event.mask
                && last.cookie == event.cookie
                && last.name == event.name
        }) {
            continue;
        }
        if state.events.len() >= inotify_max_queued_events() {
            if let Some(last) = state.events.back_mut() {
                *last = InotifyEvent {
                    wd: -1,
                    mask: IN_Q_OVERFLOW,
                    cookie: 0,
                    name: Vec::new(),
                };
            } else {
                state.events.push_back(InotifyEvent {
                    wd: -1,
                    mask: IN_Q_OVERFLOW,
                    cookie: 0,
                    name: Vec::new(),
                });
            }
            pushed = true;
            state.overflowed = true;
            continue;
        }
        state.events.push_back(event);
        pushed = true;
    }
    pushed
}

impl File for InotifyFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        self.inner.lock()
    }

    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        false
    }

    fn read(&self, mut buf: UserBuffer) -> SysResult<usize> {
        let buf_len = buf.len();
        if buf_len == 0 {
            return Ok(0);
        }

        let out = loop {
            let mut state = self.state.lock();
            let Some(front) = state.events.front() else {
                if *self.status_flags.lock() & O_NONBLOCK != 0 {
                    return Err(SysError::EAGAIN);
                }
                let task = current_task().unwrap();
                register_waiter(&mut state.read_waiters, task);
                drop(state);
                block_current_and_run_next();
                if current_process().inner_exclusive_access().is_zombie
                    || crate::syscall::signal::should_interrupt_syscall()
                {
                    return Err(SysError::EINTR);
                }
                continue;
            };
            let first_len = INOTIFY_EVENT_SIZE + front.name.len();
            if buf_len < first_len {
                return Err(SysError::EINVAL);
            }

            let mut out = Vec::new();
            while let Some(event) = state.events.front() {
                let event_len = INOTIFY_EVENT_SIZE + event.name.len();
                if out.len() + event_len > buf_len {
                    break;
                }
                let event = state.events.pop_front().unwrap();
                out.extend_from_slice(&event.wd.to_ne_bytes());
                out.extend_from_slice(&event.mask.to_ne_bytes());
                out.extend_from_slice(&event.cookie.to_ne_bytes());
                out.extend_from_slice(&(event.name.len() as u32).to_ne_bytes());
                out.extend_from_slice(&event.name);
                if event.mask == IN_Q_OVERFLOW {
                    state.overflowed = false;
                }
            }
            break out;
        };

        let mut written = 0;
        for slice in buf.buffers.iter_mut() {
            if written >= out.len() {
                break;
            }
            let copy_len = slice.len().min(out.len() - written);
            slice[..copy_len].copy_from_slice(&out[written..written + copy_len]);
            written += copy_len;
        }
        Ok(written)
    }

    fn write(&self, _buf: UserBuffer) -> SysResult<usize> {
        Err(SysError::EINVAL)
    }

    fn get_inode(&self) -> Option<Arc<dyn crate::fs::vfs::Inode>> {
        None
    }

    fn supports_epoll(&self) -> bool {
        true
    }

    fn status_flags(&self) -> u32 {
        *self.status_flags.lock()
    }

    fn set_status_flags(&self, flags: u32) {
        let mut status_flags = self.status_flags.lock();
        *status_flags = (*status_flags & !O_NONBLOCK) | (flags & O_NONBLOCK);
    }

    fn read_ready(&self) -> Option<bool> {
        Some(self.has_events())
    }

    fn register_poll_waker(&self, task: Arc<TaskControlBlock>) {
        let mut state = self.state.lock();
        register_waiter(&mut state.poll_waiters, task);
    }

    fn clear_poll_waker(&self, task: &Arc<TaskControlBlock>) {
        let mut state = self.state.lock();
        clear_waiter(&mut state.poll_waiters, task);
    }

    fn wake_poll_waiters(&self) {
        let mut state = self.state.lock();
        wake_waiter_queue(&mut state.poll_waiters);
    }
}

static INOTIFY_INSTANCES: Mutex<Vec<Weak<InotifyFile>>> = Mutex::new(Vec::new());
static INOTIFY_INSTANCE_HINT: AtomicUsize = AtomicUsize::new(0);
static NEXT_COOKIE: AtomicU32 = AtomicU32::new(1);

#[inline]
pub fn inotify_may_have_instances() -> bool {
    INOTIFY_INSTANCE_HINT.load(Ordering::Relaxed) != 0
}

pub fn inotify_max_user_instances() -> usize {
    INOTIFY_MAX_USER_INSTANCES.load(Ordering::Relaxed)
}

pub fn inotify_set_max_user_instances(value: usize) {
    INOTIFY_MAX_USER_INSTANCES.store(value, Ordering::Relaxed);
}

pub fn inotify_max_user_watches() -> usize {
    INOTIFY_MAX_USER_WATCHES.load(Ordering::Relaxed)
}

pub fn inotify_set_max_user_watches(value: usize) {
    INOTIFY_MAX_USER_WATCHES.store(value, Ordering::Relaxed);
}

pub fn inotify_max_queued_events() -> usize {
    INOTIFY_MAX_QUEUED_EVENTS.load(Ordering::Relaxed)
}

pub fn inotify_set_max_queued_events(value: usize) {
    INOTIFY_MAX_QUEUED_EVENTS.store(value, Ordering::Relaxed);
}

pub struct InotifyDentry {
    inner: DentryInner,
}

impl InotifyDentry {
    fn new(name: &str) -> Self {
        Self {
            inner: DentryInner::new(name, None),
        }
    }
}

impl Dentry for InotifyDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }

    fn open(self: Arc<Self>, flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Ok(Arc::new(InotifyFile::new(self, flags.bits() & O_NONBLOCK)))
    }
}

pub fn create_inotify_file(flags: i32) -> SysResult<Arc<InotifyFile>> {
    if flags & !(IN_CLOEXEC | IN_NONBLOCK) != 0 {
        return Err(SysError::EINVAL);
    }
    let status_flags = if flags & IN_NONBLOCK != 0 {
        O_NONBLOCK
    } else {
        0
    };
    let dentry = Arc::new(InotifyDentry::new("inotify"));
    Ok(Arc::new(InotifyFile::new(dentry, status_flags)))
}

pub fn register_inotify_file(file: &Arc<InotifyFile>) -> SysResult<()> {
    let mut instances = INOTIFY_INSTANCES.lock();
    instances.retain(|weak| weak.strong_count() > 0);
    if instances.len() >= inotify_max_user_instances() {
        return Err(SysError::EMFILE);
    }
    instances.push(Arc::downgrade(file));
    INOTIFY_INSTANCE_HINT.store(1, Ordering::Relaxed);
    Ok(())
}

pub fn inotify_init_cloexec(flags: i32) -> bool {
    flags & IN_CLOEXEC != 0
}

pub fn inotify_add_watch(
    file: Arc<InotifyFile>,
    dentry: Arc<dyn Dentry>,
    mask: u32,
) -> SysResult<i32> {
    validate_watch_mask(mask)?;
    let inode = dentry.get_inode().ok_or(SysError::ENOENT)?;
    if mask & IN_ONLYDIR != 0 && !inode.get_mode().contains(InodeMode::DIR) {
        return Err(SysError::ENOTDIR);
    }
    file.add_watch(dentry.path(), Some(inode.get_ino()), mask)
}

pub fn inotify_remove_watch(file: Arc<InotifyFile>, wd: i32) -> SysResult<()> {
    file.remove_watch(wd)
}

pub fn inotify_file_from_file(file: &Arc<dyn File + Send + Sync>) -> Option<Arc<InotifyFile>> {
    let target = Arc::as_ptr(file) as *const ();
    let mut instances = INOTIFY_INSTANCES.lock();
    let mut found = None;
    instances.retain(|weak| {
        if let Some(inotify_file) = weak.upgrade() {
            if Arc::as_ptr(&inotify_file) as *const () == target {
                found = Some(inotify_file);
            }
            true
        } else {
            false
        }
    });
    found
}

pub fn inotify_fdinfo(file: &Arc<dyn File + Send + Sync>) -> Option<String> {
    inotify_file_from_file(file).map(|file| file.fdinfo())
}

pub fn inotify_notify_path(path: &str, mask: u32) {
    if !inotify_may_have_instances() {
        return;
    }
    let mut instances = INOTIFY_INSTANCES.lock();
    instances.retain(|weak| {
        if let Some(inotify_file) = weak.upgrade() {
            inotify_file.queue_matching_event(path, None, None, mask);
            true
        } else {
            false
        }
    });
}

pub fn inotify_notify_dentry(dentry: Arc<dyn Dentry>, mask: u32) {
    if !inotify_may_have_instances() {
        return;
    }
    let path = dentry.path();
    let ino = dentry.get_inode().map(|inode| inode.get_ino());
    let parent_ino = dentry
        .parent()
        .and_then(|parent| parent.get_inode())
        .map(|inode| inode.get_ino());
    let mut instances = INOTIFY_INSTANCES.lock();
    instances.retain(|weak| {
        if let Some(inotify_file) = weak.upgrade() {
            inotify_file.queue_matching_event(&path, ino, parent_ino, mask);
            true
        } else {
            false
        }
    });
}

pub fn inotify_notify_delete(path: &str, is_dir: bool, removed: bool) {
    let mut instances = INOTIFY_INSTANCES.lock();
    instances.retain(|weak| {
        if let Some(inotify_file) = weak.upgrade() {
            inotify_file.queue_delete_event(path, None, None, is_dir, removed);
            true
        } else {
            false
        }
    });
}

pub fn inotify_notify_delete_dentry(dentry: Arc<dyn Dentry>, removed: bool) {
    let path = dentry.path();
    let ino = dentry.get_inode().map(|inode| inode.get_ino());
    let parent_ino = dentry
        .parent()
        .and_then(|parent| parent.get_inode())
        .map(|inode| inode.get_ino());
    let is_dir = dentry
        .get_inode()
        .is_some_and(|inode| inode.get_mode().contains(InodeMode::DIR));
    let mut instances = INOTIFY_INSTANCES.lock();
    instances.retain(|weak| {
        if let Some(inotify_file) = weak.upgrade() {
            inotify_file.queue_delete_event(&path, ino, parent_ino, is_dir, removed);
            true
        } else {
            false
        }
    });
}

pub fn inotify_notify_move(old_path: &str, new_path: &str, is_dir: bool) {
    let cookie = NEXT_COOKIE.fetch_add(1, Ordering::Relaxed);
    let cookie = if cookie == 0 { 1 } else { cookie };
    let mut instances = INOTIFY_INSTANCES.lock();
    instances.retain(|weak| {
        if let Some(inotify_file) = weak.upgrade() {
            inotify_file.queue_move_event(old_path, new_path, None, None, None, is_dir, cookie);
            true
        } else {
            false
        }
    });
}

pub fn inotify_notify_move_dentry(
    old_path: &str,
    new_path: &str,
    target: Option<Arc<dyn Dentry>>,
    is_dir: bool,
) {
    let cookie = NEXT_COOKIE.fetch_add(1, Ordering::Relaxed);
    let cookie = if cookie == 0 { 1 } else { cookie };
    let ino = target
        .as_ref()
        .and_then(|dentry| dentry.get_inode())
        .map(|inode| inode.get_ino());
    let old_parent_ino = parent_path(old_path)
        .and_then(|parent| crate::fs::vfs::file::find_dentry(parent).ok())
        .and_then(|parent| parent.get_inode())
        .map(|inode| inode.get_ino());
    let new_parent_ino = parent_path(new_path)
        .and_then(|parent| crate::fs::vfs::file::find_dentry(parent).ok())
        .and_then(|parent| parent.get_inode())
        .map(|inode| inode.get_ino());
    let mut instances = INOTIFY_INSTANCES.lock();
    instances.retain(|weak| {
        if let Some(inotify_file) = weak.upgrade() {
            inotify_file.queue_move_event(
                old_path,
                new_path,
                ino,
                old_parent_ino,
                new_parent_ino,
                is_dir,
                cookie,
            );
            true
        } else {
            false
        }
    });
}

pub fn inotify_notify_unmount(mount_path: &str) {
    let mut instances = INOTIFY_INSTANCES.lock();
    instances.retain(|weak| {
        if let Some(inotify_file) = weak.upgrade() {
            inotify_file.queue_unmount_events(mount_path);
            true
        } else {
            false
        }
    });
}
