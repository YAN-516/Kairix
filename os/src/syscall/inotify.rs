#![allow(missing_docs)]

use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::vfs::path::resolve_path;
use crate::fs::vfs::inode::InodeMode;
use crate::fs::vfs::{Dentry, DentryInner, File, FileInner, OpenFlags};
use crate::mm::{translated_str, UserBuffer};
use crate::task::{current_process, current_user_token};
use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
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
pub const IN_IGNORED: u32 = 0x0000_8000;
pub const IN_ISDIR: u32 = 0x4000_0000;

const INOTIFY_EVENT_SIZE: usize = 16;
const INOTIFY_NAME_ALIGN: usize = 16;
const INOTIFY_EVENT_MASK: u32 = 0x0000_0fff | IN_UNMOUNT | IN_IGNORED;

pub struct InotifyFile {
    inner: Mutex<FileInner>,
    status_flags: Mutex<u32>,
    state: Arc<Mutex<InotifyState>>,
}

struct InotifyWatch {
    wd: i32,
    path: String,
    aliases: Vec<String>,
    mask: u32,
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
}

impl InotifyFile {
    fn new(dentry: Arc<dyn Dentry>, status_flags: u32) -> Self {
        Self {
            inner: Mutex::new(FileInner { offset: 0, dentry }),
            status_flags: Mutex::new(status_flags),
            state: Arc::new(Mutex::new(InotifyState {
                next_wd: 1,
                watches: BTreeMap::new(),
                events: VecDeque::new(),
            })),
        }
    }

    fn add_watch(&self, path: String, mask: u32) -> i32 {
        let mut state = self.state.lock();
        if let Some(watch) = state.watches.values_mut().find(|watch| watch.path == path) {
            watch.mask = mask;
            return watch.wd;
        }
        let wd = state.next_wd;
        state.next_wd += 1;
        state.watches.insert(
            wd,
            InotifyWatch {
                wd,
                path,
                aliases: Vec::new(),
                mask,
            },
        );
        wd
    }

    fn queue_matching_event(&self, path: &str, mask: u32) {
        let mut state = self.state.lock();
        let mut events = Vec::new();
        for watch in state.watches.values() {
            if !mask_matches(watch.mask, mask) {
                continue;
            }
            if watch.matches_path(path) {
                events.push(InotifyEvent {
                    wd: watch.wd,
                    mask,
                    cookie: 0,
                    name: Vec::new(),
                });
            } else if let Some((parent, name)) = parent_and_name(path) {
                if watch.matches_path(parent) {
                    events.push(InotifyEvent {
                        wd: watch.wd,
                        mask,
                        cookie: 0,
                        name: event_name(name),
                    });
                }
            }
        }
        push_events(&mut state, events);
    }

    fn queue_delete_event(&self, path: &str, is_dir: bool) {
        let mut state = self.state.lock();
        let child_mask = IN_DELETE | if is_dir { IN_ISDIR } else { 0 };
        let self_mask = IN_DELETE_SELF;
        let mut events = Vec::new();
        for watch in state.watches.values() {
            if let Some((parent, name)) = parent_and_name(path) {
                if watch.matches_path(parent) && mask_matches(watch.mask, child_mask) {
                    events.push(InotifyEvent {
                        wd: watch.wd,
                        mask: child_mask,
                        cookie: 0,
                        name: event_name(name),
                    });
                }
            }
            if watch.matches_path(path) && mask_matches(watch.mask, self_mask) {
                events.push(InotifyEvent {
                    wd: watch.wd,
                    mask: self_mask,
                    cookie: 0,
                    name: Vec::new(),
                });
            }
        }
        push_events(&mut state, events);
    }

    fn queue_move_event(&self, old_path: &str, new_path: &str, is_dir: bool, cookie: u32) {
        let mut state = self.state.lock();
        let moved_from = IN_MOVED_FROM | if is_dir { IN_ISDIR } else { 0 };
        let moved_to = IN_MOVED_TO | if is_dir { IN_ISDIR } else { 0 };
        let mut events = Vec::new();
        for watch in state.watches.values_mut() {
            let watches_old = watch.matches_path(old_path);
            let watches_new = watch.matches_path(new_path);

            if let Some((old_parent, old_name)) = parent_and_name(old_path) {
                if watch.matches_path(old_parent) && mask_matches(watch.mask, moved_from) {
                    events.push(InotifyEvent {
                        wd: watch.wd,
                        mask: moved_from,
                        cookie,
                        name: event_name(old_name),
                    });
                }
            }
            if let Some((new_parent, new_name)) = parent_and_name(new_path) {
                if watch.matches_path(new_parent) && mask_matches(watch.mask, moved_to) {
                    events.push(InotifyEvent {
                        wd: watch.wd,
                        mask: moved_to,
                        cookie,
                        name: event_name(new_name),
                    });
                }
            }

            if (watches_old || watches_new) && mask_matches(watch.mask, IN_MOVE_SELF) {
                events.push(InotifyEvent {
                    wd: watch.wd,
                    mask: IN_MOVE_SELF,
                    cookie: 0,
                    name: Vec::new(),
                });
            }

            if watches_old {
                watch.add_alias(new_path);
            }
            if watches_new {
                watch.add_alias(old_path);
            }
        }
        push_events(&mut state, events);
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
        push_events(&mut state, events);
    }
}

