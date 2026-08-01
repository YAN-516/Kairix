use alloc::boxed::Box;
use alloc::ffi::CString;
use alloc::string::{String, ToString};
use alloc::sync::{Arc, Weak};
use alloc::{format, vec, vec::Vec};
use core::cell::RefMut;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use bitflags::*;
use lazy_static::*;
use log::{debug, error, info, warn};
use spin::{Mutex, MutexGuard, rwlock::RwLock};

use virtio_drivers::device::blk::VirtIOBlk;
use virtio_drivers::transport::mmio::{MmioTransport, VirtIOHeader};
use virtio_drivers::transport::{DeviceType, Transport};

use lwext4_rust::bindings::{O_APPEND, O_RDONLY, O_RDWR, O_TRUNC, O_WRONLY, SEEK_SET};
use lwext4_rust::{InodeTypes, Lwext4File};

// use crate::config::PAGE_SIZE;
use crate::drivers::block::BLOCK_DEVICE;
use crate::error::{SysError, SysResult, SyscallResult};
use crate::mm::{UserBuffer, frame_alloc};
use crate::sync::SleepLock;
use crate::timer::realtime_timespec;
use polyhal::common::FrameTracker;
use polyhal::consts::PAGE_SIZE;

use crate::fs::vfs::{
    Dentry, FileInner, OpenFlags,
    dcache::GLOBAL_DCACHE,
    file::{FS_IOC_GETFLAGS, FS_IOC_SETFLAGS, File, ioctl_get_fs_flags, ioctl_set_fs_flags},
    inode::{Inode, InodeMode, PageCacheInvalidationGuard},
    kstat::Kstat,
    path::{resolve_path, split_parent_and_name},
};

use crate::fs::lwext4::{
    Lwext4MountGate, Lwext4Op,
    dentry::Ext4Dentry,
    disk::Disk,
    ext4::file::ExtFS,
    inode::{Ext4Inode, fill_ext4_kstat},
    lwext4_mount_gate_for_path, with_lwext4_mount_lock_op, with_lwext4_mount_read_lock_op,
};

use crate::fs::get_filesystem;
use crate::fs::page::pagecache::{PAGE_CACHE, Page};

const EXT4_SEQUENTIAL_READAHEAD_PAGES: usize = 8;
/// mmap faults do not pass through the buffered read readahead state machine.
/// Fill one 64 KiB VirtIO bounce-buffer window on a cache miss so sequential
/// compiler/library mappings do not degenerate into one request per 4 KiB page.
const EXT4_MMAP_READAHEAD_PAGES: usize = 16;
const EXT4_STRIDED_READAHEAD_PAGES: usize = 4;
const EXT4_MAX_READAHEAD_STRIDE: usize = 8;
const EXT4_READAHEAD_MIN_STREAK: usize = 2;
const EXT4_HOT_PAGE_CACHE_PAGES: usize = 8;
const EXT4_WRITEBACK_BATCH_PAGES: usize = 8;

static EXT4_FLUSH_ACTIVE: AtomicBool = AtomicBool::new(false);
static EXT4_FLUSH_PID: AtomicUsize = AtomicUsize::new(0);
static EXT4_FLUSH_INODE: AtomicUsize = AtomicUsize::new(0);
static EXT4_FLUSH_PHASE: AtomicUsize = AtomicUsize::new(0);
static EXT4_FLUSH_DIRTY_PAGES: AtomicUsize = AtomicUsize::new(0);
static EXT4_FLUSH_PAGES_DONE: AtomicUsize = AtomicUsize::new(0);
static EXT4_FLUSH_CURRENT_PAGE: AtomicUsize = AtomicUsize::new(usize::MAX);
static EXT4_FLUSH_CURRENT_PPN: AtomicUsize = AtomicUsize::new(usize::MAX);
static EXT4_FLUSH_PAGE_PHASE: AtomicUsize = AtomicUsize::new(0);
static EXT4_FLUSH_FILE_SIZE: AtomicUsize = AtomicUsize::new(0);
static EXT4_WRITEBACK_COALESCED_BATCHES: AtomicUsize = AtomicUsize::new(0);
static EXT4_WRITEBACK_COALESCED_PAGES: AtomicUsize = AtomicUsize::new(0);
static EXT4_WRITE_GENERATION_RETRY_LOGS: AtomicUsize = AtomicUsize::new(0);
static EXT4_CACHE_GENERATION_RETRY_LOGS: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Copy)]
/// Lock-free diagnostic snapshot of the active ext4 page-cache flush.
pub struct Ext4FlushStats {
    /// Whether a task is currently executing this flush operation.
    pub active: bool,
    /// 0=idle, 1=file lock, 2=initial truncate, 3=page writes,
    /// 4=final truncate, 5=cache flush, 6=complete.
    pub phase: usize,
    /// Process performing the flush, or zero outside task context.
    pub pid: usize,
    /// Page-cache inode identifier being flushed.
    pub inode: usize,
    /// Number of dirty pages selected when the flush began.
    pub dirty_pages: usize,
    /// Number of selected pages written so far.
    pub pages_done: usize,
    /// Page index currently being processed, when applicable.
    pub current_page: Option<usize>,
    /// Physical page currently supplied to lwext4 writeback.
    pub current_ppn: Option<usize>,
    /// Per-page progress while `phase == 3`: 0=inactive, 1=waiting for the
    /// page write lock, 2=page locked, 3=seeking, 4=seek complete,
    /// 5=writing through lwext4, 6=write complete, 7=page complete.
    pub page_phase: usize,
    /// Latest logical file size observed by the flush.
    pub file_size: usize,
    /// Cumulative adjacent-page batches submitted through one `ext4_fwrite`.
    pub coalesced_batches: usize,
    /// Cumulative pages submitted through coalesced writeback batches.
    pub coalesced_pages: usize,
}

/// Return the current ext4 flush progress without acquiring filesystem locks.
pub fn ext4_flush_stats() -> Ext4FlushStats {
    let current_page = EXT4_FLUSH_CURRENT_PAGE.load(Ordering::Acquire);
    let current_ppn = EXT4_FLUSH_CURRENT_PPN.load(Ordering::Acquire);
    Ext4FlushStats {
        active: EXT4_FLUSH_ACTIVE.load(Ordering::Acquire),
        phase: EXT4_FLUSH_PHASE.load(Ordering::Acquire),
        pid: EXT4_FLUSH_PID.load(Ordering::Acquire),
        inode: EXT4_FLUSH_INODE.load(Ordering::Acquire),
        dirty_pages: EXT4_FLUSH_DIRTY_PAGES.load(Ordering::Acquire),
        pages_done: EXT4_FLUSH_PAGES_DONE.load(Ordering::Acquire),
        current_page: (current_page != usize::MAX).then_some(current_page),
        current_ppn: (current_ppn != usize::MAX).then_some(current_ppn),
        page_phase: EXT4_FLUSH_PAGE_PHASE.load(Ordering::Acquire),
        file_size: EXT4_FLUSH_FILE_SIZE.load(Ordering::Acquire),
        coalesced_batches: EXT4_WRITEBACK_COALESCED_BATCHES.load(Ordering::Relaxed),
        coalesced_pages: EXT4_WRITEBACK_COALESCED_PAGES.load(Ordering::Relaxed),
    }
}

struct Ext4FlushProgress;

impl Ext4FlushProgress {
    fn begin(pid: usize, inode: usize, dirty_pages: usize, file_size: usize) -> Self {
        EXT4_FLUSH_PID.store(pid, Ordering::Release);
        EXT4_FLUSH_INODE.store(inode, Ordering::Release);
        EXT4_FLUSH_DIRTY_PAGES.store(dirty_pages, Ordering::Release);
        EXT4_FLUSH_PAGES_DONE.store(0, Ordering::Release);
        EXT4_FLUSH_CURRENT_PAGE.store(usize::MAX, Ordering::Release);
        EXT4_FLUSH_CURRENT_PPN.store(usize::MAX, Ordering::Release);
        EXT4_FLUSH_PAGE_PHASE.store(0, Ordering::Release);
        EXT4_FLUSH_FILE_SIZE.store(file_size, Ordering::Release);
        EXT4_FLUSH_PHASE.store(1, Ordering::Release);
        EXT4_FLUSH_ACTIVE.store(true, Ordering::Release);
        Self
    }
}

impl Drop for Ext4FlushProgress {
    fn drop(&mut self) {
        EXT4_FLUSH_CURRENT_PPN.store(usize::MAX, Ordering::Release);
        EXT4_FLUSH_PHASE.store(6, Ordering::Release);
        EXT4_FLUSH_ACTIVE.store(false, Ordering::Release);
    }
}

/// Keep ext4 writeback from cleaning a page before the cached writer has
/// published the corresponding inode size and timestamps.
struct Ext4PageCacheWriteGuard {
    inode: Arc<dyn Inode>,
    owner_task: Option<Arc<crate::task::TaskControlBlock>>,
}

impl Ext4PageCacheWriteGuard {
    fn new(inode: Arc<dyn Inode>) -> Self {
        let owner_task = crate::task::current_task();
        if let Some(task) = owner_task.as_ref() {
            task.enter_kernel_critical_section();
        }
        inode.begin_page_cache_write();
        Self { inode, owner_task }
    }
}

impl Drop for Ext4PageCacheWriteGuard {
    fn drop(&mut self) {
        self.inode.end_page_cache_write();
        if let Some(task) = self.owner_task.take() {
            task.leave_kernel_critical_section();
        }
    }
}

/// Exclude cached writers across the complete dirty-page snapshot/write/clean
/// transaction. The inode state is shared by every open file description.
struct Ext4PageCacheWritebackGuard {
    inode: Arc<dyn Inode>,
    owner_task: Option<Arc<crate::task::TaskControlBlock>>,
}

impl Ext4PageCacheWritebackGuard {
    fn new(inode: Arc<dyn Inode>) -> Self {
        let owner_task = crate::task::current_task();
        if let Some(task) = owner_task.as_ref() {
            task.enter_kernel_critical_section();
        }
        inode.begin_page_cache_writeback();
        Self { inode, owner_task }
    }
}

impl Drop for Ext4PageCacheWritebackGuard {
    fn drop(&mut self) {
        self.inode.end_page_cache_writeback();
        if let Some(task) = self.owner_task.take() {
            task.leave_kernel_critical_section();
        }
    }
}

struct ReadAheadState {
    last_page: Option<usize>,
    last_delta: isize,
    delta_streak: usize,
}

struct HotPageEntry {
    page_id: usize,
    generation: usize,
    page: Arc<RwLock<Page>>,
}

impl ReadAheadState {
    const fn new() -> Self {
        Self {
            last_page: None,
            last_delta: 0,
            delta_streak: 0,
        }
    }
}

///the Ext4File
pub struct Ext4File {
    readable: bool,
    writable: bool,
    append: bool,
    inode: Arc<dyn Inode>,
    inner: Mutex<FileInner>,
    ///
    pub ext4file: SleepLock<Lwext4File>,
    /// Serialize a cache miss through disk read and page publication.  The
    /// lwext4 handle is also serialized, but releasing that lock before the
    /// newly loaded page is published leaves a window where every sibling
    /// fault can repeat the same disk read.
    cache_load: SleepLock<()>,
    mount_gate: Arc<Lwext4MountGate>,
    direct_dirty: AtomicBool,
    readahead: Mutex<ReadAheadState>,
    hot_pages: Mutex<Vec<HotPageEntry>>,
}

