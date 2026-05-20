#![allow(missing_docs)]
use crate::fs::Dentry;
use crate::fs::File;
use crate::fs::Inode;
use crate::fs::vfs::DentryInner;
use crate::fs::vfs::FileInner;
use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::vfs::OpenFlags;
use crate::fs::vfs::inode::InodeInner;
use crate::fs::vfs::inode::InodeMode;
use crate::fs::vfs::inode::inode_alloc;
use crate::mm::UserBuffer;
use crate::mm::{get_free_memory, get_total_memory};
use alloc::sync::{Arc, Weak};
use core::sync::atomic::Ordering;
use log::*;
use polyhal::consts::PAGE_SIZE;
use spin::{Mutex, MutexGuard};

pub struct MeminfoFile {
    inner: Mutex<FileInner>,
}

impl MeminfoFile {
    pub fn new(dentry: Arc<dyn Dentry>) -> Self {
        Self {
            inner: Mutex::new(FileInner { offset: 0, dentry, flags: OpenFlags::empty() }),
        }
    }
}

impl File for MeminfoFile {
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
        let total_kb = get_total_memory() / 1024;
        let free_kb = get_free_memory() / 1024;
        let _used_kb = total_kb - free_kb;
        let buffers_kb = 0usize;
        let cached_kb = 0usize;
        let avail_kb = free_kb;

        let info = alloc::format!(
            "MemTotal:       {:>8} kB\n\
             MemFree:        {:>8} kB\n\
             MemAvailable:   {:>8} kB\n\
             Buffers:        {:>8} kB\n\
             Cached:         {:>8} kB\n\
             SwapTotal:             0 kB\n\
             SwapFree:              0 kB\n",
            total_kb,
            free_kb,
            avail_kb,
            buffers_kb,
            cached_kb
        );

        let data = info.as_bytes();
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
        Ok(0)
    }

    fn open(&self) -> SyscallResult {
        Ok(0)
    }
    fn release(&self) -> SyscallResult {
        Ok(0)
    }
}

pub struct MeminfoDentry {
    inner: DentryInner,
}

impl MeminfoDentry {
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|_me: &Weak<MeminfoDentry>| Self {
            inner: DentryInner::new(name, parent_weak),
        })
    }
}

impl Dentry for MeminfoDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }
    fn name(&self) -> &str {
        &self.inner.name
    }
    fn open(self: Arc<Self>, _flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Ok(Arc::new(MeminfoFile::new(self)))
    }
}

pub struct MeminfoInode {
    inner: InodeInner,
}

impl MeminfoInode {
    pub fn new() -> Self {
        Self {
            inner: InodeInner::new(inode_alloc(), 0, InodeMode::CHAR, 0),
        }
    }
}

impl Inode for MeminfoInode {
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
        self.inner.rdev.load(core::sync::atomic::Ordering::Relaxed)
    }
    fn set_rdev(&self, rdev: usize) {
        self.inner.rdev.store(rdev, core::sync::atomic::Ordering::Relaxed);
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
