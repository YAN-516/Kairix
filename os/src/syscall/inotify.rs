#![allow(missing_docs)]

use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::vfs::path::resolve_path;
use crate::fs::vfs::inode::InodeMode;
use crate::fs::vfs::{Dentry, DentryInner, File, FileInner, OpenFlags};
use crate::mm::{translated_str, UserBuffer};
use crate::task::{current_process, current_user_token};
use alloc::collections::{BTreeMap, VecDeque};
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
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
pub const IN_IGNORED: u32 = 0x0000_8000;

const INOTIFY_EVENT_SIZE: usize = 16;

pub struct InotifyFile {
    inner: Mutex<FileInner>,
    status_flags: Mutex<u32>,
    state: Arc<Mutex<InotifyState>>,
}

struct InotifyWatch {
    wd: i32,
    path: alloc::string::String,
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

    fn add_watch(&self, path: alloc::string::String, mask: u32) -> i32 {
        let mut state = self.state.lock();
        if let Some(watch) = state.watches.values_mut().find(|watch| watch.path == path) {
            watch.mask = mask;
            return watch.wd;
        }
        let wd = state.next_wd;
        state.next_wd += 1;
        state.watches.insert(wd, InotifyWatch { wd, path, mask });
        wd
    }

    fn queue_matching_event(&self, path: &str, mask: u32) {
        let mut state = self.state.lock();
        let mut events = Vec::new();
        for watch in state.watches.values() {
            if watch.path == path && watch.mask & mask != 0 {
                events.push(InotifyEvent {
                    wd: watch.wd,
                    mask,
                    cookie: 0,
                    name: Vec::new(),
                });
            }
        }
        state.events.extend(events);
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
