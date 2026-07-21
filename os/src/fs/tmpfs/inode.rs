use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::File;
use crate::fs::Inode;
use crate::fs::vfs::inode::inode_alloc;
use crate::fs::vfs::inode::{
    InodeInner, InodeMode, XATTR_CREATE, XATTR_NAME_MAX, XATTR_REPLACE, XATTR_SIZE_MAX,
    check_user_xattr_support, check_xattr_write_allowed, note_punched_hole_inserted,
    note_punched_holes_removed,
};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use log::info;
use lwext4_rust::InodeTypes;
use spin::mutex::Mutex;

// memfd seal flags
///
pub const F_SEAL_SEAL: u64 = 0x0001; // prevent further seal changes
///
pub const F_SEAL_SHRINK: u64 = 0x0002; // prevent shrinking
///
pub const F_SEAL_GROW: u64 = 0x0004; // prevent growing
///
pub const F_SEAL_WRITE: u64 = 0x0008; // prevent writes

static TMPFS_INODE_CREATED: AtomicUsize = AtomicUsize::new(0);
static TMPFS_INODE_DROPPED: AtomicUsize = AtomicUsize::new(0);
static TMPFS_INODE_CURRENT: AtomicUsize = AtomicUsize::new(0);
static TMPFS_FILE_INODES: AtomicUsize = AtomicUsize::new(0);
static TMPFS_DIR_INODES: AtomicUsize = AtomicUsize::new(0);
static TMPFS_LINK_INODES: AtomicUsize = AtomicUsize::new(0);
static TMPFS_SPECIAL_INODES: AtomicUsize = AtomicUsize::new(0);
static TMPFS_XATTR_CURRENT: AtomicUsize = AtomicUsize::new(0);
static TMPFS_XATTR_BYTES: AtomicUsize = AtomicUsize::new(0);
static TMPFS_XATTR_SET_COUNT: AtomicUsize = AtomicUsize::new(0);
static TMPFS_XATTR_REMOVE_COUNT: AtomicUsize = AtomicUsize::new(0);
static TMPFS_SYMLINK_BYTES: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Copy)]
/// Snapshot of tmpfs inode and xattr metadata retained on the kernel heap.
pub struct TmpfsInodeStats {
    /// Total tmpfs inodes created since boot.
    pub created: usize,
    /// Total tmpfs inodes dropped since boot.
    pub dropped: usize,
    /// Tmpfs inodes currently alive.
    pub current: usize,
    /// Alive regular-file tmpfs inodes.
    pub file_inodes: usize,
    /// Alive directory tmpfs inodes.
    pub dir_inodes: usize,
    /// Alive symlink tmpfs inodes.
    pub link_inodes: usize,
    /// Alive tmpfs special-file inodes.
    pub special_inodes: usize,
    /// Alive tmpfs extended-attribute entries.
    pub xattrs: usize,
    /// Approximate bytes retained by tmpfs xattr names and values.
    pub xattr_bytes: usize,
    /// Total successful tmpfs xattr set operations.
    pub xattr_set_count: usize,
    /// Total successful tmpfs xattr remove operations.
    pub xattr_remove_count: usize,
    /// Approximate bytes retained by tmpfs symlink targets.
    pub symlink_bytes: usize,
}

/// Return lock-free tmpfs inode and xattr retention counters.
pub fn tmpfs_inode_stats() -> TmpfsInodeStats {
    TmpfsInodeStats {
        created: TMPFS_INODE_CREATED.load(Ordering::Relaxed),
        dropped: TMPFS_INODE_DROPPED.load(Ordering::Relaxed),
        current: TMPFS_INODE_CURRENT.load(Ordering::Relaxed),
        file_inodes: TMPFS_FILE_INODES.load(Ordering::Relaxed),
        dir_inodes: TMPFS_DIR_INODES.load(Ordering::Relaxed),
        link_inodes: TMPFS_LINK_INODES.load(Ordering::Relaxed),
        special_inodes: TMPFS_SPECIAL_INODES.load(Ordering::Relaxed),
        xattrs: TMPFS_XATTR_CURRENT.load(Ordering::Relaxed),
        xattr_bytes: TMPFS_XATTR_BYTES.load(Ordering::Relaxed),
        xattr_set_count: TMPFS_XATTR_SET_COUNT.load(Ordering::Relaxed),
        xattr_remove_count: TMPFS_XATTR_REMOVE_COUNT.load(Ordering::Relaxed),
        symlink_bytes: TMPFS_SYMLINK_BYTES.load(Ordering::Relaxed),
    }
}

