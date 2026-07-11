#![allow(missing_docs)]

use crate::error::{SysError, SysResult};
use crate::fs::vfs::inode::{InodeInner, InodeMode, inode_alloc};
use crate::fs::vfs::{Dentry, DentryInner, FileInner, OpenFlags};
use crate::fs::{File, Inode};
use crate::mm::UserBuffer;
use alloc::sync::{Arc, Weak};
use core::sync::atomic::Ordering;
use spin::{Mutex, MutexGuard};

pub struct InterruptsFile {
    inner: Mutex<FileInner>,
}

impl InterruptsFile {
    fn new(dentry: Arc<dyn Dentry>, flags: OpenFlags) -> Self {
        Self {
            inner: Mutex::new(FileInner {
                offset: 0,
                dentry,
                flags,
            }),
        }
    }
}

impl File for InterruptsFile {
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
        let data = crate::interrupts::render();
        let bytes = data.as_bytes();
        let mut inner = self.get_fileinner();
        let offset = inner.offset;
        if offset >= bytes.len() {
            return Ok(0);
        }

        let remaining = &bytes[offset..];
        let mut copied = 0;
        for slice in &mut buf.buffers {
            let len = slice.len().min(remaining.len() - copied);
            if len == 0 {
                break;
            }
            slice[..len].copy_from_slice(&remaining[copied..copied + len]);
            copied += len;
        }

        inner.offset += copied;
        if let Some(inode) = inner.dentry.get_inode() {
            inode.set_size(bytes.len());
        }
        Ok(copied)
    }

    fn write(&self, _buf: UserBuffer) -> SysResult<usize> {
        Err(SysError::EPERM)
    }
}

pub struct InterruptsDentry {
    inner: DentryInner,
}

impl InterruptsDentry {
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>) -> Arc<Self> {
        let parent = parent.as_ref().map(Arc::downgrade);
        Arc::new_cyclic(|_me: &Weak<Self>| Self {
            inner: DentryInner::new(name, parent),
        })
    }
}

impl Dentry for InterruptsDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }

    fn open(self: Arc<Self>, flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        if flags.writable() {
            return Err(SysError::EACCES);
        }
        Ok(Arc::new(InterruptsFile::new(self, flags)))
    }
}

pub struct InterruptsInode {
    inner: InodeInner,
}

impl InterruptsInode {
    pub fn new() -> Self {
        let mode =
            InodeMode::FILE | InodeMode::OWNER_READ | InodeMode::GROUP_READ | InodeMode::OTHER_READ;
        Self {
            inner: InodeInner::new(inode_alloc(), 0, mode, 0),
        }
    }
}

impl Inode for InterruptsInode {
    fn get_ino(&self) -> usize {
        self.inner.ino
    }

    fn get_mode(&self) -> InodeMode {
        self.inner.mode
    }

    fn get_size(&self) -> usize {
        self.inner.size.load(Ordering::Relaxed)
    }

    fn set_size(&self, new_size: usize) {
        self.inner.size.store(new_size, Ordering::Relaxed);
    }

    fn get_nlink(&self) -> usize {
        self.inner.nlink.load(Ordering::Relaxed)
    }
}
