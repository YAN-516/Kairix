#![allow(missing_docs)]
use alloc::sync::{Arc, Weak};
use alloc::string::ToString;

use crate::sync::{SpinNoIrqLock, SpinMutexGuard, SpinNoIrq};
use crate::fs::vfs::{DentryInner, FileInner, OpenFlags, inode::InodeMode};
use crate::fs::{Dentry, File};
use crate::mm::UserBuffer;
use crate::error::{SysError, SysResult};

/// /etc/localtime 文件（空文件）。
pub struct LocaltimeFile {
    inner: SpinNoIrqLock<FileInner>,
}

impl LocaltimeFile {
    pub fn new(dentry: Arc<dyn Dentry>) -> Self {
        Self {
            inner: SpinNoIrqLock::new(FileInner { offset: 0, dentry }),
        }
    }
}

impl File for LocaltimeFile {
    fn get_fileinner(&self) -> SpinMutexGuard<'_, FileInner, SpinNoIrq> {
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