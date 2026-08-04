use core::cell::RefCell;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::fs::page::pagecache::{PAGE_CACHE, PAGE_CACHE_FS_EXT4, tagged_inode_id};
use crate::fs::vfs::inode::{
    InodeMode, PageCacheInvalidationGuard, XATTR_CREATE, XATTR_NAME_MAX, XATTR_REPLACE,
    XATTR_SIZE_MAX, check_user_xattr_support, check_xattr_write_allowed,
    note_punched_hole_inserted, note_punched_holes_removed,
};
use alloc::collections::BTreeMap;
use alloc::ffi::CString;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec;
use alloc::vec::Vec;
use lazy_static::lazy_static;
use log::*;
use spin::mutex::Mutex;

use lwext4_rust::{
    Ext4BlockWrapper, InodeTypes, KernelDevOp, Lwext4File,
    bindings::{
        O_APPEND, O_CREAT, O_RDONLY, O_RDWR, O_TRUNC, O_WRONLY, SEEK_CUR, SEEK_END, SEEK_SET,
        ext4_getxattr, ext4_inode_stat, ext4_listxattr, ext4_removexattr, ext4_setxattr,
    },
};

use virtio_drivers::{
    device::blk::VirtIOBlk,
    transport::{
        DeviceType, Transport,
        mmio::{MmioTransport, VirtIOHeader},
    },
};

use crate::config::BLOCK_SIZE;
use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::vfs::inode::{Inode, InodeInner};
use crate::fs::vfs::kstat::Kstat;
use crate::logging;

use super::disk::Disk;
use super::ext4::file::ExtFS;
use super::{Lwext4Op, with_lwext4_path_lock_op};
#[allow(unused)]
///The inode of the Ext4 filesystem
/// the InodeInner is ino
/// this_type is the InodeTypes
pub struct Ext4Inode {
    shared: Arc<Ext4InodeSharedState>,
    this_type: InodeTypes,
    path: String,
    registry_inode_id: usize,
    cache_inode_id: usize,
}

struct Ext4InodeSharedState {
    inner: Mutex<InodeInner>,
    disk_initialized: AtomicBool,
    /// Symlink contents are immutable for the lifetime of an inode. Rename
    /// changes only the directory entry, while replacement gets a new inode
    /// incarnation, so this cache needs no generation-based invalidation.
    symlink_target: Mutex<Option<String>>,
    /// Per-inode invalidation sequence for cached on-disk `stat` fields.
    /// Keeping this separate from the mount generation prevents writes to a
    /// compiler output from invalidating every dependency's metadata cache.
    metadata_cache_generation: AtomicUsize,
    page_cache_generation: AtomicUsize,
    /// High bit: one writeback owner is excluding new cached writers.
    /// Remaining bits: cached writers that entered before that owner.
    page_cache_write_state: AtomicUsize,
    retired: AtomicBool,
}

struct Ext4InodeRegistryEntry {
    shared: Weak<Ext4InodeSharedState>,
    cache_inode_id: usize,
}

lazy_static! {
    static ref EXT4_INODE_SHARED_STATES: [Mutex<BTreeMap<usize, Ext4InodeRegistryEntry>>; EXT4_INODE_STATE_SHARDS] =
        core::array::from_fn(|_| Mutex::new(BTreeMap::new()));
}

const EXT4_INODE_STATE_SHARDS: usize = 64;
const EXT4_CACHE_INSTANCE_MASK: usize = (1usize << 60) - 1;
const EXT4_PAGE_CACHE_WRITEBACK_BIT: usize = 1usize << (usize::BITS - 1);
const EXT4_PAGE_CACHE_WRITER_MASK: usize = EXT4_PAGE_CACHE_WRITEBACK_BIT - 1;
static EXT4_CACHE_INSTANCE_SEQUENCE: AtomicUsize = AtomicUsize::new(1);

#[inline]
fn ext4_inode_state_shard(cache_inode_id: usize) -> usize {
    let mixed = cache_inode_id ^ (cache_inode_id >> 17) ^ (cache_inode_id >> 31);
    mixed & (EXT4_INODE_STATE_SHARDS - 1)
}

