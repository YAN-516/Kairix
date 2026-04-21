#![allow(missing_docs)]
use crate::fs::Dentry;
use crate::fs::File;
use crate::fs::Inode;
use crate::fs::vfs::DentryInner;
use crate::fs::vfs::FileInner;
use crate::fs::vfs::OpenFlags;
use crate::fs::vfs::inode::InodeInner;
use crate::fs::vfs::inode::InodeMode;
use crate::fs::vfs::inode::inode_alloc;
use crate::mm::UserBuffer;
use crate::mm::{translated_ref, translated_refmut};
use crate::task::current_user_token;
#[cfg(target_arch = "riscv64")]
use crate::timer::get_time_us;
use alloc::sync::{Arc, Weak};
use core::sync::atomic::Ordering;
use log::*;
use polyhal::timer::current_time;
use spin::{Mutex, MutexGuard};

/// RTC 时间结构体（与 Linux 兼容）
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RtcTime {
    pub tm_sec: i32,
    pub tm_min: i32,
    pub tm_hour: i32,
    pub tm_mday: i32,
    pub tm_mon: i32,
    pub tm_year: i32,
    pub tm_wday: i32,
    pub tm_yday: i32,
    pub tm_isdst: i32,
}

pub struct RtcFile {
    inner: Mutex<FileInner>,
}

impl RtcFile {
    pub fn new(dentry: Arc<dyn Dentry>) -> Self {
        Self {
            inner: Mutex::new(FileInner { offset: 0, dentry }),
        }
    }
}

const RTC_RD_TIME: usize = 0x8024_7009;

impl File for RtcFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        self.inner.lock()
    }

    fn readable(&self) -> bool {
        true
    }
    fn writable(&self) -> bool {
        true
    }

    fn read(&self, _buf: UserBuffer) -> usize {
        0
    }
    fn write(&self, _buf: UserBuffer) -> usize {
        0
    }

    fn ioctl(&self, request: usize, argp: usize) -> isize {
        if request == RTC_RD_TIME && argp != 0 {
            let token = current_user_token();
            let user_tm = translated_refmut(token, argp as *mut RtcTime);
            let us = current_time().as_micros() as u64;
            let total_sec = us / 1_000_000;

            *user_tm = RtcTime {
                tm_sec: (total_sec % 60) as i32,
                tm_min: ((total_sec / 60) % 60) as i32,
                tm_hour: ((total_sec / 3600) % 24) as i32,
                tm_mday: 1,
                tm_mon: 0,
                tm_year: 126, // 2026 - 1900
                tm_wday: 1,
                tm_yday: 0,
                tm_isdst: 0,
            };
            return 0;
        }
        -25 // ENOTTY
    }

    fn open(&self) -> Result<usize, i32> {
        Ok(0)
    }
    fn release(&self) -> Result<usize, i32> {
        Ok(0)
    }
}

pub struct RtcDentry {
    inner: DentryInner,
}

impl RtcDentry {
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|_me: &Weak<RtcDentry>| Self {
            inner: DentryInner::new(name, parent_weak),
        })
    }
}

impl Dentry for RtcDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }
    fn name(&self) -> &str {
        &self.inner.name
    }
    fn open(self: Arc<Self>, _flags: OpenFlags, _mode: InodeMode) -> Option<Arc<dyn File>> {
        Some(Arc::new(RtcFile::new(self)))
    }
}

pub struct RtcInode {
    inner: InodeInner,
}

impl RtcInode {
    pub fn new() -> Self {
        Self {
            inner: InodeInner::new(inode_alloc(), 0, InodeMode::CHAR),
        }
    }
}

impl Inode for RtcInode {
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