impl Ext4File {
    fn discard_closed_writeback_before_truncate(inode: &Arc<dyn Inode>) {
        let Some(cache_inode_id) = inode.cache_inode_id() else {
            return;
        };
        let (discarded, kept) = crate::fs::writeback::discard_closed_inode(cache_inode_id);
        debug!(
            "[EXT4_TRUNCATE_INVALIDATE] inode={} discarded_closed={} kept_live={}",
            cache_inode_id, discarded, kept
        );
    }

    #[track_caller]
    fn with_ext4file_op<R>(&self, operation: Lwext4Op, f: impl FnOnce(&mut Lwext4File) -> R) -> R {
        with_lwext4_mount_lock_op(&self.mount_gate, operation, || {
            let mut ext4file = self.ext4file.lock();
            f(&mut ext4file)
        })
    }

    /// Construct an Ext4File from a Dentry
    pub fn new(
        readable: bool,
        writable: bool,
        dentry: Arc<dyn Dentry>,
        types: InodeTypes,
        flags: OpenFlags,
    ) -> SysResult<Self> {
        let path = dentry.path();
        let mount_gate = lwext4_mount_gate_for_path(&path).ok_or(SysError::EIO)?;
        let inode = dentry.get_inode().ok_or(SysError::EIO)?;
        let mut effective_type = types;
        if effective_type == InodeTypes::EXT4_DE_UNKNOWN {
            if let Ok(c_probe) = CString::new(path.clone()) {
                if crate::fs::lwext4::ext4::dir::ExtDir::open(&c_probe).is_ok() {
                    effective_type = InodeTypes::EXT4_DE_DIR;
                } else {
                    effective_type = InodeTypes::EXT4_DE_REG_FILE;
                }
            }
        }

        let mut file = Lwext4File::new(path.as_str(), effective_type.clone());
        if effective_type != InodeTypes::EXT4_DE_DIR {
            let mut open_flags = if writable { O_RDWR } else { O_RDONLY };
            if flags.contains(OpenFlags::O_TRUNC) {
                open_flags |= O_TRUNC;
            }
            if flags.contains(OpenFlags::O_APPEND) {
                open_flags |= O_APPEND;
            }
            let truncating = flags.contains(OpenFlags::O_TRUNC);
            // Serialize the complete O_TRUNC transaction with cached writers
            // and writeback.  Generation checks keep readers from publishing
            // stale loads, but they cannot stop an already active old
            // writeback from running its final truncate after the new inode
            // state has been published.
            let _truncate_guard =
                truncating.then(|| Ext4PageCacheWritebackGuard::new(inode.clone()));
            if truncating {
                Self::discard_closed_writeback_before_truncate(&inode);
            }
            let mut invalidation = None;
            let open = || {
                if truncating {
                    invalidation = Some(PageCacheInvalidationGuard::new(inode.as_ref()));
                }
                let result = file.file_open(path.as_str(), open_flags);
                if truncating && result.is_ok() {
                    // O_TRUNC defines a new zero-length file. Do not trust a
                    // stale descriptor size from the C layer to republish the
                    // pre-truncate length into the shared VFS inode.
                    inode.set_size(0);
                }
                result
            };
            let open_result = if truncating {
                with_lwext4_mount_lock_op(&mount_gate, Lwext4Op::Truncate, open)
            } else {
                with_lwext4_mount_read_lock_op(&mount_gate, Lwext4Op::OpenClose, open)
            };
            if truncating && open_result.is_ok() {
                if let Some(cache_inode_id) = inode.cache_inode_id() {
                    PAGE_CACHE.remove_inode_pages(cache_inode_id);
                }
                inode.clear_punched_holes();
                invalidation
                    .take()
                    .expect("O_TRUNC invalidation guard missing")
                    .commit();
            } else if let Some(invalidation) = invalidation.take() {
                invalidation.abort();
            }
            if open_result.is_err() {
                with_lwext4_mount_read_lock_op(&mount_gate, Lwext4Op::OpenClose, || {
                    let _ = file.file_close();
                });
                return Err(SysError::ENOENT);
            }
            // A shared VFS inode may be ahead of lwext4's on-disk descriptor
            // while dirty page-cache data is waiting for writeback. Reopening
            // that inode must not publish the stale disk length and make tail
            // pages appear beyond EOF. O_TRUNC is the exception: it is an
            // explicit size replacement and must invalidate all cached data.
            let real_size = file.file_desc.fsize as usize;
            if !truncating && inode.page_cache_generation() == 0 {
                // Once a destructive truncate has advanced the inode
                // generation, the shared VFS size is authoritative. A stale
                // lwext4 descriptor must not expand it on reopen.
                inode.extend_size(real_size);
            }
        } else {
            info!("Opening a directory: {}, skipping ext4_fopen", path);
        }
        Ok(Self {
            readable,
            writable,
            append: flags.contains(OpenFlags::O_APPEND),
            inode,
            inner: Mutex::new(FileInner {
                offset: 0,
                dentry,
                flags,
            }),
            // lwext4's C locks may cooperatively yield while this handle is
            // held.  A spin mutex here can therefore occupy every CPU with
            // interrupts masked while the owner continuation is runnable but
            // unable to resume.  A blocking lock preserves the continuation
            // and lets the owner make progress.
            ext4file: SleepLock::new_fair(file),
            cache_load: SleepLock::new_fair(()),
            mount_gate,
            direct_dirty: AtomicBool::new(false),
            readahead: Mutex::new(ReadAheadState::new()),
            hot_pages: Mutex::new(Vec::new()),
        })
    }

    // /// Read all data
    // pub fn read_all(&self) -> Vec<u8> {
    //     let mut inner = self.inner.lock();
    //     let mut buffer = [0u8; 512];
    //     let mut v: Vec<u8> = Vec::new();
    //     loop {
    //         let current_offset = inner.offset;
    //         self
    //             .ext4file
    //             .lock()
    //             .file_seek(current_offset as i64, SEEK_SET)
    //             .expect("seek failed");
    //         let len = self.ext4file.lock().file_read(&mut buffer).unwrap();
    //         if len == 0 {
    //             break;
    //         }
    //         inner.offset += len;
    //         v.extend_from_slice(&buffer[..len]);
    //     }
    //     v
    // }

    #[allow(unused)]
    /// Truncate the inode to the given size
    pub fn ext4_truncate(&self, size: u64) -> SysResult<usize> {
        info!("truncate file to size={}", size);
        let old_size = self
            .inner
            .lock()
            .dentry
            .get_inode()
            .map(|inode| inode.get_size())
            .unwrap_or(0);
        let new_size = size as usize;
        self.flush_dirty_pages(None);
        let _truncate_guard = Ext4PageCacheWritebackGuard::new(self.inode.clone());
        self.clear_hot_pages();
        let destructive = new_size < old_size;
        if destructive {
            Self::discard_closed_writeback_before_truncate(&self.inode);
        }
        let mut invalidation = None;
        let res = self.with_ext4file_op(Lwext4Op::Truncate, |ext4file| {
            if destructive {
                invalidation = Some(PageCacheInvalidationGuard::new(self.inode.as_ref()));
            }
            ext4file.file_truncate(size)
        });
        if let Err(err) = res {
            if let Some(invalidation) = invalidation.take() {
                invalidation.abort();
            }
            return Err(crate::fs::lwext4::lwext4_err_to_sys(err));
        }
        if destructive {
            let trim_result = trim_cached_pages_after_size(
                self.inode
                    .cache_inode_id()
                    .unwrap_or_else(|| self.inode.get_ino()),
                new_size,
            );
            self.inode.truncate_punched_holes(new_size);
            self.inode.set_size(new_size);
            invalidation
                .take()
                .expect("truncate invalidation guard missing")
                .commit();
            trim_result?;
        } else {
            self.inode.set_size(new_size);
        }
        Ok(0)
    }

    /// Load a consecutive page run with one mount-gate acquisition and one
    /// file-position transaction. Frames and the bounce buffer are allocated
    /// before entering lwext4, and PAGE_CACHE is not held across disk I/O.
    fn load_page_range_from_disk(
        &self,
        inode_id: usize,
        start_page: usize,
        page_count: usize,
        file_size: usize,
    ) -> SysResult<Vec<(usize, Arc<RwLock<Page>>)>> {
        if page_count == 0 || file_size == 0 {
            return Ok(Vec::new());
        }
        let max_page = file_size.div_ceil(PAGE_SIZE);
        if start_page >= max_page {
            return Ok(Vec::new());
        }
        let end_page = start_page.saturating_add(page_count).min(max_page);
        let actual_pages = end_page - start_page;
        let start_offset = start_page
            .checked_mul(PAGE_SIZE)
            .ok_or(SysError::EOVERFLOW)?;
        let read_len = file_size
            .saturating_sub(start_offset)
            .min(actual_pages.saturating_mul(PAGE_SIZE));

        let mut frames = Vec::with_capacity(actual_pages);
        for page_id in start_page..end_page {
            frames.push((page_id, Arc::new(frame_alloc().ok_or(SysError::ENOMEM)?)));
        }
        let mut data = vec![0u8; read_len];
        let mut read_once = || {
            self.with_ext4file_op(Lwext4Op::Read, |ext4file| {
                ext4file
                    .file_seek(start_offset as i64, SEEK_SET)
                    .map_err(crate::fs::lwext4::lwext4_err_to_sys)?;
                let mut done = 0usize;
                while done < data.len() {
                    let n = ext4file
                        .file_read(&mut data[done..])
                        .map_err(crate::fs::lwext4::lwext4_err_to_sys)?;
                    if n == 0 {
                        break;
                    }
                    done += n;
                }
                let disk_size = if done < data.len() {
                    Some(ExtFS::file_stat(&mut ext4file.file_desc)?.size as usize)
                } else {
                    None
                };
                Ok((done, disk_size))
            })
        };
        let (mut bytes_read, mut disk_size) = read_once()?;
        let mut flushed_files = 0usize;
        if bytes_read < read_len && disk_size.expect("short read missing disk size") < file_size {
            flushed_files = crate::fs::writeback::flush_inode_now(inode_id);
            if flushed_files > 0 {
                (bytes_read, disk_size) = read_once()?;
            }
        }
        if bytes_read < read_len {
            let disk_size = disk_size.expect("short read missing disk size");
            let disk_backed_len = disk_size.saturating_sub(start_offset).min(read_len);
            if bytes_read < disk_backed_len {
                warn!(
                    "[EXT4_CACHE_SHORT_READ] inode={} offset={} requested={} actual={} vfs_size={} disk_size={} flushed_files={}",
                    inode_id,
                    start_offset,
                    read_len,
                    bytes_read,
                    file_size,
                    disk_size,
                    flushed_files
                );
                return Err(SysError::EIO);
            }
            debug!(
                "[EXT4_CACHE_ZERO_FILL] inode={} offset={} requested={} disk_bytes={} zero_bytes={} vfs_size={} disk_size={} flushed_files={}",
                inode_id,
                start_offset,
                read_len,
                bytes_read,
                read_len - bytes_read,
                file_size,
                disk_size,
                flushed_files
            );
        }

        let mut pages = Vec::with_capacity(actual_pages);
        for (page_id, frame) in frames {
            let bytes = frame.ppn.get_bytes_array();
            bytes.fill(0);
            let source_offset = (page_id - start_page) * PAGE_SIZE;
            if source_offset < bytes_read {
                let copy_len = (bytes_read - source_offset).min(PAGE_SIZE);
                bytes[..copy_len].copy_from_slice(&data[source_offset..source_offset + copy_len]);
            }
            pages.push((page_id, Arc::new(RwLock::new(Page::new(frame)))));
        }
        Ok(pages)
    }

