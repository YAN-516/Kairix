#![allow(missing_docs)]
use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::vfs::inode::{inode_alloc, InodeInner, InodeMode};
use crate::fs::vfs::{DentryInner, FileInner, OpenFlags};
use crate::fs::{Dentry, File, Inode};
use crate::mm::UserBuffer;
use alloc::format;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::str;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::{Mutex, MutexGuard};

static INOTIFY_MAX_USER_INSTANCES: AtomicUsize = AtomicUsize::new(128);
static INOTIFY_MAX_USER_WATCHES: AtomicUsize = AtomicUsize::new(8192);
static INOTIFY_MAX_QUEUED_EVENTS: AtomicUsize = AtomicUsize::new(1024);

#[derive(Clone, Copy)]
pub enum InotifySysctlKind {
    MaxUserInstances,
    MaxUserWatches,
    MaxQueuedEvents,
}

impl InotifySysctlKind {
    fn load(self) -> usize {
        self.value().load(Ordering::Relaxed)
    }

    fn store(self, value: usize) {
        self.value().store(value, Ordering::Relaxed);
    }

    fn value(self) -> &'static AtomicUsize {
        match self {
            Self::MaxUserInstances => &INOTIFY_MAX_USER_INSTANCES,
            Self::MaxUserWatches => &INOTIFY_MAX_USER_WATCHES,
            Self::MaxQueuedEvents => &INOTIFY_MAX_QUEUED_EVENTS,
        }
    }
}

pub struct InotifySysctlFile {
    inner: Mutex<FileInner>,
    kind: InotifySysctlKind,
}

impl InotifySysctlFile {
    pub fn new(dentry: Arc<dyn Dentry>, kind: InotifySysctlKind) -> Self {
        Self {
            inner: Mutex::new(FileInner {
                offset: 0,
                dentry,
                flags: OpenFlags::empty(),
            }),
            kind,
        }
    }
}

impl File for InotifySysctlFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        self.inner.lock()
    }

    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        true
    }

    fn read(&self, mut buf: UserBuffer) -> SysResult<usize> {
        let mut inner = self.get_fileinner();
        let info = format!("{}\n", self.kind.load());
        let data = info.as_bytes();
        let offset = inner.offset;
        if offset >= data.len() {
            return Ok(0);
        }

        let mut total = 0usize;
        for slice in buf.buffers.iter_mut() {
            let len = slice.len().min(data.len() - offset - total);
            if len == 0 {
                break;
            }
            slice[..len].copy_from_slice(&data[offset + total..offset + total + len]);
            total += len;
        }

        inner.offset = offset + total;
        if let Some(inode) = inner.dentry.get_inode() {
            inode.set_size(data.len());
        }
        Ok(total)
    }

    fn write(&self, buf: UserBuffer) -> SysResult<usize> {
        let len = buf.len();
        let value = parse_sysctl_value(&buf)?;
        self.kind.store(value);
        if let Some(inode) = self.get_fileinner().dentry.get_inode() {
            inode.set_size(format!("{}\n", value).len());
        }
        Ok(len)
    }

    fn open(&self) -> SyscallResult {
        Ok(0)
    }

    fn release(&self) -> SyscallResult {
        Ok(0)
    }
}

fn parse_sysctl_value(buf: &UserBuffer) -> SysResult<usize> {
    let mut bytes = Vec::new();
    for slice in buf.buffers.iter() {
        bytes.extend_from_slice(slice);
    }
    let text = str::from_utf8(&bytes).map_err(|_| SysError::EINVAL)?.trim();
    if text.is_empty() {
        return Err(SysError::EINVAL);
    }

    let mut value = 0usize;
    for byte in text.bytes() {
        if !byte.is_ascii_digit() {
            return Err(SysError::EINVAL);
        }
        value = value
            .checked_mul(10)
            .and_then(|value| value.checked_add((byte - b'0') as usize))
            .ok_or(SysError::EINVAL)?;
    }
    Ok(value)
}

pub struct InotifySysctlDentry {
    inner: DentryInner,
    kind: InotifySysctlKind,
}

impl InotifySysctlDentry {
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>, kind: InotifySysctlKind) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|_me: &Weak<InotifySysctlDentry>| Self {
            inner: DentryInner::new(name, parent_weak),
            kind,
        })
    }
}

impl Dentry for InotifySysctlDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }

    fn name(&self) -> &str {
        &self.inner.name
    }

    fn open(self: Arc<Self>, _flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Ok(Arc::new(InotifySysctlFile::new(self.clone(), self.kind)))
    }
}

pub struct InotifySysctlInode {
    inner: InodeInner,
}

impl InotifySysctlInode {
    pub fn new() -> Self {
        Self {
            inner: InodeInner::new(
                inode_alloc(),
                0,
                InodeMode::FILE
                    | InodeMode::OWNER_READ
                    | InodeMode::OWNER_WRITE
                    | InodeMode::GROUP_READ
                    | InodeMode::OTHER_READ,
                0,
            ),
        }
    }
}

impl Inode for InotifySysctlInode {
    fn get_mode(&self) -> InodeMode {
        self.inner.mode
    }

    fn set_size(&self, new_size: usize) {
        self.inner.size.store(new_size, Ordering::SeqCst);
    }

    fn get_size(&self) -> usize {
        self.inner.size.load(Ordering::SeqCst)
    }

    fn get_ino(&self) -> usize {
        self.inner.ino
    }

    fn get_nlink(&self) -> usize {
        self.inner.nlink.load(Ordering::SeqCst)
    }

    fn get_rdev(&self) -> usize {
        self.inner.rdev.load(Ordering::Relaxed)
    }

    fn set_rdev(&self, rdev: usize) {
        self.inner.rdev.store(rdev, Ordering::Relaxed);
    }

    fn inc_nlink(&self) {
        self.inner.nlink.fetch_add(1, Ordering::SeqCst);
    }

    fn dec_nlink(&self) {
        self.inner.nlink.fetch_sub(1, Ordering::SeqCst);
    }

    fn get_atime(&self) -> (i64, i64) {
        (
            self.inner.atime_sec.load(Ordering::Relaxed),
            self.inner.atime_nsec.load(Ordering::Relaxed),
        )
    }

    fn set_atime(&self, sec: i64, nsec: i64) {
        self.inner.atime_sec.store(sec, Ordering::Relaxed);
        self.inner.atime_nsec.store(nsec, Ordering::Relaxed);
    }

    fn get_mtime(&self) -> (i64, i64) {
        (
            self.inner.mtime_sec.load(Ordering::Relaxed),
            self.inner.mtime_nsec.load(Ordering::Relaxed),
        )
    }

    fn set_mtime(&self, sec: i64, nsec: i64) {
        self.inner.mtime_sec.store(sec, Ordering::Relaxed);
        self.inner.mtime_nsec.store(nsec, Ordering::Relaxed);
    }

    fn get_ctime(&self) -> (i64, i64) {
        (
            self.inner.ctime_sec.load(Ordering::Relaxed),
            self.inner.ctime_nsec.load(Ordering::Relaxed),
        )
    }

    fn set_ctime(&self, sec: i64, nsec: i64) {
        self.inner.ctime_sec.store(sec, Ordering::Relaxed);
        self.inner.ctime_nsec.store(nsec, Ordering::Relaxed);
    }
}