fn allocate_ext4_cache_inode_id() -> usize {
    let instance = EXT4_CACHE_INSTANCE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    assert!(
        instance != 0 && instance <= EXT4_CACHE_INSTANCE_MASK,
        "ext4 page-cache instance id exhausted"
    );
    tagged_inode_id(PAGE_CACHE_FS_EXT4, instance)
}

unsafe impl Send for Ext4Inode {}
unsafe impl Sync for Ext4Inode {}

impl Ext4Inode {
    ///
    pub fn new(ino: usize, types: InodeTypes, path: String, mount_id: usize) -> Self {
        info!("Inode new {:?} with ino {}", types, ino);
        let mode = InodeMode::from_inode_type(types.clone());
        let registry_inode_id = ((mount_id & 0x0fff_ffff) << 32) | (ino & 0xffff_ffff);
        let (shared, cache_inode_id) = {
            let mut states =
                EXT4_INODE_SHARED_STATES[ext4_inode_state_shard(registry_inode_id)].lock();
            if let Some(entry) = states.get_mut(&registry_inode_id) {
                if let Some(shared) = entry.shared.upgrade() {
                    (shared, entry.cache_inode_id)
                } else {
                    // The linked inode temporarily has no VFS wrapper. Keep
                    // its cache incarnation so clean/dirty cached pages remain
                    // associated with the same on-disk object when reopened.
                    let shared = Arc::new(Ext4InodeSharedState {
                        inner: Mutex::new(InodeInner::new(ino, 0, mode, 0)),
                        disk_initialized: AtomicBool::new(false),
                        symlink_target: Mutex::new(None),
                        metadata_cache_generation: AtomicUsize::new(0),
                        page_cache_generation: AtomicUsize::new(0),
                        page_cache_write_state: AtomicUsize::new(0),
                        retired: AtomicBool::new(false),
                    });
                    entry.shared = Arc::downgrade(&shared);
                    (shared, entry.cache_inode_id)
                }
            } else {
                let cache_inode_id = allocate_ext4_cache_inode_id();
                let shared = Arc::new(Ext4InodeSharedState {
                    inner: Mutex::new(InodeInner::new(ino, 0, mode, 0)),
                    disk_initialized: AtomicBool::new(false),
                    symlink_target: Mutex::new(None),
                    metadata_cache_generation: AtomicUsize::new(0),
                    page_cache_generation: AtomicUsize::new(0),
                    page_cache_write_state: AtomicUsize::new(0),
                    retired: AtomicBool::new(false),
                });
                states.insert(registry_inode_id, Ext4InodeRegistryEntry {
                    shared: Arc::downgrade(&shared),
                    cache_inode_id,
                });
                (shared, cache_inode_id)
            }
        };

        Self {
            shared,
            this_type: types,
            path,
            registry_inode_id,
            cache_inode_id,
        }
    }

    /// Initialize the VFS inode cache from authoritative on-disk metadata.
    pub fn sync_from_disk_stat(&self, stat: &ext4_inode_stat) {
        let first_sync = self
            .shared
            .disk_initialized
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        let mut inner = self.shared.inner.lock();
        inner.ino = stat.ino as usize;
        let vfs_size_is_authoritative =
            self.shared.page_cache_generation.load(Ordering::Acquire) != 0;
        if first_sync {
            inner.size.store(stat.size as usize, Ordering::Relaxed);
        } else if !vfs_size_is_authoritative {
            // Before the first destructive size operation, a newly discovered
            // dirty inode may be ahead of disk. Preserve that larger VFS size.
            inner.size.fetch_max(stat.size as usize, Ordering::Relaxed);
        }
        inner.nlink.store(stat.nlink as usize, Ordering::Relaxed);
        inner.mode = InodeMode::from_bits_truncate(stat.mode);
        inner.uid.store(stat.uid as usize, Ordering::Relaxed);
        inner.gid.store(stat.gid as usize, Ordering::Relaxed);
        inner.rdev.store(stat.rdev as usize, Ordering::Relaxed);
        inner.atime_sec.store(stat.atime as i64, Ordering::Relaxed);
        inner.atime_nsec.store(0, Ordering::Relaxed);
        inner.mtime_sec.store(stat.mtime as i64, Ordering::Relaxed);
        inner.mtime_nsec.store(0, Ordering::Relaxed);
        inner.ctime_sec.store(stat.ctime as i64, Ordering::Relaxed);
        inner.ctime_nsec.store(0, Ordering::Relaxed);
        inner.fs_flags.store(stat.flags as usize, Ordering::Relaxed);
        drop(inner);
        self.note_metadata_change();
    }