    /// Load one demand page directly into its frame. The batched bounce-buffer
    /// path is reserved for readahead so an isolated cache miss does not pay an
    /// extra allocation and copy.
    fn load_page_from_disk(
        &self,
        inode_id: usize,
        page_id: usize,
        file_size: usize,
    ) -> SysResult<Arc<RwLock<Page>>> {
        let frame = Arc::new(frame_alloc().ok_or(SysError::ENOMEM)?);
        let page_start_offset = page_id.checked_mul(PAGE_SIZE).ok_or(SysError::EOVERFLOW)?;
        let bytes = frame.ppn.get_bytes_array();
        bytes.fill(0);
        if page_start_offset < file_size {
            let valid_len = (file_size - page_start_offset).min(PAGE_SIZE);
            let mut read_once = || {
                self.with_ext4file_op(Lwext4Op::Read, |ext4file| {
                    ext4file
                        .file_seek(page_start_offset as i64, SEEK_SET)
                        .map_err(crate::fs::lwext4::lwext4_err_to_sys)?;
                    let mut done = 0usize;
                    while done < valid_len {
                        let n = ext4file
                            .file_read(&mut bytes[done..valid_len])
                            .map_err(crate::fs::lwext4::lwext4_err_to_sys)?;
                        if n == 0 {
                            break;
                        }
                        done += n;
                    }
                    let disk_size = if done < valid_len {
                        Some(ExtFS::file_stat(&mut ext4file.file_desc)?.size as usize)
                    } else {
                        None
                    };
                    Ok((done, disk_size))
                })
            };
            let (mut bytes_read, mut disk_size) = read_once()?;
            let mut flushed_files = 0usize;
            if bytes_read < valid_len
                && disk_size.expect("short read missing disk size") < file_size
            {
                flushed_files = crate::fs::writeback::flush_inode_now(inode_id);
                if flushed_files > 0 {
                    (bytes_read, disk_size) = read_once()?;
                }
            }
            if bytes_read < valid_len {
                let disk_size = disk_size.expect("short read missing disk size");
                let disk_backed_len = disk_size.saturating_sub(page_start_offset).min(valid_len);
                if bytes_read < disk_backed_len {
                    warn!(
                        "[EXT4_CACHE_SHORT_READ] inode={} offset={} requested={} actual={} vfs_size={} disk_size={} flushed_files={}",
                        inode_id,
                        page_start_offset,
                        valid_len,
                        bytes_read,
                        file_size,
                        disk_size,
                        flushed_files
                    );
                    return Err(SysError::EIO);
                }
                debug!(
                    "[EXT4_CACHE_ZERO_FILL] inode={} offset={} requested={} disk_bytes={} zero_bytes={} vfs_size={} disk_size={} flushed_files={}",
                    inode_id,
                    page_start_offset,
                    valid_len,
                    bytes_read,
                    valid_len - bytes_read,
                    file_size,
                    disk_size,
                    flushed_files
                );
            }
        }
        Ok(Arc::new(RwLock::new(Page::new(frame))))
    }

    /// Merge pages loaded without PAGE_CACHE into the cache. A concurrent
    /// loader wins on duplicate insertion; its page is returned instead.
    fn insert_loaded_pages(
        &self,
        ino: usize,
        loaded: Vec<(usize, Arc<RwLock<Page>>)>,
        requested_page: Option<usize>,
        load_generation: usize,
    ) -> (Option<Arc<RwLock<Page>>>, bool, bool) {
        let mut hot_pages = Vec::with_capacity(loaded.len());
        let mut inserted_pages = Vec::with_capacity(loaded.len());
        let mut requested = None;
        let mut under_pressure = false;
        for (page_id, new_page) in loaded {
            if self.inode.page_cache_generation() != load_generation {
                for (inserted_page_id, inserted_page) in inserted_pages {
                    PAGE_CACHE.remove_page_if_same(ino, inserted_page_id, &inserted_page);
                }
                return (None, under_pressure, false);
            }
            let (page, pressured, inserted) =
                PAGE_CACHE.insert_page_if_absent(ino, page_id, new_page);
            under_pressure |= pressured;
            if inserted {
                inserted_pages.push((page_id, page.clone()));
            }
            if requested_page == Some(page_id) {
                requested = Some(page.clone());
            }
            hot_pages.push((page_id, page));
        }
        if self.inode.page_cache_generation() != load_generation {
            for (page_id, page) in inserted_pages {
                PAGE_CACHE.remove_page_if_same(ino, page_id, &page);
            }
            return (None, under_pressure, false);
        }
        for (page_id, page) in hot_pages {
            self.remember_hot_page_for_generation(page_id, page, load_generation);
        }
        if under_pressure {
            crate::fs::writeback::request_writeback();
        }
        (requested, under_pressure, true)
    }

    fn get_hot_page_for_generation(
        &self,
        page_id: usize,
        generation: usize,
    ) -> Option<Arc<RwLock<Page>>> {
        if generation & 1 != 0 || self.inode.page_cache_generation() != generation {
            return None;
        }
        let mut hot_pages = self.hot_pages.lock();
        let pos = hot_pages
            .iter()
            .position(|entry| entry.page_id == page_id)?;
        let entry = hot_pages.remove(pos);
        if entry.generation != generation {
            return None;
        }
        if self.inode.page_cache_generation() != generation {
            return None;
        }
        let page = entry.page.clone();
        hot_pages.push(entry);
        Some(page)
    }

    fn remember_hot_page_for_generation(
        &self,
        page_id: usize,
        page: Arc<RwLock<Page>>,
        generation: usize,
    ) {
        if generation & 1 != 0 || self.inode.page_cache_generation() != generation {
            return;
        }
        let mut hot_pages = self.hot_pages.lock();
        if self.inode.page_cache_generation() != generation {
            return;
        }
        if let Some(pos) = hot_pages.iter().position(|entry| entry.page_id == page_id) {
            hot_pages.remove(pos);
        } else if hot_pages.len() >= EXT4_HOT_PAGE_CACHE_PAGES {
            hot_pages.remove(0);
        }
        hot_pages.push(HotPageEntry {
            page_id,
            generation,
            page,
        });
    }

    fn get_cached_page_for_generation(
        &self,
        ino: usize,
        page_id: usize,
        generation: usize,
    ) -> Option<Arc<RwLock<Page>>> {
        if let Some(page) = self.get_hot_page_for_generation(page_id, generation) {
            return Some(page);
        }
        let page = PAGE_CACHE.get_page_touch(ino, page_id)?;
        if self.inode.page_cache_generation() != generation {
            return None;
        }
        self.remember_hot_page_for_generation(page_id, page.clone(), generation);
        if self.inode.page_cache_generation() != generation {
            return None;
        }
        Some(page)
    }

    fn note_cache_generation_retry(
        &self,
        stage: &str,
        ino: usize,
        page_id: usize,
        load_generation: usize,
    ) {
        let retry = EXT4_CACHE_GENERATION_RETRY_LOGS.fetch_add(1, Ordering::Relaxed);
        if retry < 16 || retry % 512 == 0 {
            warn!(
                "[EXT4_CACHE_GENERATION_RETRY] stage={} inode={} page={} load_generation={} current_generation={}",
                stage,
                ino,
                page_id,
                load_generation,
                self.inode.page_cache_generation(),
            );
        }
    }

    fn clear_hot_pages(&self) {
        self.hot_pages.lock().clear();
    }

    /// 获取指定的缓存页，如果 Miss 则自动从磁盘加载并放入缓存
    fn get_or_load_cache_page(
        &self,
        ino: usize,
        page_id: usize,
        old_size: usize,
    ) -> SysResult<(Arc<RwLock<Page>>, bool)> {
        self.get_or_load_cache_page_window(ino, page_id, old_size, 1)
    }

    /// Resolve one demanded cache page and optionally load a consecutive
    /// forward window in the same ext4 transaction. The cache-load lock keeps
    /// the miss/recheck/publish sequence coherent with truncate and competing
    /// faults; callers requesting a one-page window retain the direct-to-frame
    /// demand-read path.
    fn get_or_load_cache_page_window(
        &self,
        ino: usize,
        page_id: usize,
        old_size: usize,
        max_window_pages: usize,
    ) -> SysResult<(Arc<RwLock<Page>>, bool)> {
        let initial_generation = self.inode.page_cache_generation();
        if initial_generation & 1 == 0 {
            if let Some(page) =
                self.get_cached_page_for_generation(ino, page_id, initial_generation)
            {
                return Ok((page, false));
            }
        }

        // Cover the complete miss -> load -> publish interval.  Recheck after
        // acquiring because another fault may have populated this page while
        // the current task slept.  In particular this keeps an ELF fault
        // storm from queueing one long lwext4 read per CPU for the same page.
        let _cache_load = self.cache_load.lock();
        let mut load_size = old_size;
        loop {
            let load_generation = loop {
                let generation = self.inode.page_cache_generation();
                if generation & 1 == 0 {
                    break generation;
                }
                crate::task::suspend_current_and_run_next();
            };
            if let Some(page) = self.get_cached_page_for_generation(ino, page_id, load_generation) {
                return Ok((page, false));
            }

            let max_file_page = load_size.div_ceil(PAGE_SIZE);
            let mut window_pages = 1usize;
            while window_pages < max_window_pages {
                let Some(candidate) = page_id.checked_add(window_pages) else {
                    break;
                };
                if candidate >= max_file_page || self.page_cached(ino, candidate) {
                    break;
                }
                window_pages += 1;
            }

            let loaded = match if window_pages == 1 {
                self.load_page_from_disk(ino, page_id, load_size)
                    .map(|page| vec![(page_id, page)])
            } else {
                self.load_page_range_from_disk(ino, page_id, window_pages, load_size)
            } {
                Ok(pages) => pages,
                Err(error) => {
                    if self.inode.page_cache_generation() != load_generation {
                        self.note_cache_generation_retry(
                            "demand_read_error",
                            ino,
                            page_id,
                            load_generation,
                        );
                        load_size = self.inode.get_size();
                        continue;
                    }
                    // A writer can publish the cache page after our initial miss
                    // but before the disk read notices the stale inode length.
                    // Prefer that authoritative shared page over surfacing EIO.
                    if let Some(page) =
                        self.get_cached_page_for_generation(ino, page_id, load_generation)
                    {
                        return Ok((page, false));
                    }
                    return Err(error);
                }
            };
            let (page, under_pressure, stable) =
                self.insert_loaded_pages(ino, loaded, Some(page_id), load_generation);
            if stable {
                return page.map(|page| (page, under_pressure)).ok_or(SysError::EIO);
            }
            self.note_cache_generation_retry("demand_publish", ino, page_id, load_generation);
            load_size = self.inode.get_size();
        }
    }

