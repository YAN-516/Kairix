use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::File;
use crate::fs::Inode;
use crate::fs::vfs::inode::inode_alloc;
use crate::fs::vfs::inode::make_rdev;
use crate::fs::vfs::inode::{InodeInner, InodeMode};
use alloc::sync::Arc;
use core::sync::atomic::Ordering;
use log::info;
use lwext4_rust::InodeTypes;
use spin::mutex::Mutex;
pub struct Cgroup2Inode {
    inner: Mutex<InodeInner>,
}
//待改dev
impl Cgroup2Inode {
    pub fn new(mode: InodeMode) -> Self {
        Self {
            inner: Mutex::new(InodeInner::new(
                inode_alloc(),
                0,
                mode,
                make_rdev(2, 12) as usize,
            )),
        }
    }
}

impl Inode for Cgroup2Inode {
    fn get_attr(&self) -> SysResult<usize> {
        Ok(0)
    }
    fn fsync(&self) -> SysResult<usize> {
        Ok(0)
    }
    fn get_types(&self) -> InodeTypes {
        self.get_mode().to_inode_type()
    }
    fn truncate(&self, size: u64) -> SysResult<usize> {
        self.set_size(size as usize);
        crate::fs::page::pagecache::PAGE_CACHE
            .lock()
            .remove_inode_pages(self.get_ino());
        Ok(0)
    }
    fn get_ino(&self) -> usize {
        self.inner.lock().ino
    }
    fn get_size(&self) -> usize {
        self.inner.lock().size.load(Ordering::Relaxed)
    }
    fn set_size(&self, new_size: usize) {
        self.inner.lock().size.store(new_size, Ordering::Relaxed);
    }
    fn get_nlink(&self) -> usize {
        self.inner.lock().nlink.load(Ordering::Relaxed)
    }
    fn get_mode(&self) -> InodeMode {
        self.inner.lock().mode
    }
    fn set_mode(&self, mode: InodeMode) {
        self.inner.lock().mode = mode;
    }
    fn get_uid(&self) -> usize {
        self.inner.lock().uid.load(Ordering::Relaxed)
    }
    fn set_uid(&self, uid: usize) {
        self.inner.lock().uid.store(uid, Ordering::Relaxed);
    }
    fn get_gid(&self) -> usize {
        self.inner.lock().gid.load(Ordering::Relaxed)
    }
    fn set_gid(&self, gid: usize) {
        self.inner.lock().gid.store(gid, Ordering::Relaxed);
    }
    fn inc_nlink(&self) {
        self.inner.lock().nlink.fetch_add(1, Ordering::SeqCst);
    }
    fn dec_nlink(&self) {
        self.inner.lock().nlink.fetch_sub(1, Ordering::SeqCst);
    }
    fn get_atime(&self) -> (i64, i64) {
        let inner = self.inner.lock();
        (
            inner.atime_sec.load(Ordering::Relaxed),
            inner.atime_nsec.load(Ordering::Relaxed),
        )
    }
    fn set_atime(&self, sec: i64, nsec: i64) {
        let inner = self.inner.lock();
        inner.atime_sec.store(sec, Ordering::Relaxed);
        inner.atime_nsec.store(nsec, Ordering::Relaxed);
    }
    fn get_mtime(&self) -> (i64, i64) {
        let inner = self.inner.lock();
        (
            inner.mtime_sec.load(Ordering::Relaxed),
            inner.mtime_nsec.load(Ordering::Relaxed),
        )
    }
    fn set_mtime(&self, sec: i64, nsec: i64) {
        let inner = self.inner.lock();
        inner.mtime_sec.store(sec, Ordering::Relaxed);
        inner.mtime_nsec.store(nsec, Ordering::Relaxed);
    }
    fn get_ctime(&self) -> (i64, i64) {
        let inner = self.inner.lock();
        (
            inner.ctime_sec.load(Ordering::Relaxed),
            inner.ctime_nsec.load(Ordering::Relaxed),
        )
    }
    fn set_ctime(&self, sec: i64, nsec: i64) {
        let inner = self.inner.lock();
        inner.ctime_sec.store(sec, Ordering::Relaxed);
        inner.ctime_nsec.store(nsec, Ordering::Relaxed);
    }
}
