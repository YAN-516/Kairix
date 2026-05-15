use crate::error::{SysError, SysResult};
use crate::fs::fat32::superblock::Fat32SuperBlock;
use crate::fs::vfs::inode::{inode_alloc, Inode, InodeInner, InodeMode};
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use core::sync::atomic::Ordering;
use lwext4_rust::InodeTypes;
use spin::mutex::Mutex;

pub struct Fat32Inode {
    inner: Mutex<InodeInner>,
    rel_path: String,
    is_dir: bool,
    superblock: Weak<Fat32SuperBlock>,
    link_target: Mutex<Option<String>>,
}

impl Fat32Inode {
    pub fn new(
        ino: usize,
        size: usize,
        mode: InodeMode,
        rel_path: String,
        is_dir: bool,
        superblock: Weak<Fat32SuperBlock>,
    ) -> Self {
        Self {
            inner: Mutex::new(InodeInner::new(ino, size, mode, 0)),
            rel_path,
            is_dir,
            superblock,
            link_target: Mutex::new(None),
        }
    }

    pub fn new_symlink(
        target: &str,
        rel_path: String,
        superblock: Weak<Fat32SuperBlock>,
    ) -> Self {
        let mode = InodeMode::from_bits_truncate(0o777) | InodeMode::LINK;
        Self {
            inner: Mutex::new(InodeInner::new(inode_alloc(), 0, mode, 0)),
            rel_path,
            is_dir: false,
            superblock,
            link_target: Mutex::new(Some(String::from(target))),
        }
    }
}

impl Inode for Fat32Inode {
    fn get_attr(&self) -> SysResult<usize> {
        Ok(0)
    }

    fn fsync(&self) -> SysResult<usize> {
        Ok(0)
    }

    fn truncate(&self, size: u64) -> SysResult<usize> {
        self.set_size(size as usize);
        crate::fs::page::pagecache::PAGE_CACHE.lock().remove_inode_pages(self.get_ino());
        Ok(0)
    }

    fn get_types(&self) -> InodeTypes {
        let mode = self.inner.lock().mode;
        if mode.contains(InodeMode::LINK) {
            InodeTypes::EXT4_DE_SYMLINK
        } else if self.is_dir {
            InodeTypes::EXT4_DE_DIR
        } else {
            InodeTypes::EXT4_DE_REG_FILE
        }
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
}