fn tmpfs_inode_type_counter(mode: InodeMode) -> &'static AtomicUsize {
    let ty = mode & InodeMode::TYPE_MASK;
    if ty == InodeMode::FILE {
        &TMPFS_FILE_INODES
    } else if ty == InodeMode::DIR {
        &TMPFS_DIR_INODES
    } else if ty == InodeMode::LINK {
        &TMPFS_LINK_INODES
    } else {
        &TMPFS_SPECIAL_INODES
    }
}

fn note_tmpfs_inode_created(mode: InodeMode) {
    TMPFS_INODE_CREATED.fetch_add(1, Ordering::Relaxed);
    TMPFS_INODE_CURRENT.fetch_add(1, Ordering::Relaxed);
    tmpfs_inode_type_counter(mode).fetch_add(1, Ordering::Relaxed);
}

fn note_tmpfs_inode_dropped(mode: InodeMode) {
    TMPFS_INODE_DROPPED.fetch_add(1, Ordering::Relaxed);
    TMPFS_INODE_CURRENT.fetch_sub(1, Ordering::Relaxed);
    tmpfs_inode_type_counter(mode).fetch_sub(1, Ordering::Relaxed);
}

fn xattr_account_bytes(name: &str, value: &[u8]) -> usize {
    name.len() + value.len()
}

#[allow(unused)]
/// the inode of tempfs
pub struct TempInode {
    inner: Mutex<InodeInner>,
    this_mode: InodeMode,
    link_target: Mutex<Option<String>>,
    xattrs: Mutex<BTreeMap<String, Vec<u8>>>,
    seals: AtomicU64,
}

impl TempInode {
    ///
    pub fn new(mode: InodeMode) -> Self {
        note_tmpfs_inode_created(mode);
        Self {
            inner: Mutex::new(InodeInner::new(inode_alloc(), 0, mode, 0)),
            this_mode: mode,
            link_target: Mutex::new(None),
            xattrs: Mutex::new(BTreeMap::new()),
            seals: AtomicU64::new(0),
        }
    }

    /// Create a symlink inode with the given target.
    pub fn new_symlink(target: &str) -> Self {
        let mode = InodeMode::from_bits_truncate(0o777) | InodeMode::LINK;
        note_tmpfs_inode_created(mode);
        TMPFS_SYMLINK_BYTES.fetch_add(target.len(), Ordering::Relaxed);
        Self {
            inner: Mutex::new(InodeInner::new(inode_alloc(), 0, mode, 0)),
            this_mode: mode,
            link_target: Mutex::new(Some(String::from(target))),
            xattrs: Mutex::new(BTreeMap::new()),
            seals: AtomicU64::new(0),
        }
    }

    /// Create a special file inode (device, fifo, socket) with the given device number.
    pub fn new_dev(mode: InodeMode, rdev: usize) -> Self {
        note_tmpfs_inode_created(mode);
        Self {
            inner: Mutex::new(InodeInner::new(inode_alloc(), 0, mode, rdev)),
            this_mode: mode,
            link_target: Mutex::new(None),
            xattrs: Mutex::new(BTreeMap::new()),
            seals: AtomicU64::new(0),
        }
    }

    /// Check if a seal is set
    pub fn has_seal(&self, seal: u64) -> bool {
        (self.seals.load(Ordering::Relaxed) & seal) != 0
    }
}

