use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::File;
use crate::fs::Inode;
use crate::fs::vfs::inode::inode_alloc;
use crate::fs::vfs::inode::{InodeInner, InodeMode};
use alloc::string::String;
use alloc::sync::Arc;
use lwext4_rust::InodeTypes;
use core::sync::atomic::{AtomicU64, Ordering};
use log::info;
use spin::mutex::Mutex;

// memfd seal flags
///
pub const F_SEAL_SEAL: u64 = 0x0001;  // prevent further seal changes
///
pub const F_SEAL_SHRINK: u64 = 0x0002; // prevent shrinking
///
pub const F_SEAL_GROW: u64 = 0x0004;   // prevent growing
///
pub const F_SEAL_WRITE: u64 = 0x0008;  // prevent writes

#[allow(unused)]
/// the inode of tempfs
pub struct TempInode {
    inner: Mutex<InodeInner>,
    this_mode: InodeMode,
    link_target: Mutex<Option<String>>,
    seals: AtomicU64,
}

impl TempInode {
    ///
    pub fn new(mode: InodeMode) -> Self {
        Self {
            inner: Mutex::new(InodeInner::new(inode_alloc(), 0, mode, 0)),
            this_mode: mode,
            link_target: Mutex::new(None),
            seals: AtomicU64::new(0),
        }
    }

    /// Create a symlink inode with the given target.
    pub fn new_symlink(target: &str) -> Self {
        let mode = InodeMode::from_bits_truncate(0o777) | InodeMode::LINK;
        Self {
            inner: Mutex::new(InodeInner::new(inode_alloc(), 0, mode, 0)),
            this_mode: mode,
            link_target: Mutex::new(Some(String::from(target))),
            seals: AtomicU64::new(0),
        }
    }

    /// Create a special file inode (device, fifo, socket) with the given device number.
    pub fn new_dev(mode: InodeMode, rdev: usize) -> Self {
        Self {
            inner: Mutex::new(InodeInner::new(inode_alloc(), 0, mode, rdev)),
            this_mode: mode,
            link_target: Mutex::new(None),
            seals: AtomicU64::new(0),
        }
    }


    /// Check if a seal is set
    pub fn has_seal(&self, seal: u64) -> bool {
        (self.seals.load(Ordering::Relaxed) & seal) != 0
    }
}

impl Inode for TempInode {
    /// Get the attributes of the file, such as size, permissions, etc.
    fn get_attr(&self) -> SysResult<usize> {
        Ok(0)
    }
    /// Flush the file, synchronize the data to disk.
    fn fsync(&self) -> SysResult<usize> {
        Ok(0)
    }
    ///
    fn get_types(&self) -> InodeTypes {
        self.get_mode().to_inode_type()
    }

    fn truncate(&self, size: u64) -> SysResult<usize> {
        self.set_size(size as usize);
        crate::fs::page::pagecache::PAGE_CACHE.lock().remove_inode_pages(self.get_ino());
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
    fn get_rdev(&self) -> usize {
        self.inner.lock().rdev.load(Ordering::Relaxed)
    }
    fn set_rdev(&self, rdev: usize) {
        self.inner.lock().rdev.store(rdev, Ordering::Relaxed);
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

    fn readlink(&self) -> Result<String, i32> {
        let target = self.link_target.lock();
        match target.as_ref() {
            Some(t) => Ok(t.clone()),
            None => Err(-22),
        }
    }
    
    fn get_seals(&self) -> u64 {
        self.seals.load(Ordering::Relaxed)
    }
    
    fn set_seals(&self, new_seals: u64) -> Result<(), SysError> {
        let current = self.seals.load(Ordering::Relaxed);
        if (current & F_SEAL_SEAL) != 0 {
            return Err(SysError::EPERM);
        }
        self.seals.store(current | new_seals, Ordering::Relaxed);
        Ok(())
    }
}