    fn has_xattr(&self, name: &str) -> SysResult<bool> {
        let cpath = CString::new(self.path.clone()).map_err(|_| SysError::EINVAL)?;
        let mut list_size = 0usize;
        let ret = with_lwext4_path_lock_op(&self.path, Lwext4Op::Xattr, || unsafe {
            ext4_listxattr(cpath.as_ptr(), core::ptr::null_mut(), 0, &mut list_size)
        })?;
        if ret != 0 {
            return Err(super::lwext4_err_to_sys(ret));
        }
        if list_size == 0 {
            return Ok(false);
        }

        let mut list = vec![0u8; list_size];
        let ret = with_lwext4_path_lock_op(&self.path, Lwext4Op::Xattr, || unsafe {
            ext4_listxattr(
                cpath.as_ptr(),
                list.as_mut_ptr() as *mut core::ffi::c_char,
                list.len(),
                &mut list_size,
            )
        })?;
        if ret != 0 {
            return Err(super::lwext4_err_to_sys(ret));
        }

        Ok(list[..list_size]
            .split(|byte| *byte == 0)
            .any(|entry| entry == name.as_bytes()))
    }
}

/// Combine fresh ext4 allocation metadata with mutable VFS inode state.
pub fn fill_ext4_kstat(inode: &dyn Inode, disk: &ext4_inode_stat, stat: &mut Kstat) {
    stat.st_ino = disk.ino as u64;
    stat.st_nlink = disk.nlink;
    stat.st_size = if inode.get_mode().get_type() == InodeMode::FILE {
        inode.get_size() as i64
    } else {
        disk.size as i64
    };
    stat.st_mode = inode.get_mode().bits();
    stat.st_uid = inode.get_uid() as u32;
    stat.st_gid = inode.get_gid() as u32;
    stat.st_rdev = inode.get_rdev() as u64;
    stat.st_blksize = disk.block_size as i32;
    stat.st_blocks = disk
        .blocks
        .saturating_sub(inode.get_punched_hole_pages() as u64 * 8);
    stat.st_fs_flags = inode.get_fs_flags();

    let (atime_sec, atime_nsec) = inode.get_atime();
    let (mtime_sec, mtime_nsec) = inode.get_mtime();
    let (ctime_sec, ctime_nsec) = inode.get_ctime();
    stat.st_atime_sec = atime_sec;
    stat.st_atime_nsec = atime_nsec;
    stat.st_mtime_sec = mtime_sec;
    stat.st_mtime_nsec = mtime_nsec;
    stat.st_ctime_sec = ctime_sec;
    stat.st_ctime_nsec = ctime_nsec;
}

impl Inode for Ext4Inode {
    /// Get the attributes of the file, such as size, permissions, etc.
    fn get_attr(&self) -> SysResult<usize> {
        unimplemented!()
    }
    /// Flush the file, synchronize the data to disk.
    fn fsync(&self) -> SysResult<usize> {
        unimplemented!()
    }
    fn truncate(&self, size: u64) -> SysResult<usize> {
        let invalidation = PageCacheInvalidationGuard::new(self);
        self.set_size(size as usize);
        self.truncate_punched_holes(size as usize);
        // 截断文件时清除该 inode 的页缓存，避免旧页面被后续写入/读取误用
        PAGE_CACHE.remove_inode_pages(self.cache_inode_id);
        invalidation.commit();
        // 注意：实际的 ext4 文件截断由 Ext4File::new() 中的 O_TRUNC 标志完成，
        // 或者由 Ext4File::truncate() 方法完成。
        // 这里只更新 in-memory 状态和清除页缓存。
        Ok(0)
    }
    ///
    fn get_types(&self) -> InodeTypes {
        match self.this_type {
            InodeTypes::EXT4_DE_REG_FILE => InodeTypes::EXT4_DE_REG_FILE,
            InodeTypes::EXT4_DE_DIR => InodeTypes::EXT4_DE_DIR,
            InodeTypes::EXT4_DE_SYMLINK => InodeTypes::EXT4_DE_SYMLINK,
            InodeTypes::EXT4_DE_CHRDEV => InodeTypes::EXT4_DE_CHRDEV,
            InodeTypes::EXT4_DE_BLKDEV => InodeTypes::EXT4_DE_BLKDEV,
            InodeTypes::EXT4_DE_FIFO => InodeTypes::EXT4_DE_FIFO,
            InodeTypes::EXT4_DE_SOCK => InodeTypes::EXT4_DE_SOCK,
            _ => {
                warn!("Unsupported InodeType: {:?}", self.this_type);
                InodeTypes::EXT4_DE_UNKNOWN
            }
        }
    }

