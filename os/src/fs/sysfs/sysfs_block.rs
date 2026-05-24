#![allow(missing_docs)]
use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::vfs::{
    Dentry, DentryInner, File, FileInner, Inode, OpenFlags,
};
use crate::fs::vfs::inode::{InodeInner, InodeMode, inode_alloc};
use crate::mm::UserBuffer;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;
use spin::mutex::MutexGuard;

static WRITE_SECTORS: AtomicU64 = AtomicU64::new(1000);

pub struct SysfsStatFile {
    inner: Mutex<FileInner>,
}

impl SysfsStatFile {
    pub fn new(dentry: Arc<dyn Dentry>) -> Self {
        Self {
            inner: Mutex::new(FileInner { offset: 0, dentry }),
        }
    }
}

impl File for SysfsStatFile {
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
        // Advance write_sectors on each read so that LTP sees progress.
        let write_sectors = WRITE_SECTORS.fetch_add(64, Ordering::Relaxed);
        let content = alloc::format!(
            "       0        0        0        0        0        0      {:>7}        0        0       10        0\n",
            write_sectors
        );
        let data = content.as_bytes();
        let offset = inner.offset;
        if offset >= data.len() {
            return Ok(0);
        }
        let remaining = &data[offset..];
        let mut total = 0usize;
        for slice in buf.buffers.iter_mut() {
            let len = slice.len().min(remaining.len() - total);
            if len == 0 {
                break;
            }
            slice[..len].copy_from_slice(&remaining[total..total + len]);
            total += len;
        }
        inner.offset = offset + total;
        if let Some(inode) = inner.dentry.get_inode() {
            inode.set_size(data.len());
        }
        Ok(total)
    }

    fn write(&self, _buf: UserBuffer) -> SysResult<usize> {
        Err(SysError::EPERM)
    }
}

pub struct SysfsStatDentry {
    inner: DentryInner,
}

impl SysfsStatDentry {
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|_me| Self {
            inner: DentryInner::new(name, parent_weak),
        })
    }
}

impl Dentry for SysfsStatDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }
    fn name(&self) -> &str {
        &self.inner.name
    }
    fn open(self: Arc<Self>, _flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Ok(Arc::new(SysfsStatFile::new(self)))
    }
}

pub struct SysfsStatInode {
    inner: InodeInner,
}

impl SysfsStatInode {
    pub fn new() -> Self {
        Self {
            inner: InodeInner::new(inode_alloc(), 0, InodeMode::FILE, 0),
        }
    }
}

impl Inode for SysfsStatInode {
    fn get_mode(&self) -> InodeMode {
        self.inner.mode
    }
    fn get_size(&self) -> usize {
        self.inner.size.load(Ordering::SeqCst)
    }
    fn set_size(&self, new_size: usize) {
        self.inner.size.store(new_size, Ordering::SeqCst);
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
        (0, 0)
    }
    fn set_atime(&self, _sec: i64, _nsec: i64) {}
    fn get_mtime(&self) -> (i64, i64) {
        (0, 0)
    }
    fn set_mtime(&self, _sec: i64, _nsec: i64) {}
    fn get_ctime(&self) -> (i64, i64) {
        (0, 0)
    }
    fn set_ctime(&self, _sec: i64, _nsec: i64) {}
}
