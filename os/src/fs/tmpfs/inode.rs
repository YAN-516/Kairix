use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::File;
use crate::fs::Inode;
use crate::fs::vfs::inode::inode_alloc;
use crate::fs::vfs::inode::{InodeInner, InodeMode};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use lwext4_rust::InodeTypes;
use core::sync::atomic::Ordering;
use log::info;
use spin::mutex::Mutex;

#[allow(unused)]
/// the inode of tempfs
pub struct TempInode {
    inner: Mutex<InodeInner>,
    this_mode: InodeMode,
    link_target: Mutex<Option<String>>,
    xattrs: Mutex<BTreeMap<String, Vec<u8>>>,
}

impl TempInode {
    ///
    pub fn new(mode: InodeMode) -> Self {
        Self {
            inner: Mutex::new(InodeInner::new(inode_alloc(), 0, mode, 0)),
            this_mode: mode,
            link_target: Mutex::new(None),
            xattrs: Mutex::new(BTreeMap::new()),
        }
    }

    /// Create a symlink inode with the given target.
    pub fn new_symlink(target: &str) -> Self {
        let mode = InodeMode::from_bits_truncate(0o777) | InodeMode::LINK;
        Self {
            inner: Mutex::new(InodeInner::new(inode_alloc(), 0, mode, 0)),
            this_mode: mode,
            link_target: Mutex::new(Some(String::from(target))),
            xattrs: Mutex::new(BTreeMap::new()),
        }
    }

    /// Create a special file inode (device, fifo, socket) with the given device number.
    pub fn new_dev(mode: InodeMode, rdev: usize) -> Self {
        Self {
            inner: Mutex::new(InodeInner::new(inode_alloc(), 0, mode, rdev)),
            this_mode: mode,
            link_target: Mutex::new(None),
            xattrs: Mutex::new(BTreeMap::new()),
        }
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
        crate::fs::page::pagecache::PAGE_CACHE
            .lock()
            .remove_inode_pages(crate::fs::page::pagecache::tagged_inode_id(
                crate::fs::page::pagecache::PAGE_CACHE_FS_TMPFS,
                self.get_ino(),
            ));
        Ok(0)
    }

    fn get_ino(&self) -> usize {
        self.inner.lock().ino
    }

    fn cache_inode_id(&self) -> Option<usize> {
        Some(crate::fs::page::pagecache::tagged_inode_id(
            crate::fs::page::pagecache::PAGE_CACHE_FS_TMPFS,
            self.get_ino(),
        ))
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

    fn setxattr(&self, name: &str, value: &[u8], flags: i32) -> SyscallResult {
        const XATTR_NAME_MAX: usize = 255;
        const XATTR_SIZE_MAX: usize = 65536;
        const XATTR_CREATE: i32 = 1;
        const XATTR_REPLACE: i32 = 2;

        if flags & !(XATTR_CREATE | XATTR_REPLACE) != 0 {
            return Err(SysError::EINVAL);
        }
        if name.is_empty() {
            return Err(SysError::ERANGE);
        }
        if name.len() > XATTR_NAME_MAX {
            return Err(SysError::ERANGE);
        }
        if value.len() > XATTR_SIZE_MAX {
            return Err(SysError::E2BIG);
        }

        let mut xattrs = self.xattrs.lock();
        match flags {
            XATTR_CREATE => {
                if xattrs.contains_key(name) {
                    return Err(SysError::EEXIST);
                }
                xattrs.insert(name.to_string(), value.to_vec());
            }
            XATTR_REPLACE => {
                if !xattrs.contains_key(name) {
                    return Err(SysError::ENODATA);
                }
                xattrs.insert(name.to_string(), value.to_vec());
            }
            _ => {
                xattrs.insert(name.to_string(), value.to_vec());
            }
        }
        Ok(0)
    }

    fn getxattr(&self, name: &str, buf: &mut [u8]) -> SyscallResult {
        let xattrs = self.xattrs.lock();
        match xattrs.get(name) {
            Some(value) => {
                let len = value.len();
                if !buf.is_empty() {
                    let copy_len = len.min(buf.len());
                    buf[..copy_len].copy_from_slice(&value[..copy_len]);
                }
                Ok(len)
            }
            None => Err(SysError::ENODATA),
        }
    }

    fn listxattr(&self, buf: &mut [u8]) -> SyscallResult {
        let xattrs = self.xattrs.lock();
        let mut total = 0usize;
        for name in xattrs.keys() {
            let name_bytes = name.as_bytes();
            let entry_len = name_bytes.len() + 1; // include '\0'
            if !buf.is_empty() {
                if total + entry_len > buf.len() {
                    return Err(SysError::ERANGE);
                }
                buf[total..total + name_bytes.len()].copy_from_slice(name_bytes);
                buf[total + name_bytes.len()] = 0;
            }
            total += entry_len;
        }
        Ok(total)
    }

    fn removexattr(&self, name: &str) -> SyscallResult {
        let mut xattrs = self.xattrs.lock();
        if xattrs.remove(name).is_some() {
            Ok(0)
        } else {
            Err(SysError::ENODATA)
        }
    }
}
