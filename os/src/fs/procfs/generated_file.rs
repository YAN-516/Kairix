#![allow(missing_docs)]

use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::vfs::inode::{InodeInner, InodeMode, inode_alloc};
use crate::fs::vfs::{DentryInner, FileInner, OpenFlags};
use crate::fs::{Dentry, File, Inode};
use crate::mm::UserBuffer;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use core::sync::atomic::Ordering;
use spin::{Mutex, MutexGuard};

pub type ContentGenerator = fn() -> String;

/// A read-only procfs file whose complete contents are generated on demand.
pub struct GeneratedFile {
    inner: Mutex<FileInner>,
    generate: ContentGenerator,
}

impl GeneratedFile {
    fn new(dentry: Arc<dyn Dentry>, flags: OpenFlags, generate: ContentGenerator) -> Self {
        Self {
            inner: Mutex::new(FileInner {
                offset: 0,
                dentry,
                flags,
            }),
            generate,
        }
    }
}

impl File for GeneratedFile {
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
        let mut inner = self.get_fileinner();
        let content = (self.generate)();
        let data = content.as_bytes();
        let offset = inner.offset;
        if offset >= data.len() {
            return Ok(0);
        }

        let remaining = &data[offset..];
        let mut copied = 0usize;
        for slice in buf.buffers.iter_mut() {
            let len = slice.len().min(remaining.len() - copied);
            if len == 0 {
                break;
            }
            slice[..len].copy_from_slice(&remaining[copied..copied + len]);
            copied += len;
        }

        inner.offset = offset + copied;
        if let Some(inode) = inner.dentry.get_inode() {
            inode.set_size(data.len());
        }
        Ok(copied)
    }

    fn write(&self, _buf: UserBuffer) -> SysResult<usize> {
        Err(SysError::EBADF)
    }

    fn open(&self) -> SyscallResult {
        Ok(0)
    }

    fn release(&self) -> SyscallResult {
        Ok(0)
    }
}

pub struct GeneratedFileDentry {
    inner: DentryInner,
    generate: ContentGenerator,
}

impl GeneratedFileDentry {
    pub fn new(
        name: &str,
        parent: Option<Arc<dyn Dentry>>,
        generate: ContentGenerator,
    ) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(Arc::downgrade);
        Arc::new_cyclic(|_me: &Weak<GeneratedFileDentry>| Self {
            inner: DentryInner::new(name, parent_weak),
            generate,
        })
    }
}

impl Dentry for GeneratedFileDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }

    fn open(self: Arc<Self>, flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        let generate = self.generate;
        Ok(Arc::new(GeneratedFile::new(self, flags, generate)))
    }
}

pub struct GeneratedFileInode {
    inner: InodeInner,
}

impl GeneratedFileInode {
    pub fn new() -> Self {
        let mode =
            InodeMode::FILE | InodeMode::OWNER_READ | InodeMode::GROUP_READ | InodeMode::OTHER_READ;
        Self {
            inner: InodeInner::new(inode_alloc(), 0, mode, 0),
        }
    }
}

impl Inode for GeneratedFileInode {
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
