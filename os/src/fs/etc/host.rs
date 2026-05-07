#![allow(missing_docs)]
use alloc::sync::{Arc, Weak};
use alloc::string::ToString;

use crate::sync::{SpinNoIrqLock, SpinMutexGuard, SpinNoIrq};
use crate::fs::vfs::{DentryInner, FileInner, OpenFlags, inode::InodeMode};
use crate::fs::{Dentry, File};
use crate::mm::UserBuffer;
use crate::error::{SysError, SysResult};

/// /etc/host 文件。
pub struct HostFile {
    inner: SpinNoIrqLock<FileInner>,
}

impl HostFile {
    pub fn new(dentry: Arc<dyn Dentry>) -> Self {
        Self {
            inner: SpinNoIrqLock::new(FileInner { offset: 0, dentry }),
        }
    }
}

impl File for HostFile {
    fn get_fileinner(&self) -> SpinMutexGuard<'_, FileInner, SpinNoIrq> {
        self.inner.lock()
    }

    fn readable(&self) -> bool { true }
    fn writable(&self) -> bool { true }

    fn read(&self, mut buf: UserBuffer) -> SysResult<usize> {
        let mut inner = self.get_fileinner();
        static CONTENT: &[u8] = b"127.0.0.1\tlocalhost\n127.0.1.1\tkairix\n";
        let offset = inner.offset;
        if offset >= CONTENT.len() {
            return Ok(0);
        }
        let remaining = &CONTENT[offset..];
        let mut total = 0usize;
        for slice in buf.buffers.iter_mut() {
            let len = slice.len().min(remaining.len() - total);
            if len == 0 { break; }
            slice[..len].copy_from_slice(&remaining[total..total + len]);
            total += len;
        }
        inner.offset = offset + total;
        if let Some(inode) = inner.dentry.get_inode() {
            inode.set_size(CONTENT.len());
        }
        Ok(total)
    }

    fn write(&self, buf: UserBuffer) -> SysResult<usize> {
        Ok(buf.len())
    }
}

/// /etc/host 的 dentry。
pub struct HostDentry {
    inner: DentryInner,
}

impl HostDentry {
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|_me: &Weak<HostDentry>| Self {
            inner: DentryInner::new(name, parent_weak),
        })
    }
}

impl Dentry for HostDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }
    fn name(&self) -> &str {
        &self.inner.name
    }
    fn open(self: Arc<Self>, _flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Ok(Arc::new(HostFile::new(self)))
    }
}