    fn get_or_alloc_overwrite_page(
        &self,
        ino: usize,
        page_id: usize,
    ) -> SysResult<(Arc<RwLock<Page>>, bool)> {
        let initial_generation = self.inode.page_cache_generation();
        if initial_generation & 1 == 0 {
            if let Some(page) =
                self.get_cached_page_for_generation(ino, page_id, initial_generation)
            {
                return Ok((page, false));
            }
        }
        let _cache_load = self.cache_load.lock();
        loop {
            let generation = loop {
                let generation = self.inode.page_cache_generation();
                if generation & 1 == 0 {
                    break generation;
                }
                crate::task::suspend_current_and_run_next();
            };
            if let Some(page) = self.get_cached_page_for_generation(ino, page_id, generation) {
                return Ok((page, false));
            }

            let new_frame = Arc::new(frame_alloc().ok_or(SysError::ENOMEM)?);
            let new_page = Arc::new(RwLock::new(Page::new(new_frame)));
            if self.inode.page_cache_generation() != generation {
                self.note_cache_generation_retry("overwrite_allocate", ino, page_id, generation);
                continue;
            }
            let (page, under_pressure, inserted) =
                PAGE_CACHE.insert_page_if_absent(ino, page_id, new_page);
            if self.inode.page_cache_generation() != generation {
                if inserted {
                    PAGE_CACHE.remove_page_if_same(ino, page_id, &page);
                }
                self.note_cache_generation_retry("overwrite_publish", ino, page_id, generation);
                continue;
            }
            self.remember_hot_page_for_generation(page_id, page.clone(), generation);
            if under_pressure {
                crate::fs::writeback::request_writeback();
            }
            return Ok((page, under_pressure));
        }
    }

    fn prefetch_page_range(
        &self,
        ino: usize,
        start_page: usize,
        file_size: usize,
        page_count: usize,
        reverse: bool,
    ) -> bool {
        if file_size == 0 || page_count == 0 {
            return false;
        }
        let max_page = file_size.div_ceil(PAGE_SIZE);
        let high_page = start_page.min(max_page.saturating_sub(1));
        let (first_page, actual_pages) = if reverse {
            let first_page = high_page.saturating_sub(page_count.saturating_sub(1));
            (first_page, high_page - first_page + 1)
        } else {
            let end_page = start_page.saturating_add(page_count).min(max_page);
            (start_page, end_page.saturating_sub(start_page))
        };
        let load_generation = self.inode.page_cache_generation();
        if load_generation & 1 != 0 {
            return false;
        }
        let loaded = match self.load_page_range_from_disk(ino, first_page, actual_pages, file_size)
        {
            Ok(loaded) => loaded,
            Err(_) => return false,
        };
        let (_, under_pressure, stable) =
            self.insert_loaded_pages(ino, loaded, None, load_generation);
        if !stable {
            self.note_cache_generation_retry("readahead_publish", ino, first_page, load_generation);
        }
        under_pressure
    }

    fn page_cached(&self, ino: usize, page_id: usize) -> bool {
        PAGE_CACHE.get_page(ino, page_id).is_some()
    }

    fn prefetch_strided_pages(
        &self,
        ino: usize,
        start_page: usize,
        file_size: usize,
        page_count: usize,
        stride: isize,
    ) -> bool {
        if file_size == 0 || page_count == 0 || stride == 0 {
            return false;
        }
        let max_page = (file_size + PAGE_SIZE - 1) / PAGE_SIZE;
        let mut page_id = start_page as isize;
        let mut under_pressure = false;
        for _ in 0..page_count {
            if page_id < 0 || page_id as usize >= max_page {
                break;
            }
            if let Ok((_, pressure)) = self.get_or_load_cache_page(ino, page_id as usize, file_size)
            {
                under_pressure |= pressure;
            } else {
                break;
            }
            page_id = page_id.saturating_add(stride);
        }
        under_pressure
    }

    fn maybe_readahead_after_page(&self, ino: usize, page_id: usize, file_size: usize) -> bool {
        let delta = {
            let mut state = self.readahead.lock();
            let delta = match state.last_page {
                Some(last_page) if last_page == page_id => return false,
                Some(last_page) => page_id as isize - last_page as isize,
                None => {
                    state.last_page = Some(page_id);
                    return false;
                }
            };
            state.last_page = Some(page_id);
            if delta == state.last_delta {
                state.delta_streak = state.delta_streak.saturating_add(1);
            } else {
                state.last_delta = delta;
                state.delta_streak = 1;
            }
            if state.delta_streak < EXT4_READAHEAD_MIN_STREAK {
                return false;
            }
            delta
        };

        let stride = delta.unsigned_abs();
        if stride == 0 || stride > EXT4_MAX_READAHEAD_STRIDE {
            return false;
        }
        if delta == 1 {
            let start_page = page_id.saturating_add(1);
            if self.page_cached(ino, start_page) {
                return false;
            }
            self.prefetch_page_range(
                ino,
                start_page,
                file_size,
                EXT4_SEQUENTIAL_READAHEAD_PAGES,
                false,
            )
        } else if delta == -1 {
            let Some(start_page) = page_id.checked_sub(1) else {
                return false;
            };
            if self.page_cached(ino, start_page) {
                return false;
            }
            self.prefetch_page_range(
                ino,
                start_page,
                file_size,
                EXT4_SEQUENTIAL_READAHEAD_PAGES,
                true,
            )
        } else {
            let next_page = page_id as isize + delta;
            if next_page < 0 {
                return false;
            }
            let next_page = next_page as usize;
            if self.page_cached(ino, next_page) {
                return false;
            }
            self.prefetch_strided_pages(
                ino,
                next_page,
                file_size,
                EXT4_STRIDED_READAHEAD_PAGES,
                delta,
            )
        }
    }

    fn read_cached_at(
        &self,
        inode: &Arc<dyn Inode>,
        offset: usize,
        max_len: usize,
        buf: &mut UserBuffer,
    ) -> SysResult<(usize, bool)> {
        if max_len == 0 {
            return Ok((0, false));
        }
        let ino = inode.cache_inode_id().unwrap_or_else(|| inode.get_ino());
        let file_size = inode.get_size();
        let mut current_offset = offset;
        let mut remaining = max_len.min(file_size.saturating_sub(current_offset));
        let mut total_read_size = 0usize;
        let mut should_flush_cache = false;
        if current_offset >= file_size {
            return Ok((0, false));
        }
        for slice in buf.buffers.iter_mut() {
            if remaining == 0 {
                break;
            }
            let mut slice_offset = 0;
            let slice_len = slice.len().min(remaining);
            while slice_offset < slice_len && current_offset < file_size {
                let page_id = current_offset / PAGE_SIZE;
                let page_offset = current_offset % PAGE_SIZE;
                let left_in_page = PAGE_SIZE - page_offset;
                let left_in_slice = slice_len - slice_offset;
                let left_in_file = file_size - current_offset;
                let read_bytes = left_in_page.min(left_in_slice).min(left_in_file);
                if inode.is_punched_hole_page(page_id) {
                    slice[slice_offset..slice_offset + read_bytes].fill(0);
                    current_offset += read_bytes;
                    slice_offset += read_bytes;
                    total_read_size += read_bytes;
                    remaining -= read_bytes;
                    continue;
                }
                let (target_page, under_pressure) =
                    self.get_or_load_cache_page(ino, page_id, file_size)?;
                should_flush_cache |= under_pressure && self.writable();
                {
                    let page_reader = target_page.read();
                    let frame = page_reader.resident_frame().ok_or(SysError::EIO)?;
                    let src_data =
                        &frame.ppn.get_bytes_array()[page_offset..page_offset + read_bytes];
                    slice[slice_offset..slice_offset + read_bytes].copy_from_slice(src_data);

                    current_offset += read_bytes;
                    slice_offset += read_bytes;
                    total_read_size += read_bytes;
                    remaining -= read_bytes;
                }
                should_flush_cache |=
                    self.maybe_readahead_after_page(ino, page_id, file_size) && self.writable();
            }
        }
        Ok((total_read_size, should_flush_cache))
    }

    fn touch_modified_inode(inode: &Arc<dyn Inode>) {
        let (now_sec, now_nsec) = realtime_timespec();
        inode.set_mtime(now_sec, now_nsec);
        inode.set_ctime(now_sec, now_nsec);
    }

    fn finish_cached_write(
        inode: &Arc<dyn Inode>,
        write_generation: usize,
        old_size: usize,
        current_offset: usize,
        total_write_size: usize,
    ) {
        if inode.page_cache_generation() != write_generation {
            return;
        }
        if current_offset > old_size {
            // Another writer may have extended the inode while this operation
            // was waiting for a page. Never replace that larger size.
            inode.extend_size(current_offset);
        }
        if total_write_size > 0 {
            Self::touch_modified_inode(inode);
        }
    }

    fn finish_partial_cached_write_or_error(
        inode: &Arc<dyn Inode>,
        write_generation: usize,
        old_size: usize,
        current_offset: usize,
        total_write_size: usize,
        should_flush_cache: bool,
        error: SysError,
    ) -> SysResult<(usize, bool)> {
        if total_write_size == 0 {
            return Err(error);
        }
        // Linux write semantics report a completed prefix instead of losing
        // that progress to a later-page error.
        Self::finish_cached_write(
            inode,
            write_generation,
            old_size,
            current_offset,
            total_write_size,
        );
        Ok((total_write_size, should_flush_cache))
    }