    fn readlink(&self) -> Result<String, i32> {
        crate::task::processor::record_current_syscall_stage_nolock(78, 78410);
        if self.this_type != InodeTypes::EXT4_DE_SYMLINK {
            return Err(-22);
        }
        if let Some(target) = self.shared.symlink_target.lock().as_ref().cloned() {
            crate::task::processor::record_current_syscall_stage_nolock(78, 78412);
            return Ok(target);
        }
        let cpath = CString::new(self.path.clone()).map_err(|_| -22)?;
        let mut buf = vec![0u8; 4096];
        crate::task::processor::record_current_syscall_stage_nolock(78, 78411);
        let started_ns = polyhal::timer::current_time().as_nanos() as usize;
        let result = ExtFS::readlink(&cpath, &mut buf);
        let elapsed_ns =
            (polyhal::timer::current_time().as_nanos() as usize).saturating_sub(started_ns);
        if elapsed_ns >= 10_000_000 {
            log::error!(
                "[READLINKAT_LWEXT4_SLOW] cpu={} step=readlink elapsed_ns={} path={} outcome={} lock={:?}",
                polyhal::arch::hart_id(),
                elapsed_ns,
                self.path,
                if result.is_ok() { "ok" } else { "error" },
                crate::fs::lwext4::lwext4_lock_stats(),
            );
        }
        crate::task::processor::record_current_syscall_stage_nolock(78, 78412);
        match result {
            Ok(len) => {
                buf.truncate(len);
                let target = String::from_utf8_lossy(&buf).into_owned();
                let mut cached = self.shared.symlink_target.lock();
                if let Some(existing) = cached.as_ref() {
                    Ok(existing.clone())
                } else {
                    *cached = Some(target.clone());
                    Ok(target)
                }
            }
            Err(e) => Err(e.code() as i32),
        }
    }
    fn get_ino(&self) -> usize {
        self.shared.inner.lock().ino
    }

    fn cache_inode_id(&self) -> Option<usize> {
        Some(self.cache_inode_id)
    }

    fn metadata_cache_generation(&self) -> usize {
        self.shared
            .metadata_cache_generation
            .load(Ordering::Acquire)
    }

    fn note_metadata_change(&self) {
        self.shared
            .metadata_cache_generation
            .fetch_add(1, Ordering::AcqRel);
    }

    fn retire_page_cache_identity(&self) {
        if self.shared.retired.swap(true, Ordering::AcqRel) {
            return;
        }
        let ino = self.get_ino();
        let mut states =
            EXT4_INODE_SHARED_STATES[ext4_inode_state_shard(self.registry_inode_id)].lock();
        let is_current_incarnation = states.get(&self.registry_inode_id).is_some_and(|entry| {
            entry.cache_inode_id == self.cache_inode_id
                && entry.shared.ptr_eq(&Arc::downgrade(&self.shared))
        });
        if is_current_incarnation {
            states.remove(&self.registry_inode_id);
        }
        info!(
            "[EXT4_INODE_CACHE_RETIRE] inode={} cache_inode_id={:#x}",
            ino, self.cache_inode_id
        );
    }

    fn page_cache_generation(&self) -> usize {
        self.shared.page_cache_generation.load(Ordering::Acquire)
    }

    fn begin_page_cache_write(&self) {
        loop {
            let state = self.shared.page_cache_write_state.load(Ordering::Acquire);
            if state & EXT4_PAGE_CACHE_WRITEBACK_BIT != 0 {
                crate::task::suspend_current_and_run_next();
                continue;
            }
            debug_assert!(state < EXT4_PAGE_CACHE_WRITER_MASK);
            if self
                .shared
                .page_cache_write_state
                .compare_exchange_weak(state, state + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return;
            }
        }
    }