impl InotifyWatch {
    fn matches_path(&self, path: &str) -> bool {
        self.path == path || self.aliases.iter().any(|alias| alias == path)
    }

    fn add_alias(&mut self, path: &str) {
        if self.path != path && !self.aliases.iter().any(|alias| alias == path) {
            self.aliases.push(String::from(path));
        }
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

fn push_events(state: &mut InotifyState, events: Vec<InotifyEvent>) {
    for event in events {
        if state.events.back().is_some_and(|last| {
            last.wd == event.wd
                && last.mask == event.mask
                && last.cookie == event.cookie
                && last.name == event.name
        }) {
            continue;
        }
        state.events.push_back(event);
    }
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

    fn read(&self, buf: UserBuffer) -> SysResult<usize> {
        if buf.len() == 0 {
            return Ok(0);
        }

        let mut state = self.state.lock();
        let Some(front) = state.events.front() else {
            return Err(SysError::EAGAIN);
        };
        let first_len = INOTIFY_EVENT_SIZE + front.name.len();
        if buf.len() < first_len {
            return Err(SysError::EINVAL);
        }

        let mut out = Vec::new();
        while let Some(event) = state.events.front() {
            let event_len = INOTIFY_EVENT_SIZE + event.name.len();
            if out.len() + event_len > buf.len() {
                break;
            }
            let event = state.events.pop_front().unwrap();
            out.extend_from_slice(&event.wd.to_ne_bytes());
            out.extend_from_slice(&event.mask.to_ne_bytes());
            out.extend_from_slice(&event.cookie.to_ne_bytes());
            out.extend_from_slice(&(event.name.len() as u32).to_ne_bytes());
            out.extend_from_slice(&event.name);
        }

        let mut written = 0;
        for slice in buf.buffers {
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

    fn status_flags(&self) -> u32 {
        *self.status_flags.lock()
    }

    fn set_status_flags(&self, flags: u32) {
        let mut status_flags = self.status_flags.lock();
        *status_flags = (*status_flags & !O_NONBLOCK) | (flags & O_NONBLOCK);
    }
}

static INOTIFY_INSTANCES: Mutex<Vec<Weak<InotifyFile>>> = Mutex::new(Vec::new());
static NEXT_COOKIE: AtomicU32 = AtomicU32::new(1);

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

pub fn sys_inotify_init1(flags: i32) -> SyscallResult {
    if flags & !(IN_CLOEXEC | IN_NONBLOCK) != 0 {
        return Err(SysError::EINVAL);
    }

    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    let fd = inner.alloc_fd()?;
    let status_flags = if flags & IN_NONBLOCK != 0 {
        O_NONBLOCK
    } else {
        0
    };
    let dentry = Arc::new(InotifyDentry::new("inotify"));
    let file = Arc::new(InotifyFile::new(dentry, status_flags));
    inner.fd_table[fd] = Some(file.clone());
    if flags & IN_CLOEXEC != 0 {
        inner.fd_flags[fd] |= 1;
    }
    INOTIFY_INSTANCES.lock().push(Arc::downgrade(&file));
    Ok(fd)
}

pub fn sys_inotify_add_watch(fd: usize, path: *const u8, mask: u32) -> SyscallResult {
    if path.is_null() {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let raw_path = translated_str(token, path)?;
    let cwd = current_process().inner_exclusive_access().cwd.clone();
    let dentry = resolve_path(cwd, &raw_path)?;
    let watch_path = dentry.path();

    let process = current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return Err(SysError::EBADF);
    }
    let Some(file) = &inner.fd_table[fd] else {
        return Err(SysError::EBADF);
    };
    let Some(inotify_file) = find_inotify_file(file) else {
        return Err(SysError::EINVAL);
    };
    Ok(inotify_file.add_watch(watch_path, mask) as usize)
}

pub fn sys_inotify_rm_watch(fd: usize, wd: i32) -> SyscallResult {
    let process = current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return Err(SysError::EBADF);
    }
    let Some(file) = &inner.fd_table[fd] else {
        return Err(SysError::EBADF);
    };
    let Some(inotify_file) = find_inotify_file(file) else {
        return Err(SysError::EINVAL);
    };
    let mut state = inotify_file.state.lock();
    if state.watches.remove(&wd).is_some() {
        state.events.push_back(InotifyEvent {
            wd,
            mask: IN_IGNORED,
            cookie: 0,
            name: Vec::new(),
        });
        Ok(0)
    } else {
        Err(SysError::EINVAL)
    }
}

fn find_inotify_file(file: &Arc<dyn File + Send + Sync>) -> Option<Arc<InotifyFile>> {
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

pub fn inotify_notify_path(path: &str, mask: u32) {
    let mut instances = INOTIFY_INSTANCES.lock();
    instances.retain(|weak| {
        if let Some(inotify_file) = weak.upgrade() {
            inotify_file.queue_matching_event(path, mask);
            true
        } else {
            false
        }
    });
}

pub fn inotify_notify_delete(path: &str, is_dir: bool) {
    let mut instances = INOTIFY_INSTANCES.lock();
    instances.retain(|weak| {
        if let Some(inotify_file) = weak.upgrade() {
            inotify_file.queue_delete_event(path, is_dir);
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
            inotify_file.queue_move_event(old_path, new_path, is_dir, cookie);
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
