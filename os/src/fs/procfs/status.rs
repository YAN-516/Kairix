#![allow(missing_docs)]
use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::vfs::inode::inode_alloc;
use crate::fs::vfs::inode::InodeInner;
use crate::fs::vfs::inode::InodeMode;
use crate::fs::vfs::DentryInner;
use crate::fs::vfs::FileInner;
use crate::fs::vfs::OpenFlags;
use crate::fs::Dentry;
use crate::fs::File;
use crate::fs::Inode;
use crate::mm::vm_area::MapArea;
use crate::mm::UserBuffer;
use crate::task::current_process;
use alloc::format;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use core::sync::atomic::Ordering;
use polyhal::consts::PAGE_SIZE;
use spin::{Mutex, MutexGuard};

/// /proc/self/status 文件。
pub struct StatusFile {
    inner: Mutex<FileInner>,
}

impl StatusFile {
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

impl File for StatusFile {
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
        let process = current_process();
        let proc_inner = process.inner_exclusive_access();

        let mut info = String::new();

        // 进程名称
        info.push_str(&format!("Name:\t{}\n", "ltp_test"));

        // 进程ID (线程组ID)
        info.push_str(&format!("Pid:\t{}\n", process.pid.0));

        // 线程组ID (Tgid)
        info.push_str(&format!("Tgid:\t{}\n", process.pid.0));

        // 父进程ID
        let ppid = proc_inner
            .parent
            .as_ref()
            .map(|p| p.upgrade().map(|p| p.pid.0).unwrap_or(0))
            .unwrap_or(0);
        info.push_str(&format!("PPid:\t{}\n", ppid));

        // 进程状态
        info.push_str(&format!("State:\tR (running)\n"));

        // 线程数
        info.push_str(&format!("Threads:\t{}\n", 1));

        // UID/GID
        info.push_str(&format!("Uid:\t0\t0\t0\t0\n"));
        info.push_str(&format!("Gid:\t0\t0\t0\t0\n"));

        // 内存信息
        let mut vmsize = 0;
        let mut rss = 0;
        for area in proc_inner.vm_set.areas.iter() {
            vmsize += area.end_va().0 - area.start_va().0;
            rss += area.data_frames.len() * PAGE_SIZE;
        }
        info.push_str(&format!("VmSize:\t{} kB\n", vmsize / 1024));
        info.push_str(&format!("VmRSS:\t{} kB\n", rss / 1024));
        info.push_str(&format!("VmData:\t{} kB\n", vmsize / 1024));
        info.push_str(&format!("VmStack:\t{} kB\n", 8192));
        info.push_str(&format!("VmLck:\t{} kB\n", vmsize / 1024));

        drop(proc_inner);

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

/// /proc/self/status 的 dentry。
pub struct StatusDentry {
    inner: DentryInner,
}

impl StatusDentry {
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|_me: &Weak<StatusDentry>| Self {
            inner: DentryInner::new(name, parent_weak),
        })
    }
}

impl Dentry for StatusDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }
    fn name(&self) -> &str {
        &self.inner.name
    }
    fn open(self: Arc<Self>, _flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Ok(Arc::new(StatusFile::new(self)))
    }
}

/// /proc/self/status 的 inode。
pub struct StatusInode {
    inner: InodeInner,
}

impl StatusInode {
    pub fn new() -> Self {
        Self {
            inner: InodeInner::new(inode_alloc(), 0, InodeMode::FILE, 0),
        }
    }
}

impl Inode for StatusInode {
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
        self.inner
            .rdev
            .store(rdev, core::sync::atomic::Ordering::Relaxed);
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