impl Drop for TempInode {
    fn drop(&mut self) {
        note_tmpfs_inode_dropped(self.this_mode);
        if let Some(target) = self.link_target.lock().as_ref() {
            TMPFS_SYMLINK_BYTES.fetch_sub(target.len(), Ordering::Relaxed);
        }
        let xattrs = self.xattrs.lock();
        let mut bytes = 0usize;
        for (name, value) in xattrs.iter() {
            bytes += xattr_account_bytes(name, value);
        }
        if !xattrs.is_empty() {
            TMPFS_XATTR_CURRENT.fetch_sub(xattrs.len(), Ordering::Relaxed);
            TMPFS_XATTR_BYTES.fetch_sub(bytes, Ordering::Relaxed);
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
        self.truncate_punched_holes(size as usize);
        crate::fs::page::pagecache::PAGE_CACHE.remove_inode_pages(
            crate::fs::page::pagecache::tagged_inode_id(
                crate::fs::page::pagecache::PAGE_CACHE_FS_TMPFS,
                self.get_ino(),
            ),
        );
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

    fn get_punched_hole_pages(&self) -> usize {
        self.inner.lock().punched_hole_pages.len()
    }

    fn is_punched_hole_page(&self, page_id: usize) -> bool {
        self.inner.lock().punched_hole_pages.contains(&page_id)
    }

    fn add_punched_hole_page(&self, page_id: usize) {
        if self.inner.lock().punched_hole_pages.insert(page_id) {
            note_punched_hole_inserted();
        }
    }

    fn clear_punched_hole_page(&self, page_id: usize) {
        if self.inner.lock().punched_hole_pages.remove(&page_id) {
            note_punched_holes_removed(1);
        }
    }

    fn clear_punched_holes(&self) {
        let mut inner = self.inner.lock();
        let removed = inner.punched_hole_pages.len();
        inner.punched_hole_pages.clear();
        note_punched_holes_removed(removed);
    }

    fn truncate_punched_holes(&self, size: usize) {
        let first_invalid_page = size.div_ceil(polyhal::consts::PAGE_SIZE);
        let mut inner = self.inner.lock();
        let removed = inner.punched_hole_pages.split_off(&first_invalid_page);
        note_punched_holes_removed(removed.len());
    }

    fn get_size(&self) -> usize {
        self.inner.lock().size.load(Ordering::Relaxed)
    }

    fn set_size(&self, new_size: usize) {
        self.inner.lock().size.store(new_size, Ordering::Relaxed);
    }

    fn extend_size(&self, minimum_size: usize) -> usize {
        let inner = self.inner.lock();
        let current = inner.size.load(Ordering::Relaxed);
        let resulting_size = current.max(minimum_size);
        if resulting_size != current {
            inner.size.store(resulting_size, Ordering::Relaxed);
        }
        resulting_size
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
    fn get_fs_flags(&self) -> u32 {
        self.inner.lock().fs_flags.load(Ordering::Relaxed) as u32
    }
    fn set_fs_flags(&self, flags: u32) {
        self.inner
            .lock()
            .fs_flags
            .store(flags as usize, Ordering::Relaxed);
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
        check_xattr_write_allowed(self.get_fs_flags())?;
        if name.starts_with("user.") {
            check_user_xattr_support(self.get_mode())?;
        }

        let mut xattrs = self.xattrs.lock();
        match flags {
            XATTR_CREATE => {
                if xattrs.contains_key(name) {
                    return Err(SysError::EEXIST);
                }
                let new_name = name.to_string();
                let new_value = value.to_vec();
                let new_bytes = xattr_account_bytes(&new_name, &new_value);
                xattrs.insert(new_name, new_value);
                TMPFS_XATTR_CURRENT.fetch_add(1, Ordering::Relaxed);
                TMPFS_XATTR_BYTES.fetch_add(new_bytes, Ordering::Relaxed);
            }
            XATTR_REPLACE => {
                let old_bytes = xattrs
                    .get(name)
                    .map(|old| xattr_account_bytes(name, old))
                    .ok_or(SysError::ENODATA)?;
                let new_name = name.to_string();
                let new_value = value.to_vec();
                let new_bytes = xattr_account_bytes(&new_name, &new_value);
                xattrs.insert(new_name, new_value);
                TMPFS_XATTR_BYTES.fetch_sub(old_bytes, Ordering::Relaxed);
                TMPFS_XATTR_BYTES.fetch_add(new_bytes, Ordering::Relaxed);
            }
            _ => {
                let old_bytes = xattrs.get(name).map(|old| xattr_account_bytes(name, old));
                let new_name = name.to_string();
                let new_value = value.to_vec();
                let new_bytes = xattr_account_bytes(&new_name, &new_value);
                xattrs.insert(new_name, new_value);
                if let Some(old_bytes) = old_bytes {
                    TMPFS_XATTR_BYTES.fetch_sub(old_bytes, Ordering::Relaxed);
                } else {
                    TMPFS_XATTR_CURRENT.fetch_add(1, Ordering::Relaxed);
                }
                TMPFS_XATTR_BYTES.fetch_add(new_bytes, Ordering::Relaxed);
            }
        }
        TMPFS_XATTR_SET_COUNT.fetch_add(1, Ordering::Relaxed);
        Ok(0)
    }

    fn getxattr(&self, name: &str, buf: &mut [u8]) -> SyscallResult {
        let xattrs = self.xattrs.lock();
        match xattrs.get(name) {
            Some(value) => {
                let len = value.len();
                if !buf.is_empty() {
                    if buf.len() < len {
                        return Err(SysError::ERANGE);
                    }
                    buf[..len].copy_from_slice(value);
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
        let value = xattrs.remove(name).ok_or(SysError::ENODATA)?;
        TMPFS_XATTR_CURRENT.fetch_sub(1, Ordering::Relaxed);
        TMPFS_XATTR_BYTES.fetch_sub(xattr_account_bytes(name, &value), Ordering::Relaxed);
        TMPFS_XATTR_REMOVE_COUNT.fetch_add(1, Ordering::Relaxed);
        Ok(0)
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
