#![allow(missing_docs)]
use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::vfs::DentryInner;
use crate::fs::vfs::FileInner;
use crate::fs::vfs::OpenFlags;
use crate::fs::vfs::inode::InodeInner;
use crate::fs::vfs::inode::InodeMode;
use crate::fs::vfs::inode::inode_alloc;
use crate::fs::{Dentry, File, Inode};
use crate::mm::UserBuffer;
use alloc::format;
use alloc::sync::{Arc, Weak};
use core::sync::atomic::Ordering;
use spin::{Mutex, MutexGuard};

const KAIRIX_PERF_INITIAL_SIZE: usize = 8192;

pub struct KairixPerfFile {
    inner: Mutex<FileInner>,
}

impl KairixPerfFile {
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

impl File for KairixPerfFile {
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
        let lifecycle = crate::task::task::task_lifecycle_stats();
        let deferred_exited = crate::task::deferred_exited_task_count();
        let processors = crate::task::processor::processor_task_stats();
        let load_balance = crate::task::manager::load_balance_stats();
        let task_states = crate::task::manager::task_state_stats();
        let page_cache = crate::fs::page::pagecache::atomic_stats();
        let page_cache_lock = crate::fs::page::pagecache::PAGE_CACHE.stats();
        let lwext4_lock = crate::fs::lwext4::lwext4_lock_stats();
        let ext4_flush = crate::fs::lwext4::file::ext4_flush_stats();
        let block_io = crate::drivers::block::virtio_blk::virtio_block_io_stats();
        let writeback_pending = crate::fs::writeback::try_pending_count();
        let info = format!(
            "task_created: {}\n\
             task_dropped: {}\n\
             task_live_delta: {}\n\
             deferred_exited_current: {}\n\
             processor_current_tasks: {}\n\
             processor_locked: {}\n\
             load_balance_remote_enqueues: {}\n\
             load_balance_steal_attempts: {}\n\
             load_balance_steal_successes: {}\n\
             load_balance_ready_tasks: {:?}\n\
             load_balance_online_mask: {:#x}\n\
             task_state_process_table_busy: {}\n\
             task_state_process_locks_busy: {}\n\
             task_state_first_busy_process_pid: {}\n\
             task_state_first_busy_process_owner_cpu: {}\n\
             task_state_first_busy_process_owner_line: {}\n\
             task_state_task_locks_busy: {}\n\
             task_state_total: {}\n\
             task_state_ready: {}\n\
             task_state_running: {}\n\
             task_state_blocked: {}\n\
             task_state_zombie: {}\n\
             task_state_sleep: {}\n\
             task_state_ready_unowned: {}\n\
             task_state_running_not_on_cpu: {}\n\
             task_state_blocked_queued: {}\n\
             task_state_workload_sample_count: {}\n\
             task_state_workload_samples: {:?}\n\
             task_state_workload_context_samples: {:?}\n\
             page_cache_pages: {}\n\
             page_cache_tmpfs_pages: {}\n\
             page_cache_fat32_pages: {}\n\
             page_cache_ext4_pages: {}\n\
             page_cache_unknown_pages: {}\n\
             page_cache_insert_count: {}\n\
             page_cache_remove_count: {}\n\
             page_cache_lock: {:?}\n\
             lwext4_lock: {:?}\n\
             ext4_flush: {:?}\n\
             block_io: {:?}\n\
             writeback_pending_files: {:?}\n",
            lifecycle.created,
            lifecycle.dropped,
            lifecycle.live_delta,
            deferred_exited,
            processors.current_tasks,
            processors.locked_processors,
            load_balance.remote_enqueues,
            load_balance.steal_attempts,
            load_balance.steal_successes,
            load_balance.ready_tasks,
            load_balance.online_mask,
            task_states.process_table_busy,
            task_states.process_locks_busy,
            task_states.first_busy_process_pid,
            task_states.first_busy_process_owner_cpu,
            task_states.first_busy_process_owner_line,
            task_states.task_locks_busy,
            task_states.total,
            task_states.ready,
            task_states.running,
            task_states.blocked,
            task_states.zombie,
            task_states.sleep,
            task_states.ready_unowned,
            task_states.running_not_on_cpu,
            task_states.blocked_queued,
            task_states.workload_sample_count,
            task_states.workload_samples,
            task_states.workload_context_samples,
            page_cache.pages,
            page_cache.tmpfs_pages,
            page_cache.fat32_pages,
            page_cache.ext4_pages,
            page_cache.unknown_pages,
            page_cache.insert_count,
            page_cache.remove_count,
            page_cache_lock,
            lwext4_lock,
            ext4_flush,
            block_io,
            writeback_pending
        );

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
        Err(SysError::EROFS)
    }

    fn open(&self) -> SyscallResult {
        Ok(0)
    }

    fn release(&self) -> SyscallResult {
        Ok(0)
    }
}

pub struct KairixPerfDentry {
    inner: DentryInner,
}

impl KairixPerfDentry {
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|_me: &Weak<KairixPerfDentry>| Self {
            inner: DentryInner::new(name, parent_weak),
        })
    }
}

impl Dentry for KairixPerfDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }

    fn name(&self) -> &str {
        &self.inner.name
    }

    fn open(self: Arc<Self>, _flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Ok(Arc::new(KairixPerfFile::new(self)))
    }
}

pub struct KairixPerfInode {
    inner: InodeInner,
}

impl KairixPerfInode {
    pub fn new() -> Self {
        let mode =
            InodeMode::FILE | InodeMode::OWNER_READ | InodeMode::GROUP_READ | InodeMode::OTHER_READ;
        Self {
            inner: InodeInner::new(inode_alloc(), KAIRIX_PERF_INITIAL_SIZE, mode, 0),
        }
    }
}

impl Inode for KairixPerfInode {
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
