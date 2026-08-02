#![allow(missing_docs)]
use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::vfs::DentryInner;
use crate::fs::vfs::FileInner;
use crate::fs::vfs::OpenFlags;
use crate::fs::vfs::inode::InodeInner;
use crate::fs::vfs::inode::InodeMode;
use crate::fs::vfs::inode::inode_alloc;
use crate::fs::{Dentry, File, Inode};
use crate::mm::UserBuffer;
use alloc::format;
use alloc::sync::{Arc, Weak};
use core::sync::atomic::Ordering;
use spin::{Mutex, MutexGuard};

const KAIRIX_PERF_INITIAL_SIZE: usize = 256;
const BUILDSTORM_KERNEL_DIAG_VERSION: &str = "2026-08-02.11";

pub struct KairixPerfFile {
    inner: Mutex<FileInner>,
}

impl KairixPerfFile {
    pub fn new(dentry: Arc<dyn Dentry>) -> Self {
        Self {
            inner: Mutex::new(FileInner {
                offset: 0,
                dentry,
                flags: OpenFlags::empty(),
            }),
        }
    }
}

impl File for KairixPerfFile {
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
        let now_ns = polyhal::timer::current_time().as_nanos() as usize;
        let (online_mask, online_cpus, capacity_ns) =
            crate::task::manager::online_cpu_capacity_ns(now_ns);
        let user_ns = crate::task::task::global_user_runtime_ns(now_ns).min(capacity_ns);
        let idle_ns = crate::task::processor::total_idle_time_ns_at(now_ns)
            .min(capacity_ns.saturating_sub(user_ns));
        let kernel_ns = capacity_ns.saturating_sub(user_ns.saturating_add(idle_ns));
        let info = format!(
            "buildstorm_kernel_diag_version: {}\n\
             global_cpu_time: now_ns={} online_mask={:#x} online_cpus={} capacity_ns={} user_ns={} kernel_ns={} idle_ns={}\n",
            BUILDSTORM_KERNEL_DIAG_VERSION,
            now_ns,
            online_mask,
            online_cpus,
            capacity_ns,
            user_ns,
            kernel_ns,
            idle_ns,
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
        Err(SysError::EROFS)
    }

    fn open(&self) -> SyscallResult {
        Ok(0)
    }

    fn release(&self) -> SyscallResult {
        Ok(0)
    }
}

pub struct KairixPerfDentry {
    inner: DentryInner,
}

impl KairixPerfDentry {
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|_me: &Weak<KairixPerfDentry>| Self {
            inner: DentryInner::new(name, parent_weak),
        })
    }
}

impl Dentry for KairixPerfDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }

    fn name(&self) -> &str {
        &self.inner.name
    }

    fn open(self: Arc<Self>, _flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Ok(Arc::new(KairixPerfFile::new(self)))
    }
}

pub struct KairixPerfInode {
    inner: InodeInner,
}

impl KairixPerfInode {
    pub fn new() -> Self {
        let mode =
            InodeMode::FILE | InodeMode::OWNER_READ | InodeMode::GROUP_READ | InodeMode::OTHER_READ;
        Self {
            inner: InodeInner::new(inode_alloc(), KAIRIX_PERF_INITIAL_SIZE, mode, 0),
        }
    }
}

impl Inode for KairixPerfInode {
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
