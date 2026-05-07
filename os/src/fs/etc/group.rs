#![allow(missing_docs)]
use alloc::sync::{Arc, Weak};
use alloc::string::ToString;

use crate::sync::{SpinNoIrqLock, SpinMutexGuard, SpinNoIrq};
use crate::fs::vfs::{DentryInner, FileInner, OpenFlags, inode::InodeMode};
use crate::fs::{Dentry, File};
use crate::mm::UserBuffer;
use crate::error::{SysError, SysResult};

/// /etc/group 文件。
pub struct GroupFile {
    inner: SpinNoIrqLock<FileInner>,
}

impl GroupFile {
    pub fn new(dentry: Arc<dyn Dentry>) -> Self {
        Self {
            inner: SpinNoIrqLock::new(FileInner { offset: 0, dentry }),
        }
    }
}

impl File for GroupFile {
    fn get_fileinner(&self) -> SpinMutexGuard<'_, FileInner, SpinNoIrq> {
        self.inner.lock()
    }

    fn readable(&self) -> bool { true }
    fn writable(&self) -> bool { true }

    fn read(&self, mut buf: UserBuffer) -> SysResult<usize> {
        let mut inner = self.get_fileinner();
        static CONTENT: &[u8] = b"root:x:0:\n";
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

/// /etc/group 的 dentry。
pub struct GroupDentry {
    inner: DentryInner,
}

impl GroupDentry {
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|_me: &Weak<GroupDentry>| Self {
            inner: DentryInner::new(name, parent_weak),
        })
    }
}

impl Dentry for GroupDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }
    fn name(&self) -> &str {
        &self.inner.name
    }
    fn open(self: Arc<Self>, _flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Ok(Arc::new(GroupFile::new(self)))
    }
}
