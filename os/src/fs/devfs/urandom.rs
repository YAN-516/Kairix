#![allow(missing_docs)]
use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::vfs::OpenFlags;
use crate::fs::vfs::inode::inode_alloc;
use crate::fs::vfs::inode::{InodeInner, InodeMode, make_rdev};
use crate::fs::vfs::{DentryInner, FileInner};
use crate::fs::{Dentry, File, Inode, String};
use crate::mm::UserBuffer;
#[cfg(target_arch = "riscv64")]
use crate::sbi;
#[cfg(target_arch = "riscv64")]
use crate::sbi::get_tp;
#[cfg(target_arch = "loongarch64")]
use crate::sbi_la::get_tp;
#[cfg(target_arch = "riscv64")]
use crate::timer::get_time_us;
use alloc::sync::{Arc, Weak};
use core::sync::atomic::Ordering;
use polyhal::timer::current_time;
use spin::{Mutex, MutexGuard};

lazy_static::lazy_static! {
    static ref RNG_STATE: Mutex<u64> = Mutex::new(0);
}

fn xorshift64(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// 填充缓冲区为伪随机字节
pub fn fill_random(buf: &mut [u8]) {
    let mut state = RNG_STATE.lock();
    if *state == 0 {
        *state = (current_time().as_micros() as u64)
            .wrapping_add(get_tp() as u64)
            .wrapping_add(0x9e3779b97f4a7c15);
    }
    let mut word = xorshift64(&mut *state);
    let mut word_idx = 0usize;
    for i in 0..buf.len() {
        if word_idx == 8 {
            word = xorshift64(&mut *state);
            word_idx = 0;
        }
        buf[i] = (word >> (word_idx * 8)) as u8;
        word_idx += 1;
    }
}

pub struct UrandomFile {
    inner: Mutex<FileInner>,
}

impl UrandomFile {
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

impl File for UrandomFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        self.inner.lock()
    }

    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        false
    }

    fn read(&self, buf: UserBuffer) -> SysResult<usize> {
        let mut total = 0usize;
        for slice in buf.buffers.into_iter() {
            fill_random(slice);
            total += slice.len();
        }
        Ok(total)
    }

    fn write(&self, _buf: UserBuffer) -> SysResult<usize> {
        Ok(0)
    }
}

unsafe impl Send for UrandomDentry {}
unsafe impl Sync for UrandomDentry {}

pub struct UrandomDentry {
    inner: DentryInner,
}

impl UrandomDentry {
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|_me: &Weak<UrandomDentry>| Self {
            inner: DentryInner::new(name, parent_weak),
        })
    }
}

impl Dentry for UrandomDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }

    fn name(&self) -> &str {
        &self.inner.name
    }

    fn open(self: Arc<Self>, _flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Ok(Arc::new(UrandomFile::new(self)))
    }
}

pub struct UrandomInode {
    inner: InodeInner,
}

impl UrandomInode {
    pub fn new() -> Self {
        let mode = InodeMode::CHAR;
        Self {
            inner: InodeInner::new(inode_alloc(), 0, mode, make_rdev(1, 9) as usize),
        }
    }
}

impl Inode for UrandomInode {
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