    fn end_page_cache_write(&self) {
        let previous = self
            .shared
            .page_cache_write_state
            .fetch_sub(1, Ordering::AcqRel);
        debug_assert_ne!(previous & EXT4_PAGE_CACHE_WRITER_MASK, 0);
    }

    fn begin_page_cache_writeback(&self) {
        loop {
            let state = self.shared.page_cache_write_state.load(Ordering::Acquire);
            if state & EXT4_PAGE_CACHE_WRITEBACK_BIT != 0 {
                crate::task::suspend_current_and_run_next();
                continue;
            }
            if self
                .shared
                .page_cache_write_state
                .compare_exchange_weak(
                    state,
                    state | EXT4_PAGE_CACHE_WRITEBACK_BIT,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_err()
            {
                continue;
            }
            while self.shared.page_cache_write_state.load(Ordering::Acquire)
                != EXT4_PAGE_CACHE_WRITEBACK_BIT
            {
                crate::task::suspend_current_and_run_next();
            }
            return;
        }
    }

    fn end_page_cache_writeback(&self) {
        let result = self.shared.page_cache_write_state.compare_exchange(
            EXT4_PAGE_CACHE_WRITEBACK_BIT,
            0,
            Ordering::Release,
            Ordering::Relaxed,
        );
        debug_assert!(result.is_ok());
    }

    fn begin_page_cache_invalidation(&self) -> usize {
        loop {
            let stable_generation = self.shared.page_cache_generation.load(Ordering::Acquire);
            if stable_generation & 1 != 0 {
                crate::task::suspend_current_and_run_next();
                continue;
            }
            let invalidating_generation = stable_generation.wrapping_add(1);
            if self
                .shared
                .page_cache_generation
                .compare_exchange_weak(
                    stable_generation,
                    invalidating_generation,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                return invalidating_generation;
            }
        }
    }

    fn end_page_cache_invalidation(&self) -> usize {
        loop {
            let invalidating_generation = self.shared.page_cache_generation.load(Ordering::Acquire);
            assert_eq!(
                invalidating_generation & 1,
                1,
                "ending inactive ext4 cache invalidation"
            );
            let stable_generation = invalidating_generation.wrapping_add(1);
            if self
                .shared
                .page_cache_generation
                .compare_exchange_weak(
                    invalidating_generation,
                    stable_generation,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                return stable_generation;
            }
        }
    }

    fn abort_page_cache_invalidation(&self) -> usize {
        loop {
            let invalidating_generation = self.shared.page_cache_generation.load(Ordering::Acquire);
            assert_eq!(
                invalidating_generation & 1,
                1,
                "aborting inactive ext4 cache invalidation"
            );
            let stable_generation = invalidating_generation.wrapping_sub(1);
            if self
                .shared
                .page_cache_generation
                .compare_exchange_weak(
                    invalidating_generation,
                    stable_generation,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                return stable_generation;
            }
        }
    }

    fn get_punched_hole_pages(&self) -> usize {
        self.shared.inner.lock().punched_hole_pages.len()
    }

    fn is_punched_hole_page(&self, page_id: usize) -> bool {
        self.shared
            .inner
            .lock()
            .punched_hole_pages
            .contains(&page_id)
    }

    fn add_punched_hole_page(&self, page_id: usize) {
        if self.shared.inner.lock().punched_hole_pages.insert(page_id) {
            note_punched_hole_inserted();
        }
    }

    fn clear_punched_hole_page(&self, page_id: usize) {
        if self.shared.inner.lock().punched_hole_pages.remove(&page_id) {
            note_punched_holes_removed(1);
        }
    }

    fn clear_punched_holes(&self) {
        let mut inner = self.shared.inner.lock();
        let removed = inner.punched_hole_pages.len();
        inner.punched_hole_pages.clear();
        note_punched_holes_removed(removed);
    }

    fn truncate_punched_holes(&self, size: usize) {
        let first_invalid_page = size.div_ceil(polyhal::consts::PAGE_SIZE);
        let mut inner = self.shared.inner.lock();
        let removed = inner.punched_hole_pages.split_off(&first_invalid_page);
        note_punched_holes_removed(removed.len());
    }

    fn get_size(&self) -> usize {
        self.shared.inner.lock().size.load(Ordering::Relaxed)
    }

    fn set_size(&self, new_size: usize) {
        let inner = self.shared.inner.lock();
        let old_size = inner.size.swap(new_size, Ordering::Relaxed);
        drop(inner);
        if old_size != new_size {
            self.note_metadata_change();
        }
    }

    fn extend_size(&self, minimum_size: usize) -> usize {
        let inner = self.shared.inner.lock();
        let current = inner.size.load(Ordering::Relaxed);
        let resulting_size = current.max(minimum_size);
        if resulting_size != current {
            inner.size.store(resulting_size, Ordering::Relaxed);
        }
        drop(inner);
        if resulting_size != current {
            self.note_metadata_change();
        }
        resulting_size
    }

    fn replace_size_if_current(&self, expected_size: usize, replacement_size: usize) -> bool {
        let inner = self.shared.inner.lock();
        if inner.size.load(Ordering::Relaxed) != expected_size {
            return false;
        }
        inner.size.store(replacement_size, Ordering::Relaxed);
        drop(inner);
        if replacement_size != expected_size {
            self.note_metadata_change();
        }
        true
    }

    fn get_nlink(&self) -> usize {
        self.shared.inner.lock().nlink.load(Ordering::Relaxed)
    }
    fn get_rdev(&self) -> usize {
        self.shared.inner.lock().rdev.load(Ordering::Relaxed)
    }
    fn set_rdev(&self, rdev: usize) {
        let inner = self.shared.inner.lock();
        let old = inner.rdev.swap(rdev, Ordering::Relaxed);
        drop(inner);
        if old != rdev {
            self.note_metadata_change();
        }
    }
    fn get_fs_flags(&self) -> u32 {
        self.shared.inner.lock().fs_flags.load(Ordering::Relaxed) as u32
    }
    fn set_fs_flags(&self, flags: u32) {
        let inner = self.shared.inner.lock();
        let old = inner.fs_flags.swap(flags as usize, Ordering::Relaxed);
        drop(inner);
        if old != flags as usize {
            self.note_metadata_change();
        }
    }

    fn get_mode(&self) -> InodeMode {
        self.shared.inner.lock().mode
    }
    fn set_mode(&self, mode: InodeMode) {
        let mut inner = self.shared.inner.lock();
        let old = inner.mode;
        inner.mode = mode;
        drop(inner);
        if old != mode {
            self.note_metadata_change();
        }
    }
    fn get_uid(&self) -> usize {
        self.shared.inner.lock().uid.load(Ordering::Relaxed)
    }
    fn set_uid(&self, uid: usize) {
        let inner = self.shared.inner.lock();
        let old = inner.uid.swap(uid, Ordering::Relaxed);
        drop(inner);
        if old != uid {
            self.note_metadata_change();
        }
    }
    fn get_gid(&self) -> usize {
        self.shared.inner.lock().gid.load(Ordering::Relaxed)
    }
    fn set_gid(&self, gid: usize) {
        let inner = self.shared.inner.lock();
        let old = inner.gid.swap(gid, Ordering::Relaxed);
        drop(inner);
        if old != gid {
            self.note_metadata_change();
        }
    }
    fn inc_nlink(&self) {
        self.shared
            .inner
            .lock()
            .nlink
            .fetch_add(1, Ordering::SeqCst);
        self.note_metadata_change();
    }

    fn dec_nlink(&self) {
        self.shared
            .inner
            .lock()
            .nlink
            .fetch_sub(1, Ordering::SeqCst);
        self.note_metadata_change();
    }

    fn get_atime(&self) -> (i64, i64) {
        let inner = self.shared.inner.lock();
        (
            inner.atime_sec.load(Ordering::Relaxed),
            inner.atime_nsec.load(Ordering::Relaxed),
        )
    }

    fn set_atime(&self, sec: i64, nsec: i64) {
        let inner = self.shared.inner.lock();
        let old_sec = inner.atime_sec.load(Ordering::Relaxed);
        let old_nsec = inner.atime_nsec.load(Ordering::Relaxed);
        inner.atime_sec.store(sec, Ordering::Relaxed);
        inner.atime_nsec.store(nsec, Ordering::Relaxed);
        drop(inner);
        if old_sec != sec || old_nsec != nsec {
            self.note_metadata_change();
        }
    }

    fn get_mtime(&self) -> (i64, i64) {
        let inner = self.shared.inner.lock();
        (
            inner.mtime_sec.load(Ordering::Relaxed),
            inner.mtime_nsec.load(Ordering::Relaxed),
        )
    }

    fn set_mtime(&self, sec: i64, nsec: i64) {
        let inner = self.shared.inner.lock();
        let old_sec = inner.mtime_sec.load(Ordering::Relaxed);
        let old_nsec = inner.mtime_nsec.load(Ordering::Relaxed);
        inner.mtime_sec.store(sec, Ordering::Relaxed);
        inner.mtime_nsec.store(nsec, Ordering::Relaxed);
        drop(inner);
        if old_sec != sec || old_nsec != nsec {
            self.note_metadata_change();
        }
    }

    fn get_ctime(&self) -> (i64, i64) {
        let inner = self.shared.inner.lock();
        (
            inner.ctime_sec.load(Ordering::Relaxed),
            inner.ctime_nsec.load(Ordering::Relaxed),
        )
    }

    fn set_ctime(&self, sec: i64, nsec: i64) {
        let inner = self.shared.inner.lock();
        let old_sec = inner.ctime_sec.load(Ordering::Relaxed);
        let old_nsec = inner.ctime_nsec.load(Ordering::Relaxed);
        inner.ctime_sec.store(sec, Ordering::Relaxed);
        inner.ctime_nsec.store(nsec, Ordering::Relaxed);
        drop(inner);
        if old_sec != sec || old_nsec != nsec {
            self.note_metadata_change();
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

        let cpath = CString::new(self.path.clone()).map_err(|_| SysError::EINVAL)?;
        let cname = CString::new(name).map_err(|_| SysError::EINVAL)?;

        match flags {
            XATTR_CREATE => {
                if self.has_xattr(name)? {
                    return Err(SysError::EEXIST);
                }
            }
            XATTR_REPLACE => {
                if !self.has_xattr(name)? {
                    return Err(SysError::ENODATA);
                }
            }
            _ => {}
        }

        let ret = with_lwext4_path_lock_op(&self.path, Lwext4Op::Xattr, || unsafe {
            ext4_setxattr(
                cpath.as_ptr(),
                cname.as_ptr(),
                name.len(),
                value.as_ptr() as *const core::ffi::c_void,
                value.len(),
            )
        })?;
        if ret != 0 {
            return Err(super::lwext4_err_to_sys(ret));
        }
        self.note_metadata_change();
        Ok(0)
    }

    fn getxattr(&self, name: &str, buf: &mut [u8]) -> SyscallResult {
        if name.is_empty() {
            return Err(SysError::ERANGE);
        }
        let cpath = CString::new(self.path.clone()).map_err(|_| SysError::EINVAL)?;
        let cname = CString::new(name).map_err(|_| SysError::EINVAL)?;
        let mut data_size = 0usize;

        if !buf.is_empty() {
            let mut required_size = 0usize;
            let ret = with_lwext4_path_lock_op(&self.path, Lwext4Op::Xattr, || unsafe {
                ext4_getxattr(
                    cpath.as_ptr(),
                    cname.as_ptr(),
                    name.len(),
                    core::ptr::null_mut(),
                    0,
                    &mut required_size,
                )
            })?;
            if ret != 0 {
                return Err(super::lwext4_err_to_sys(ret));
            }
            if buf.len() < required_size {
                return Err(SysError::ERANGE);
            }
        }

        let ret = with_lwext4_path_lock_op(&self.path, Lwext4Op::Xattr, || unsafe {
            ext4_getxattr(
                cpath.as_ptr(),
                cname.as_ptr(),
                name.len(),
                buf.as_mut_ptr() as *mut core::ffi::c_void,
                buf.len(),
                &mut data_size,
            )
        })?;
        if ret != 0 {
            return Err(super::lwext4_err_to_sys(ret));
        }
        Ok(data_size as isize as usize)
    }

    fn listxattr(&self, buf: &mut [u8]) -> SyscallResult {
        let cpath = CString::new(self.path.clone()).map_err(|_| SysError::EINVAL)?;
        let mut ret_size = 0usize;
        let ret = with_lwext4_path_lock_op(&self.path, Lwext4Op::Xattr, || unsafe {
            ext4_listxattr(cpath.as_ptr(), core::ptr::null_mut(), 0, &mut ret_size)
        })?;
        if ret != 0 {
            return Err(super::lwext4_err_to_sys(ret));
        }
        if buf.is_empty() {
            return Ok(ret_size);
        }
        if buf.len() < ret_size {
            return Err(SysError::ERANGE);
        }

        let ret = with_lwext4_path_lock_op(&self.path, Lwext4Op::Xattr, || unsafe {
            ext4_listxattr(
                cpath.as_ptr(),
                buf.as_mut_ptr() as *mut core::ffi::c_char,
                buf.len(),
                &mut ret_size,
            )
        })?;
        if ret != 0 {
            return Err(super::lwext4_err_to_sys(ret));
        }
        if !buf.is_empty() && buf.len() < ret_size {
            return Err(SysError::ERANGE);
        }
        Ok(ret_size)
    }

    fn removexattr(&self, name: &str) -> SyscallResult {
        if name.is_empty() {
            return Err(SysError::ERANGE);
        }
        let cpath = CString::new(self.path.clone()).map_err(|_| SysError::EINVAL)?;
        let cname = CString::new(name).map_err(|_| SysError::EINVAL)?;
        let ret = with_lwext4_path_lock_op(&self.path, Lwext4Op::Xattr, || unsafe {
            ext4_removexattr(cpath.as_ptr(), cname.as_ptr(), name.len())
        })?;
        if ret != 0 {
            return Err(super::lwext4_err_to_sys(ret));
        }
        self.note_metadata_change();
        Ok(0)
    }
}

impl Drop for Ext4Inode {
    fn drop(&mut self) {
        if self.shared.retired.load(Ordering::Acquire) && Arc::strong_count(&self.shared) == 1 {
            let (discarded_writeback, kept_writeback) =
                crate::fs::writeback::discard_closed_inode(self.cache_inode_id);
            debug_assert_eq!(kept_writeback, 0);
            let cached_pages = PAGE_CACHE.inode_pages_count(self.cache_inode_id);
            PAGE_CACHE.remove_inode_pages(self.cache_inode_id);
            self.clear_punched_holes();
            info!(
                "[EXT4_INODE_CACHE_RECLAIM] inode={} cache_inode_id={:#x} cached_pages={} discarded_writeback={} kept_writeback={}",
                self.get_ino(),
                self.cache_inode_id,
                cached_pages,
                discarded_writeback,
                kept_writeback,
            );
        }
    }
}

/// translate between InodeTypes and InodeMode
impl InodeMode {
    /// Convert an InodeTypes to an InodeMode, setting the type bits and permission bits.
    pub fn from_inode_type(itype: InodeTypes) -> Self {
        let perm_mode = InodeMode::OWNER_MASK | InodeMode::GROUP_MASK | InodeMode::OTHER_MASK;
        let file_mode = match itype {
            InodeTypes::EXT4_DE_DIR => InodeMode::DIR,
            InodeTypes::EXT4_DE_REG_FILE => InodeMode::FILE,
            InodeTypes::EXT4_DE_CHRDEV => InodeMode::CHAR,
            InodeTypes::EXT4_DE_FIFO => InodeMode::FIFO,
            InodeTypes::EXT4_DE_BLKDEV => InodeMode::BLOCK,
            InodeTypes::EXT4_DE_SOCK => InodeMode::SOCKET,
            InodeTypes::EXT4_DE_SYMLINK => InodeMode::LINK,
            _ => InodeMode::TYPE_MASK,
        };
        file_mode | perm_mode
    }
    /// Convert an InodeMode to an InodeTypes, extracting the type bits and ignoring the permission bits.
    pub fn to_inode_type(self) -> InodeTypes {
        match self.get_type() {
            InodeMode::DIR => InodeTypes::EXT4_DE_DIR,
            InodeMode::FILE => InodeTypes::EXT4_DE_REG_FILE,
            InodeMode::CHAR => InodeTypes::EXT4_DE_CHRDEV,
            InodeMode::FIFO => InodeTypes::EXT4_DE_FIFO,
            InodeMode::BLOCK => InodeTypes::EXT4_DE_BLKDEV,
            InodeMode::SOCKET => InodeTypes::EXT4_DE_SOCK,
            InodeMode::LINK => InodeTypes::EXT4_DE_SYMLINK,
            _ => InodeTypes::EXT4_DE_UNKNOWN,
        }
    }
}