    fn write_cached_at(
        &self,
        inode: &Arc<dyn Inode>,
        offset: usize,
        old_size: usize,
        buf: &UserBuffer,
    ) -> SysResult<(usize, bool)> {
        'write_retry: loop {
            let write_generation = loop {
                let generation = inode.page_cache_generation();
                if generation & 1 == 0 {
                    break generation;
                }
                crate::task::suspend_current_and_run_next();
            };
            let ino = inode.cache_inode_id().unwrap_or_else(|| inode.get_ino());
            let mut total_write_size = 0usize;
            let mut current_offset = offset;
            let mut should_flush_cache = false;
            // Do not materialize the gap between EOF and a positioned write.
            // Uncached bytes beyond the backing file's current EOF are already
            // returned as zero by load_page_from_disk(), and lwext4 represents the
            // range as a sparse hole when writeback extends the file. Writing zero
            // pages here can race with a concurrent lower-offset pwrite and erase
            // data that writer has already committed to the shared page cache.
            for slice in buf.buffers.iter() {
                let mut slice_offset = 0;
                let slice_len = slice.len();
                while slice_offset < slice_len {
                    let page_id = current_offset / PAGE_SIZE;
                    let page_offset = current_offset % PAGE_SIZE;
                    let write_bytes = (PAGE_SIZE - page_offset).min(slice_len - slice_offset);
                    if inode.page_cache_generation() != write_generation {
                        let retry =
                            EXT4_WRITE_GENERATION_RETRY_LOGS.fetch_add(1, Ordering::Relaxed);
                        if retry < 16 || retry % 512 == 0 {
                            log::error!(
                                "[EXT4_WRITE_RETRY] stage=before_page inode={} offset={:#x} generation={} current_generation={} bytes={}",
                                ino,
                                current_offset,
                                write_generation,
                                inode.page_cache_generation(),
                                write_bytes,
                            );
                        }
                        continue 'write_retry;
                    }
                    let overwrites_whole_page = page_offset == 0 && write_bytes == PAGE_SIZE;
                    let page_was_hole = inode.is_punched_hole_page(page_id);
                    let page_result = if overwrites_whole_page || page_was_hole {
                        self.get_or_alloc_overwrite_page(ino, page_id)
                    } else {
                        self.get_or_load_cache_page(ino, page_id, old_size)
                    };
                    let (target_page, under_pressure) = match page_result {
                        Ok(page) => page,
                        Err(err) => {
                            if err == SysError::EIO {
                                error!(
                                    "[EXT4_WRITEBACK_EIO] stage=cache_page_prepare inode={} page={} offset={} len={} old_size={} whole_page={} punched_hole={} error={:?} ext4_flush={:?} block_io={:?}",
                                    ino,
                                    page_id,
                                    current_offset,
                                    write_bytes,
                                    old_size,
                                    overwrites_whole_page,
                                    page_was_hole,
                                    err,
                                    ext4_flush_stats(),
                                    crate::drivers::block::virtio_blk::virtio_block_io_stats(),
                                );
                            }
                            return Self::finish_partial_cached_write_or_error(
                                inode,
                                write_generation,
                                old_size,
                                current_offset,
                                total_write_size,
                                should_flush_cache,
                                err,
                            );
                        }
                    };
                    should_flush_cache |= under_pressure;
                    let mut page_modified = false;
                    let generation_changed = {
                        let mut page_writer = target_page.write();
                        if inode.page_cache_generation() != write_generation {
                            // O_TRUNC completed while this writer was loading or
                            // allocating the page. Do not report these bytes as
                            // written: restart the whole operation under the new
                            // generation so the caller never observes a false
                            // successful prefix.
                            true
                        } else if page_was_hole && !overwrites_whole_page {
                            match page_writer.ensure_resident() {
                                Ok(frame) => frame.ppn.get_bytes_array().fill(0),
                                Err(err) => {
                                    if err == SysError::EIO {
                                        error!(
                                        "[EXT4_WRITEBACK_EIO] stage=cache_page_resident inode={} page={} offset={} len={} old_size={} error={:?} ext4_flush={:?} block_io={:?}",
                                        ino,
                                        page_id,
                                        current_offset,
                                        write_bytes,
                                        old_size,
                                        err,
                                        ext4_flush_stats(),
                                        crate::drivers::block::virtio_blk::virtio_block_io_stats(),
                                    );
                                    }
                                    return Self::finish_partial_cached_write_or_error(
                                        inode,
                                        write_generation,
                                        old_size,
                                        current_offset,
                                        total_write_size,
                                        should_flush_cache,
                                        err,
                                    );
                                }
                            }
                            let data_to_write = &slice[slice_offset..slice_offset + write_bytes];
                            page_writer.modify_with_generation(
                                page_offset,
                                data_to_write,
                                write_generation,
                            );
                            page_modified = true;
                            false
                        } else {
                            let data_to_write = &slice[slice_offset..slice_offset + write_bytes];
                            page_writer.modify_with_generation(
                                page_offset,
                                data_to_write,
                                write_generation,
                            );
                            page_modified = true;
                            false
                        }
                    };
                    if generation_changed {
                        let retry =
                            EXT4_WRITE_GENERATION_RETRY_LOGS.fetch_add(1, Ordering::Relaxed);
                        if retry < 16 || retry % 512 == 0 {
                            log::error!(
                                "[EXT4_WRITE_RETRY] stage=after_page inode={} page={} generation={} current_generation={}",
                                ino,
                                page_id,
                                write_generation,
                                inode.page_cache_generation(),
                            );
                        }
                        continue 'write_retry;
                    }
                    if page_modified {
                        if inode.page_cache_generation() == write_generation {
                            inode.clear_punched_hole_page(page_id);
                        }
                    } else {
                        PAGE_CACHE.remove_page_if_same(ino, page_id, &target_page);
                    }
                    current_offset += write_bytes;
                    slice_offset += write_bytes;
                    total_write_size += write_bytes;
                }
            }
            Self::finish_cached_write(
                inode,
                write_generation,
                old_size,
                current_offset,
                total_write_size,
            );
            if inode.page_cache_generation() != write_generation {
                let retry = EXT4_WRITE_GENERATION_RETRY_LOGS.fetch_add(1, Ordering::Relaxed);
                if retry < 16 || retry % 512 == 0 {
                    log::error!(
                        "[EXT4_WRITE_RETRY] stage=after_write inode={} generation={} current_generation={} bytes={}",
                        ino,
                        write_generation,
                        inode.page_cache_generation(),
                        total_write_size,
                    );
                }
                continue 'write_retry;
            }
            return Ok((total_write_size, should_flush_cache));
        }
    }

    fn flush_dirty_pages(&self, max_pages: Option<usize>) -> (usize, bool) {
        if !self.writable() {
            return (0, false);
        }
        let direct_dirty = self.direct_dirty.swap(false, Ordering::AcqRel);
        let inode = {
            let inner = self.inner.lock();
            inner.dentry.get_inode().unwrap()
        };
        // A cached writer publishes inode size only after all bytes have been
        // copied. Wait for that publication before taking the size/page
        // snapshot, and prevent a new writer from dirtying the snapshot while
        // pages are being cleaned.
        let _writeback_guard = Ext4PageCacheWritebackGuard::new(inode.clone());
        let inode_id = inode.cache_inode_id().unwrap_or_else(|| inode.get_ino());
        let file_size = inode.get_size();
        let flush_generation = inode.page_cache_generation();

        // Never wait for an individual page while its cache shard is held. A
        // page writer may need cache/reclaim services before it can release
        // its RwLock, which would otherwise invert the two lock levels.
        let cached_page_count = PAGE_CACHE.inode_pages_count(inode_id);
        let mut cached_pages = Vec::with_capacity(cached_page_count);
        let snapshot_truncated = PAGE_CACHE.append_inode_pages(inode_id, &mut cached_pages);
        let limit = max_pages.unwrap_or(usize::MAX);
        let mut dirty_pages = Vec::new();
        let mut has_more = snapshot_truncated;
        for (page_id, page_lock) in cached_pages {
            let dirty = if max_pages.is_some() {
                let Some(page) = page_lock.try_read() else {
                    has_more = true;
                    continue;
                };
                page.dirty
            } else {
                page_lock.read().dirty
            };
            if !dirty {
                continue;
            }
            if dirty_pages.len() >= limit {
                has_more = true;
                break;
            }
            dirty_pages.push((page_id, page_lock));
        }
        // Page-cache shards do not promise inode-page iteration order.  Flush
        // low offsets first so a newly created file advances its physical EOF
        // monotonically; sparse higher pages remain correct because the size
        // preparation below now performs real truncate-growth semantics.
        dirty_pages.sort_unstable_by_key(|(page_id, _)| *page_id);
        let dirty_page_count = dirty_pages.len();
        if dirty_page_count == 0 && !direct_dirty {
            // Clean queued files need neither inode-size preparation nor an
            // lwext4 block-cache flush. If a page was busy or the snapshot was
            // truncated, `has_more` keeps the file queued for a later pass.
            return (0, has_more);
        }
        let pid = crate::task::current_task()
            .and_then(|task| task.process.upgrade())
            .map(|process| process.getpid())
            .unwrap_or(0);
        let _progress = Ext4FlushProgress::begin(pid, inode_id, dirty_page_count, file_size);

        // File-size preparation is a separate short transaction.  In
        // particular, the lwext4 gate is no longer held while we wait for any
        // page lock or while processing the rest of the dirty batch.
        EXT4_FLUSH_PHASE.store(2, Ordering::Release);
        let initial_truncate_ok = self.with_ext4file_op(Lwext4Op::Writeback, |ext4file| {
            if inode.page_cache_generation() != flush_generation || flush_generation & 1 != 0 {
                return true;
            }
            // The descriptor length may be ahead of the on-disk inode while
            // delayed page-cache data is pending.  Always enter lwext4 so it
            // verifies and, when needed, grows the inode itself.
            match ext4file.file_truncate(file_size as u64) {
                Ok(_) => true,
                Err(e) => {
                    error!(
                        "[EXT4_WRITEBACK_EIO] stage=initial_truncate pid={} inode={} file_size={} raw_error={} ext4_flush={:?} block_io={:?}",
                        pid,
                        inode_id,
                        file_size,
                        e,
                        ext4_flush_stats(),
                        crate::drivers::block::virtio_blk::virtio_block_io_stats(),
                    );
                    warn!(
                        "file_truncate before flush failed: size={}, err={:?}",
                        file_size, e
                    );
                    false
                }
            }
        });
        if !initial_truncate_ok {
            self.direct_dirty.store(true, Ordering::Release);
            return (0, dirty_page_count != 0 || direct_dirty || has_more);
        }

        let mut flushed = 0usize;
        let mut write_failed = false;
        EXT4_FLUSH_PHASE.store(3, Ordering::Release);
        for batch in dirty_pages.chunks(EXT4_WRITEBACK_BATCH_PAGES) {
            let mut batch_complete = false;
            while !batch_complete {
                EXT4_FLUSH_PAGE_PHASE.store(1, Ordering::Release);
                // Page locks are selected before entering lwext4. This keeps
                // the mount gate out of page-lock wait paths while allowing a
                // run of adjacent pages to share one gate/file-handle lock.
                let mut locked_pages = Vec::with_capacity(batch.len());
                let mut busy_page = false;
                for (page_id, page_lock) in batch {
                    if let Some(page) = page_lock.try_write() {
                        locked_pages.push((*page_id, page));
                    } else {
                        busy_page = true;
                    }
                }

                if locked_pages.is_empty() {
                    has_more = true;
                    if max_pages.is_some() {
                        break;
                    }
                    crate::task::suspend_current_and_run_next();
                    continue;
                }

                // The common writeback case is a full run of adjacent dirty
                // pages.  Stage that run before entering lwext4 so one
                // ext4_fwrite transaction can cover all of its blocks.  Apart
                // from avoiding seven transaction/lock round trips for the
                // default eight-page batch, this lets lwext4 submit adjacent
                // physical blocks together.  Allocation failure or an
                // irregular batch falls back to the per-page path below.
                let coalesced_write = if locked_pages.len() > 1 {
                    let generation = inode.page_cache_generation();
                    let current_file_size = inode.get_size();
                    let first_page = locked_pages[0].0;
                    let first_offset = first_page.saturating_mul(PAGE_SIZE);
                    let mut expected_page = first_page;
                    let mut total_len = 0usize;
                    let mut eligible = generation & 1 == 0;

                    for (index, (page_id, page)) in locked_pages.iter().enumerate() {
                        let offset = page_id.saturating_mul(PAGE_SIZE);
                        if !eligible
                            || *page_id != expected_page
                            || !page.dirty
                            || page.dirty_generation() != generation
                            || offset >= current_file_size
                            || page.resident_frame().is_none()
                        {
                            eligible = false;
                            break;
                        }
                        let write_len = (current_file_size - offset).min(PAGE_SIZE);
                        // Only the final page in a run may end at a partial EOF.
                        if index + 1 != locked_pages.len() && write_len != PAGE_SIZE {
                            eligible = false;
                            break;
                        }
                        let Some(new_total) = total_len.checked_add(write_len) else {
                            eligible = false;
                            break;
                        };
                        total_len = new_total;
                        let Some(next_page) = expected_page.checked_add(1) else {
                            eligible = false;
                            break;
                        };
                        expected_page = next_page;
                    }

                    if eligible {
                        let mut buffer = Vec::new();
                        if buffer.try_reserve_exact(total_len).is_ok() {
                            for (page_id, page) in locked_pages.iter() {
                                let offset = *page_id * PAGE_SIZE;
                                let write_len = (current_file_size - offset).min(PAGE_SIZE);
                                let frame = page
                                    .resident_frame()
                                    .expect("coalesced writeback page lost its resident frame");
                                buffer.extend_from_slice(&frame.ppn.get_bytes_array()[..write_len]);
                            }
                            Some((
                                generation,
                                current_file_size,
                                first_page,
                                first_offset,
                                buffer,
                            ))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };

                let outcome = with_lwext4_mount_lock_op(
                    &self.mount_gate,
                    Lwext4Op::Writeback,
                    || {
                        let mut ext4file = self.ext4file.lock();
                        let mut batch_flushed = 0usize;
                        if let Some((
                            prepared_generation,
                            prepared_file_size,
                            first_page,
                            first_offset,
                            buffer,
                        )) = coalesced_write
                        {
                            // A direct writer may have changed the inode while
                            // the staging buffer was allocated.  In that rare
                            // case use the fully revalidating per-page path.
                            if inode.page_cache_generation() == prepared_generation
                                && inode.get_size() == prepared_file_size
                            {
                                EXT4_FLUSH_CURRENT_PAGE.store(first_page, Ordering::Release);
                                EXT4_FLUSH_CURRENT_PPN.store(usize::MAX, Ordering::Release);
                                EXT4_FLUSH_PAGE_PHASE.store(3, Ordering::Release);
                                if let Err(e) = ext4file.file_seek(first_offset as i64, SEEK_SET) {
                                    error!(
                                        "[EXT4_WRITEBACK_EIO] stage=batch_seek pid={} inode={} first_page={} pages={} offset={} len={} raw_error={} file_size={} ext4_flush={:?} block_io={:?}",
                                        pid,
                                        inode_id,
                                        first_page,
                                        locked_pages.len(),
                                        first_offset,
                                        buffer.len(),
                                        e,
                                        prepared_file_size,
                                        ext4_flush_stats(),
                                        crate::drivers::block::virtio_blk::virtio_block_io_stats(),
                                    );
                                    return Err(());
                                }
                                EXT4_FLUSH_PAGE_PHASE.store(4, Ordering::Release);
                                crate::fs::lwext4::record_lwext4_writeback_batch_source(
                                    buffer.as_ptr() as usize,
                                    buffer.len(),
                                    inode_id,
                                    first_page,
                                );
                                EXT4_FLUSH_PAGE_PHASE.store(5, Ordering::Release);
                                let written = match ext4file.file_write(&buffer) {
                                    Ok(written) => written,
                                    Err(e) => {
                                        error!(
                                            "[EXT4_WRITEBACK_EIO] stage=batch_write pid={} inode={} first_page={} pages={} offset={} len={} raw_error={} file_size={} ext4_flush={:?} block_io={:?}",
                                            pid,
                                            inode_id,
                                            first_page,
                                            locked_pages.len(),
                                            first_offset,
                                            buffer.len(),
                                            e,
                                            prepared_file_size,
                                            ext4_flush_stats(),
                                            crate::drivers::block::virtio_blk::virtio_block_io_stats(),
                                        );
                                        return Err(());
                                    }
                                };
                                EXT4_FLUSH_PAGE_PHASE.store(6, Ordering::Release);
                                if written != buffer.len() {
                                    error!(
                                        "[EXT4_WRITEBACK_EIO] stage=batch_short_write pid={} inode={} first_page={} pages={} offset={} expected={} written={} file_size={} ext4_flush={:?} block_io={:?}",
                                        pid,
                                        inode_id,
                                        first_page,
                                        locked_pages.len(),
                                        first_offset,
                                        buffer.len(),
                                        written,
                                        prepared_file_size,
                                        ext4_flush_stats(),
                                        crate::drivers::block::virtio_blk::virtio_block_io_stats(),
                                    );
                                    return Err(());
                                }
                                for (_, page) in locked_pages.iter_mut() {
                                    page.clear_dirty();
                                }
                                EXT4_WRITEBACK_COALESCED_BATCHES.fetch_add(1, Ordering::Relaxed);
                                EXT4_WRITEBACK_COALESCED_PAGES
                                    .fetch_add(locked_pages.len(), Ordering::Relaxed);
                                EXT4_FLUSH_CURRENT_PPN.store(usize::MAX, Ordering::Release);
                                EXT4_FLUSH_PAGE_PHASE.store(7, Ordering::Release);
                                return Ok(locked_pages.len());
                            }
                        }
                        for (page_id, page) in locked_pages.iter_mut() {
                            EXT4_FLUSH_CURRENT_PAGE.store(*page_id, Ordering::Release);
                            EXT4_FLUSH_PAGE_PHASE.store(2, Ordering::Release);
                            if !page.dirty {
                                EXT4_FLUSH_PAGE_PHASE.store(7, Ordering::Release);
                                continue;
                            }

                            let current_generation = inode.page_cache_generation();
                            if current_generation & 1 != 0
                                || page.dirty_generation() != current_generation
                            {
                                page.clear_dirty();
                                EXT4_FLUSH_PAGE_PHASE.store(7, Ordering::Release);
                                continue;
                            }

                            let current_file_size = inode.get_size();
                            let offset = *page_id * PAGE_SIZE;
                            if offset >= current_file_size {
                                page.clear_dirty();
                                EXT4_FLUSH_PAGE_PHASE.store(7, Ordering::Release);
                                continue;
                            }
                            let write_len = (current_file_size - offset).min(PAGE_SIZE);
                            let Some(frame) = page.resident_frame() else {
                                error!(
                                    "[EXT4_WRITEBACK_EIO] stage=resident_frame_missing pid={} inode={} page={} offset={} len={} file_size={} ext4_flush={:?} block_io={:?}",
                                    pid,
                                    inode_id,
                                    *page_id,
                                    offset,
                                    write_len,
                                    current_file_size,
                                    ext4_flush_stats(),
                                    crate::drivers::block::virtio_blk::virtio_block_io_stats(),
                                );
                                EXT4_FLUSH_PAGE_PHASE.store(7, Ordering::Release);
                                return Err(());
                            };
                            EXT4_FLUSH_CURRENT_PPN.store(frame.ppn.0, Ordering::Release);

                            EXT4_FLUSH_PAGE_PHASE.store(3, Ordering::Release);
                            if let Err(e) = ext4file.file_seek(offset as i64, SEEK_SET) {
                                error!(
                                    "[EXT4_WRITEBACK_EIO] stage=seek pid={} inode={} page={} offset={} len={} raw_error={} file_size={} ext4_flush={:?} block_io={:?}",
                                    pid,
                                    inode_id,
                                    *page_id,
                                    offset,
                                    write_len,
                                    e,
                                    current_file_size,
                                    ext4_flush_stats(),
                                    crate::drivers::block::virtio_blk::virtio_block_io_stats(),
                                );
                                warn!(
                                    "ext4 seek during flush failed: offset={}, err={:?}",
                                    offset, e
                                );
                                return Err(());
                            }
                            EXT4_FLUSH_PAGE_PHASE.store(4, Ordering::Release);
                            let buffer = &frame.ppn.get_bytes_array()[..write_len];
                            crate::fs::lwext4::record_lwext4_writeback_source(
                                buffer.as_ptr() as usize,
                                buffer.len(),
                                frame.ppn.0,
                                inode_id,
                                *page_id,
                            );
                            EXT4_FLUSH_PAGE_PHASE.store(5, Ordering::Release);
                            let written = match ext4file.file_write(buffer) {
                                Ok(written) => written,
                                Err(e) => {
                                    error!(
                                        "[EXT4_WRITEBACK_EIO] stage=write pid={} inode={} page={} offset={} len={} raw_error={} file_size={} ext4_flush={:?} block_io={:?}",
                                        pid,
                                        inode_id,
                                        *page_id,
                                        offset,
                                        write_len,
                                        e,
                                        current_file_size,
                                        ext4_flush_stats(),
                                        crate::drivers::block::virtio_blk::virtio_block_io_stats(),
                                    );
                                    warn!(
                                        "ext4 write during flush failed: offset={}, len={}, err={:?}",
                                        offset, write_len, e
                                    );
                                    return Err(());
                                }
                            };
                            EXT4_FLUSH_PAGE_PHASE.store(6, Ordering::Release);
                            if written != write_len {
                                error!(
                                    "[EXT4_WRITEBACK_EIO] stage=short_write pid={} inode={} page={} offset={} expected={} written={} file_size={} ext4_flush={:?} block_io={:?}",
                                    pid,
                                    inode_id,
                                    *page_id,
                                    offset,
                                    write_len,
                                    written,
                                    current_file_size,
                                    ext4_flush_stats(),
                                    crate::drivers::block::virtio_blk::virtio_block_io_stats(),
                                );
                                warn!(
                                    "ext4 short write during flush: offset={}, expected={}, written={}",
                                    offset, write_len, written
                                );
                                return Err(());
                            }
                            page.clear_dirty();
                            EXT4_FLUSH_CURRENT_PPN.store(usize::MAX, Ordering::Release);
                            batch_flushed += 1;
                            EXT4_FLUSH_PAGE_PHASE.store(7, Ordering::Release);
                        }
                        Ok(batch_flushed)
                    },
                );

                match outcome {
                    Ok(batch_flushed) => {
                        flushed += batch_flushed;
                        EXT4_FLUSH_PAGES_DONE.store(flushed, Ordering::Release);
                    }
                    Err(()) => {
                        write_failed = true;
                        has_more = true;
                    }
                }
                drop(locked_pages);

                if write_failed {
                    break;
                }
                if busy_page {
                    has_more = true;
                    if max_pages.is_none() {
                        crate::task::suspend_current_and_run_next();
                        continue;
                    }
                }
                batch_complete = true;
            }
            if write_failed {
                break;
            }
        }

        if write_failed {
            self.direct_dirty.store(true, Ordering::Release);
            return (flushed, true);
        }

        // A later writer can set direct_dirty concurrently; this flush never
        // clears that newer request. The initial truncate already established
        // the requested size, so avoid a second identical ext4 transaction in
        // the ordinary case. A concurrent truncate/growth changes either the
        // generation or size and still takes the final correction path.
        let final_file_size = inode.get_size();
        EXT4_FLUSH_FILE_SIZE.store(final_file_size, Ordering::Release);
        EXT4_FLUSH_PHASE.store(4, Ordering::Release);
        let final_generation = inode.page_cache_generation();
        let final_truncate_ok = if final_generation == flush_generation
            && final_file_size == file_size
        {
            true
        } else {
            self.with_ext4file_op(Lwext4Op::Writeback, |ext4file| {
                let current_generation = inode.page_cache_generation();
                if current_generation & 1 != 0 {
                    return true;
                }
                let current_file_size = inode.get_size();
                EXT4_FLUSH_FILE_SIZE.store(current_file_size, Ordering::Release);
                match ext4file.file_truncate(current_file_size as u64) {
                    Ok(_) => true,
                    Err(e) => {
                        error!(
                            "[EXT4_WRITEBACK_EIO] stage=final_truncate pid={} inode={} file_size={} raw_error={} ext4_flush={:?} block_io={:?}",
                            pid,
                            inode_id,
                            final_file_size,
                            e,
                            ext4_flush_stats(),
                            crate::drivers::block::virtio_blk::virtio_block_io_stats(),
                        );
                        warn!(
                            "file_truncate after flush failed: size={}, err={:?}",
                            final_file_size, e
                        );
                        false
                    }
                }
            })
        };
        if !final_truncate_ok {
            self.direct_dirty.store(true, Ordering::Release);
            return (flushed, true);
        }

        EXT4_FLUSH_PHASE.store(5, Ordering::Release);
        let cache_flush_ok = self.with_ext4file_op(Lwext4Op::Writeback, |ext4file| match ext4file
            .file_cache_flush()
        {
            Ok(_) => true,
            Err(e) => {
                error!(
                    "[EXT4_WRITEBACK_EIO] stage=cache_flush pid={} inode={} file_size={} raw_error={} ext4_flush={:?} block_io={:?}",
                    pid,
                    inode_id,
                    final_file_size,
                    e,
                    ext4_flush_stats(),
                    crate::drivers::block::virtio_blk::virtio_block_io_stats(),
                );
                warn!("ext4 cache flush failed: {:?}", e);
                false
            }
        });
        if !cache_flush_ok {
            self.direct_dirty.store(true, Ordering::Release);
        }
        (flushed, has_more)
    }
}

fn trim_cached_pages_after_size(cache_inode_id: usize, new_size: usize) -> SysResult<()> {
    let tail_offset = new_size % PAGE_SIZE;
    let first_removed_page = new_size.div_ceil(PAGE_SIZE);
    let tail_page = (tail_offset != 0)
        .then(|| PAGE_CACHE.get_page(cache_inode_id, new_size / PAGE_SIZE))
        .flatten();
    if let Some(page) = tail_page {
        let mut page = page.write();
        let was_dirty = page.dirty;
        page.ensure_resident()?.ppn.get_bytes_array()[tail_offset..].fill(0);
        page.dirty = was_dirty;
    }
    PAGE_CACHE.remove_inode_pages_from(cache_inode_id, first_removed_page);
    Ok(())
}

impl Drop for Ext4File {
    fn drop(&mut self) {
        with_lwext4_mount_read_lock_op(&self.mount_gate, Lwext4Op::OpenClose, || {
            let mut ext4file = self.ext4file.lock();
            let _ = ext4file.file_close();
        });
    }
}

impl File for Ext4File {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        self.inner.lock()
    }
    fn get_inode(&self) -> Option<Arc<dyn Inode>> {
        Some(self.inode.clone())
    }
    fn seek_position(&self, offset: isize, whence: i32) -> SysResult<usize> {
        const SEEK_SET: i32 = 0;
        const SEEK_CUR: i32 = 1;
        const SEEK_END: i32 = 2;

        let mut inner = self.inner.lock();
        let inode = inner.dentry.get_inode().ok_or(SysError::ESPIPE)?;
        let new_off = match whence {
            SEEK_SET => offset,
            SEEK_CUR => (inner.offset as isize).saturating_add(offset),
            SEEK_END => {
                if inode.get_mode().get_type() == InodeMode::DIR {
                    return Err(SysError::EINVAL);
                }
                (inode.get_size() as isize).saturating_add(offset)
            }
            _ => return Err(SysError::EINVAL),
        };
        if new_off < 0 {
            return Err(SysError::EINVAL);
        }
        inner.offset = new_off as usize;
        Ok(new_off as usize)
    }
    fn readable(&self) -> bool {
        self.readable
    }
    fn writable(&self) -> bool {
        self.writable
    }
    fn is_append(&self) -> bool {
        self.append
    }
    fn supports_sparse_holes(&self) -> bool {
        true
    }
    fn read_all(&self) -> Vec<u8> {
        let size = self
            .inner
            .lock()
            .dentry
            .get_inode()
            .map(|inode| inode.get_size())
            .unwrap_or(0);
        let mut data = vec![0u8; size];
        if size == 0 {
            return data;
        }

        let read_len = self.with_ext4file_op(Lwext4Op::Read, |ext4file| {
            if ext4file.file_seek(0, SEEK_SET).is_err() {
                return 0;
            }
            let mut offset = 0usize;
            while offset < size {
                match ext4file.file_read(&mut data[offset..]) {
                    Ok(0) => break,
                    Ok(n) => offset += n,
                    Err(_) => break,
                }
            }
            offset
        });
        if read_len == 0 {
            return Vec::new();
        }
        data.truncate(read_len);
        data
    }
    //read the data
    fn read(&self, mut buf: UserBuffer) -> SysResult<usize> {
        let request_len = buf.len();
        let (inode, should_update_atime, dentry, start_offset, reserved_len, reserved_end) = {
            let mut inner = self.get_fileinner();
            let inode = inner.dentry.get_inode().unwrap();
            let should_update_atime = !inner.flags.contains(OpenFlags::O_NOATIME)
                && buf.buffers.iter().any(|slice| !slice.is_empty());
            // 使用 inode 中缓存的大小，而不是 ext4 文件描述符中的大小
            // 因为 ext4 文件描述符的 fsize 可能没有及时更新
            let file_size = inode.get_size();
            let start_offset = inner.offset;
            if start_offset >= file_size || request_len == 0 {
                return Ok(0);
            }
            let reserved_len = request_len.min(file_size - start_offset);
            let reserved_end = start_offset + reserved_len;
            inner.offset = reserved_end;
            let dentry = if should_update_atime {
                Some(inner.dentry.clone())
            } else {
                None
            };
            (
                inode,
                should_update_atime,
                dentry,
                start_offset,
                reserved_len,
                reserved_end,
            )
        };
        let (total_read_size, should_flush_cache) =
            self.read_cached_at(&inode, start_offset, reserved_len, &mut buf)?;
        if total_read_size != reserved_len {
            let actual_end = start_offset + total_read_size;
            let mut inner = self.inner.lock();
            if inner.offset == reserved_end {
                inner.offset = actual_end;
            }
        }
        if should_update_atime && total_read_size > 0 {
            if let Some(dentry) = dentry {
                crate::syscall::maybe_update_atime_for_dentry(&dentry, &inode, false);
            }
        }
        if should_flush_cache {
            crate::fs::writeback::request_writeback();
        }
        Ok(total_read_size)
    }

    fn read_at(&self, offset: usize, mut buf: UserBuffer) -> SysResult<usize> {
        let (inode, should_update_atime, dentry) = {
            let inner = self.get_fileinner();
            let inode = inner.dentry.get_inode().unwrap();
            let should_update_atime = !inner.flags.contains(OpenFlags::O_NOATIME)
                && buf.buffers.iter().any(|slice| !slice.is_empty());
            let dentry = if should_update_atime {
                Some(inner.dentry.clone())
            } else {
                None
            };
            (inode, should_update_atime, dentry)
        };
        let max_len = buf.len();
        let (total_read_size, should_flush_cache) =
            self.read_cached_at(&inode, offset, max_len, &mut buf)?;
        if should_update_atime && total_read_size > 0 {
            if let Some(dentry) = dentry {
                crate::syscall::maybe_update_atime_for_dentry(&dentry, &inode, false);
            }
        }
        if should_flush_cache {
            crate::fs::writeback::request_writeback();
        }
        Ok(total_read_size)
    }

    fn read_at_direct(&self, offset: usize, buf: &mut [u8]) -> SysResult<usize> {
        if !self.readable() {
            return Err(SysError::EBADF);
        }
        if buf.is_empty() {
            return Ok(0);
        }
        let inode = self.get_inode().ok_or(SysError::EIO)?;
        let file_size = inode.get_size();
        if offset >= file_size {
            return Ok(0);
        }
        let mut done = 0usize;
        let total_len = (file_size - offset).min(buf.len());
        while done < total_len {
            let pos = offset + done;
            let page_id = pos / PAGE_SIZE;
            let page_offset = pos % PAGE_SIZE;
            if inode.is_punched_hole_page(page_id) {
                let read_len = (PAGE_SIZE - page_offset).min(total_len - done);
                buf[done..done + read_len].fill(0);
                done += read_len;
                continue;
            }

            // Read all consecutive non-hole pages in one lwext4 operation.
            // The previous page-at-a-time loop made execve of large tools issue
            // thousands of seek/read pairs while repeatedly taking the same
            // mount gate.
            const DIRECT_READ_RUN_MAX: usize = 64 * 1024;
            let run_start = done;
            let mut run_end = done;
            let max_run_end = run_start.saturating_add(DIRECT_READ_RUN_MAX).min(total_len);
            while run_end < max_run_end {
                let run_pos = offset + run_end;
                let run_page = run_pos / PAGE_SIZE;
                if inode.is_punched_hole_page(run_page) {
                    break;
                }
                let page_end = (run_page + 1).checked_mul(PAGE_SIZE).unwrap_or(usize::MAX);
                run_end = max_run_end.min(page_end.saturating_sub(offset));
            }
            let n = self.with_ext4file_op(Lwext4Op::Read, |ext4file| {
                ext4file
                    .file_seek(pos as i64, SEEK_SET)
                    .map_err(crate::fs::lwext4::lwext4_err_to_sys)?;
                ext4file
                    .file_read(&mut buf[run_start..run_end])
                    .map_err(crate::fs::lwext4::lwext4_err_to_sys)
            })?;
            if n == 0 {
                break;
            }
            done += n;
            if n < run_end - run_start {
                break;
            }
        }
        Ok(done)
    }

    fn write_at(&self, offset: usize, buf: UserBuffer) -> SysResult<usize> {
        let inode = {
            let inner = self.inner.lock();
            inner.dentry.get_inode().unwrap()
        };
        let _write_guard = Ext4PageCacheWriteGuard::new(inode.clone());
        if inode.get_fs_flags()
            & (crate::fs::vfs::inode::FS_IMMUTABLE_FL | crate::fs::vfs::inode::FS_APPEND_FL)
            != 0
        {
            return Err(SysError::EPERM);
        }
        let old_size = inode.get_size();
        let (total_write_size, should_flush_cache) =
            self.write_cached_at(&inode, offset, old_size, &buf)?;
        if should_flush_cache {
            crate::fs::writeback::request_writeback();
        }
        Ok(total_write_size)
    }

    fn write(&self, buf: UserBuffer) -> SysResult<usize> {
        // info!("enter VFS Write-back Cache");
        let request_len = buf.len();
        let (inode, write_guard, old_size, start_offset, reserved_end, size_reserved) = {
            let mut inner = self.inner.lock();
            let inode = inner.dentry.get_inode().unwrap();
            let write_guard = Ext4PageCacheWriteGuard::new(inode.clone());
            if inode.get_fs_flags()
                & (crate::fs::vfs::inode::FS_IMMUTABLE_FL | crate::fs::vfs::inode::FS_APPEND_FL)
                != 0
            {
                return Err(SysError::EPERM);
            }
            let old_size = inode.get_size();
            let append = inner.flags.contains(OpenFlags::O_APPEND);
            let start_offset = if append { old_size } else { inner.offset };
            let reserved_end = start_offset
                .checked_add(request_len)
                .ok_or(SysError::EFBIG)?;
            let size_reserved = append && reserved_end > old_size;
            if size_reserved {
                // Append must reserve its range before releasing the shared
                // file-description lock. Ordinary writes publish size only
                // after bytes have actually entered the page cache.
                inode.extend_size(reserved_end);
            }
            inner.offset = reserved_end;
            (
                inode,
                write_guard,
                old_size,
                start_offset,
                reserved_end,
                size_reserved,
            )
        };
        let (total_write_size, should_flush_cache) =
            match self.write_cached_at(&inode, start_offset, old_size, &buf) {
                Ok(result) => result,
                Err(err) => {
                    let mut inner = self.inner.lock();
                    let owns_offset_reservation = inner.offset == reserved_end;
                    if owns_offset_reservation {
                        inner.offset = start_offset;
                    }
                    drop(inner);
                    if size_reserved && owns_offset_reservation {
                        inode.replace_size_if_current(reserved_end, old_size);
                    }
                    return Err(err);
                }
            };
        if total_write_size != request_len {
            let actual_end = start_offset + total_write_size;
            let mut inner = self.inner.lock();
            let owns_offset_reservation = inner.offset == reserved_end;
            if owns_offset_reservation {
                inner.offset = actual_end;
            }
            drop(inner);
            if size_reserved && owns_offset_reservation {
                inode.replace_size_if_current(reserved_end, old_size.max(actual_end));
            }
        }
        if should_flush_cache {
            crate::fs::writeback::request_writeback();
        }
        drop(write_guard);
        Ok(total_write_size)
    }

    fn write_at_direct(&self, offset: usize, buf: &[u8]) -> SysResult<usize> {
        if !self.writable() {
            return Err(SysError::EBADF);
        }
        if buf.is_empty() {
            return Ok(0);
        }
        self.clear_hot_pages();
        let inode = self.get_inode().ok_or(SysError::EIO)?;
        if inode.get_fs_flags()
            & (crate::fs::vfs::inode::FS_IMMUTABLE_FL | crate::fs::vfs::inode::FS_APPEND_FL)
            != 0
        {
            return Err(SysError::EPERM);
        }
        let mut written = 0usize;
        while written < buf.len() {
            let pos = offset + written;
            let page_id = pos / PAGE_SIZE;
            let page_offset = pos % PAGE_SIZE;
            let write_len = (PAGE_SIZE - page_offset).min(buf.len() - written);
            let overwrites_whole_page = page_offset == 0 && write_len == PAGE_SIZE;
            let page_was_hole = inode.is_punched_hole_page(page_id);

            if page_was_hole && !overwrites_whole_page {
                let mut page = [0u8; PAGE_SIZE];
                page[page_offset..page_offset + write_len]
                    .copy_from_slice(&buf[written..written + write_len]);
                let n = self.with_ext4file_op(Lwext4Op::Write, |ext4file| {
                    ext4file
                        .file_seek((page_id * PAGE_SIZE) as i64, SEEK_SET)
                        .map_err(crate::fs::lwext4::lwext4_err_to_sys)?;
                    ext4file
                        .file_write(&page)
                        .map_err(crate::fs::lwext4::lwext4_err_to_sys)
                })?;
                if n != PAGE_SIZE {
                    if written > 0 {
                        return Ok(written);
                    }
                    return Err(SysError::EIO);
                }
                inode.clear_punched_hole_page(page_id);
                written += write_len;
            } else {
                let n = self.with_ext4file_op(Lwext4Op::Write, |ext4file| {
                    ext4file
                        .file_seek(pos as i64, SEEK_SET)
                        .map_err(crate::fs::lwext4::lwext4_err_to_sys)?;
                    ext4file
                        .file_write(&buf[written..written + write_len])
                        .map_err(crate::fs::lwext4::lwext4_err_to_sys)
                })?;
                if n == 0 {
                    break;
                }
                inode.clear_punched_hole_page(page_id);
                written += n;
            }
        }
        if written > 0 {
            let end = offset + written;
            inode.extend_size(end);
            let (now_sec, now_nsec) = realtime_timespec();
            inode.set_mtime(now_sec, now_nsec);
            inode.set_ctime(now_sec, now_nsec);
            self.direct_dirty.store(true, Ordering::Release);
        }
        Ok(written)
    }

    fn ls(&self) -> Vec<(String, u64, u8)> {
        self.get_fileinner().dentry.ls()
    }

    fn get_stat(&self, stat: &mut Kstat) -> SysResult<()> {
        match self.get_dentry().get_stat(stat) {
            Ok(()) => return Ok(()),
            // An open-but-unlinked or renamed file can no longer be resolved
            // by its original dentry path. Preserve Linux fstat semantics by
            // falling back to the still-live lwext4 file descriptor.
            Err(SysError::ENOENT) => {}
            Err(error) => return Err(error),
        }
        let inode = self.get_inode().ok_or(SysError::EIO)?;
        let disk = self.with_ext4file_op(Lwext4Op::Stat, |ext4file| {
            if ext4file.file_desc.mp.is_null() {
                None
            } else {
                Some(ExtFS::file_stat(&mut ext4file.file_desc))
            }
        });
        let Some(disk) = disk else {
            return self.get_dentry().get_stat(stat);
        };
        fill_ext4_kstat(inode.as_ref(), &disk?, stat);
        Ok(())
    }

    ///
    fn flush(&self) {
        info!("enter VFS flush (write-back to disk)");
        self.flush_dirty_pages(None);
        info!("finish VFS flush");
    }

    fn fsync(&self) -> SysResult<()> {
        let (flushed_pages, has_more) = self.flush_dirty_pages(None);
        let direct_dirty = self.direct_dirty.load(Ordering::Acquire);
        if has_more || direct_dirty {
            let inode_id = self
                .get_inode()
                .map(|inode| inode.cache_inode_id().unwrap_or_else(|| inode.get_ino()));
            error!(
                "[EXT4_WRITEBACK_EIO] stage=fsync_incomplete inode={:?} flushed_pages={} has_more={} direct_dirty={} ext4_flush={:?} block_io={:?}",
                inode_id,
                flushed_pages,
                has_more,
                direct_dirty,
                ext4_flush_stats(),
                crate::drivers::block::virtio_blk::virtio_block_io_stats(),
            );
            return Err(SysError::EIO);
        }
        crate::fs::lwext4::flush_lwext4_mount(&self.mount_gate).map_err(|err| {
            if err == SysError::EIO {
                error!(
                    "[EXT4_WRITEBACK_EIO] stage=mount_flush inode={:?} error={:?} ext4_flush={:?} block_io={:?}",
                    self.get_inode().map(|inode| inode
                        .cache_inode_id()
                        .unwrap_or_else(|| inode.get_ino())),
                    err,
                    ext4_flush_stats(),
                    crate::drivers::block::virtio_blk::virtio_block_io_stats(),
                );
            }
            err
        })
    }

    fn has_private_writeback_state(&self) -> bool {
        self.direct_dirty.load(Ordering::Acquire)
    }

    fn flush_pages(&self, max_pages: usize) -> (usize, bool) {
        self.flush_dirty_pages(Some(max_pages))
    }

    fn ioctl(&self, request: usize, argp: usize) -> SyscallResult {
        let inode = self.get_inode().ok_or(SysError::EIO)?;
        match request {
            FS_IOC_GETFLAGS => ioctl_get_fs_flags(inode, argp),
            FS_IOC_SETFLAGS => ioctl_set_fs_flags(inode, argp),
            _ => Err(SysError::ENOTTY),
        }
    }

    fn truncate(&self, size: u64) -> SyscallResult {
        if let Some(inode) = self.get_inode() {
            if inode.get_fs_flags()
                & (crate::fs::vfs::inode::FS_IMMUTABLE_FL | crate::fs::vfs::inode::FS_APPEND_FL)
                != 0
            {
                return Err(SysError::EPERM);
            }
        }
        let inode = self.get_inode().ok_or(SysError::EIO)?;
        let old_size = inode.get_size();
        let new_size = size as usize;
        self.flush_dirty_pages(None);
        let _truncate_guard = Ext4PageCacheWritebackGuard::new(inode.clone());
        self.clear_hot_pages();
        let destructive = new_size < old_size;
        if destructive {
            Self::discard_closed_writeback_before_truncate(&inode);
        }
        let mut invalidation = None;
        let res = self.with_ext4file_op(Lwext4Op::Truncate, |ext4file| {
            if destructive {
                invalidation = Some(PageCacheInvalidationGuard::new(inode.as_ref()));
            }
            ext4file.file_truncate(size)
        });
        if let Err(err) = res {
            if let Some(invalidation) = invalidation.take() {
                invalidation.abort();
            }
            let mapped = crate::fs::lwext4::lwext4_err_to_sys(err);
            if mapped == SysError::EIO {
                error!(
                    "[EXT4_WRITEBACK_EIO] stage=truncate inode={} old_size={} new_size={} raw_error={} ext4_flush={:?} block_io={:?}",
                    inode.cache_inode_id().unwrap_or_else(|| inode.get_ino()),
                    old_size,
                    new_size,
                    err,
                    ext4_flush_stats(),
                    crate::drivers::block::virtio_blk::virtio_block_io_stats(),
                );
            }
            return Err(mapped);
        }
        if destructive {
            let trim_result = trim_cached_pages_after_size(
                inode.cache_inode_id().unwrap_or_else(|| inode.get_ino()),
                new_size,
            );
            inode.truncate_punched_holes(new_size);
            inode.set_size(new_size);
            invalidation
                .take()
                .expect("truncate invalidation guard missing")
                .commit();
            trim_result?;
        } else {
            inode.set_size(new_size);
        }
        Ok(0)
    }

    fn get_cache_frame(&self, page_id: usize) -> Option<Arc<FrameTracker>> {
        let inode = self.inode.clone();
        let ino = inode.cache_inode_id().unwrap_or_else(|| inode.get_ino());
        let file_size = inode.get_size();
        let (target_page, under_pressure) = self
            .get_or_load_cache_page_window(ino, page_id, file_size, EXT4_MMAP_READAHEAD_PAGES)
            .ok()?;
        if under_pressure && self.writable() {
            crate::fs::writeback::request_writeback();
        }
        target_page.read().resident_frame()
    }

    fn populate_page_cache(&self, offset: usize, len: usize) -> SysResult<usize> {
        if len == 0 {
            return Ok(0);
        }
        let inode = self.get_inode().ok_or(SysError::EIO)?;
        let file_size = inode.get_size();
        if offset >= file_size {
            return Ok(0);
        }
        let ino = inode.cache_inode_id().unwrap_or_else(|| inode.get_ino());
        let end = offset.saturating_add(len).min(file_size);
        let start_page = offset / PAGE_SIZE;
        let end_page = (end + PAGE_SIZE - 1) / PAGE_SIZE;
        let mut should_flush_cache = false;
        for page_id in start_page..end_page {
            if inode.is_punched_hole_page(page_id) {
                continue;
            }
            let (_, under_pressure) = self.get_or_load_cache_page(ino, page_id, file_size)?;
            should_flush_cache |= under_pressure && self.writable();
        }
        if should_flush_cache {
            crate::fs::writeback::request_writeback();
        }
        Ok(end - offset)
    }
}

impl OpenFlags {
    /// Convert OpenFlags to ext4 open flags (O_RDONLY, O_WRONLY, O_RDWR)
    pub fn into_ext4_flags(&self) -> u32 {
        match self.bits() & 0o3 {
            0o1 => O_WRONLY,
            0o2 => O_RDWR,
            _ => O_RDONLY,
        }
    }
}
