#![allow(missing_docs)]
use alloc::sync::{Arc, Weak};
use alloc::string::ToString;

use spin::{Mutex, MutexGuard};
use crate::fs::vfs::inode::{inode_alloc, InodeInner, InodeMode};
use crate::fs::vfs::{DentryInner, FileInner, OpenFlags};
use crate::fs::{Dentry, File, Inode};
use crate::mm::UserBuffer;
use crate::error::{SysError, SysResult};

/// /etc/localtime 文件（空文件）。
pub struct LocaltimeFile {
    inner: Mutex<FileInner>,
}

impl LocaltimeFile {
    pub fn new(dentry: Arc<dyn Dentry>) -> Self {
        Self {
            inner: Mutex::new(FileInner { offset: 0, dentry }),
        }
    }
}

impl File for LocaltimeFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        self.inner.lock()
    }

    fn readable(&self) -> bool { true }
    fn writable(&self) -> bool { true }

    fn read(&self, _buf: UserBuffer) -> SysResult<usize> {
        Ok(0)
    }

    fn write(&self, buf: UserBuffer) -> SysResult<usize> {
        Ok(buf.len())
    }
}

/// /etc/localtime 的 dentry。
pub struct LocaltimeDentry {
    inner: DentryInner,
}

impl LocaltimeDentry {
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|_me: &Weak<LocaltimeDentry>| Self {
            inner: DentryInner::new(name, parent_weak),
        })
    }
}

impl Dentry for LocaltimeDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }
    fn name(&self) -> &str {
        &self.inner.name
    }
    fn open(self: Arc<Self>, _flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Ok(Arc::new(LocaltimeFile::new(self)))
    }
}

/// /etc/localtime 的 inode。
pub struct LocaltimeInode {
    inner: InodeInner,
}

impl LocaltimeInode {
    pub fn new() -> Self {
        Self {
            inner: InodeInner::new(inode_alloc(), 0, InodeMode::FILE, 0),
        }
    }
}

impl Inode for LocaltimeInode {
    fn get_mode(&self) -> InodeMode {
        self.inner.mode
    }
    fn set_size(&self, new_size: usize) {
        self.inner.size.store(new_size, core::sync::atomic::Ordering::SeqCst);
    }
    fn get_size(&self) -> usize {
        self.inner.size.load(core::sync::atomic::Ordering::SeqCst)
    }
    fn get_ino(&self) -> usize {
        self.inner.ino
    }
    fn get_nlink(&self) -> usize {
        self.inner.nlink.load(core::sync::atomic::Ordering::SeqCst)
    }
    fn get_rdev(&self) -> usize {
        self.inner.rdev.load(core::sync::atomic::Ordering::Relaxed)
    }
    fn set_rdev(&self, rdev: usize) {
        self.inner.rdev.store(rdev, core::sync::atomic::Ordering::Relaxed);
    }
    fn inc_nlink(&self) {
        self.inner.nlink.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
    }
    fn dec_nlink(&self) {
        self.inner.nlink.fetch_sub(1, core::sync::atomic::Ordering::SeqCst);
    }
    fn get_atime(&self) -> (i64, i64) {
        (
            self.inner.atime_sec.load(core::sync::atomic::Ordering::Relaxed),
            self.inner.atime_nsec.load(core::sync::atomic::Ordering::Relaxed),
        )
    }
    fn set_atime(&self, sec: i64, nsec: i64) {
        self.inner.atime_sec.store(sec, core::sync::atomic::Ordering::Relaxed);
        self.inner.atime_nsec.store(nsec, core::sync::atomic::Ordering::Relaxed);
    }
    fn get_mtime(&self) -> (i64, i64) {
        (
            self.inner.mtime_sec.load(core::sync::atomic::Ordering::Relaxed),
            self.inner.mtime_nsec.load(core::sync::atomic::Ordering::Relaxed),
        )
    }
    fn set_mtime(&self, sec: i64, nsec: i64) {
        self.inner.mtime_sec.store(sec, core::sync::atomic::Ordering::Relaxed);
        self.inner.mtime_nsec.store(nsec, core::sync::atomic::Ordering::Relaxed);
    }
    fn get_ctime(&self) -> (i64, i64) {
        (
            self.inner.ctime_sec.load(core::sync::atomic::Ordering::Relaxed),
            self.inner.ctime_nsec.load(core::sync::atomic::Ordering::Relaxed),
        )
    }
    fn set_ctime(&self, sec: i64, nsec: i64) {
        self.inner.ctime_sec.store(sec, core::sync::atomic::Ordering::Relaxed);
        self.inner.ctime_nsec.store(nsec, core::sync::atomic::Ordering::Relaxed);
    }
}
