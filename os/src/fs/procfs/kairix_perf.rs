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
use alloc::vec::Vec;
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
        true
    }

    fn read(&self, mut buf: UserBuffer) -> SysResult<usize> {
        let mut inner = self.get_fileinner();
        let recycle = crate::task::recycle_allocator_perf_stats();
        let deferred = crate::task::deferred_exited_task_stats();
        let lifecycle = crate::task::task::task_lifecycle_stats();
        let perf = crate::task::perf_stats::snapshot();
        let info = format!(
            "recycle_dealloc_calls: {}\n\
             recycle_dealloc_scan_total: {}\n\
             recycle_dealloc_scan_max: {}\n\
             deferred_exited_pushes: {}\n\
             deferred_exited_reaps: {}\n\
             deferred_exited_max: {}\n\
             deferred_exited_current: {}\n\
             task_created: {}\n\
             task_dropped: {}\n\
             task_live_delta: {}\n\
             clone_thread_calls: {}\n\
             clone_thread_ns_total: {}\n\
             clone_thread_ns_max: {}\n\
             clone_process_calls: {}\n\
             clone_process_ns_total: {}\n\
             clone_process_ns_max: {}\n\
             exit_calls: {}\n\
             exit_ns_total: {}\n\
             exit_ns_max: {}\n\
             kstack_alloc_calls: {}\n\
             kstack_alloc_ns_total: {}\n\
             kstack_alloc_ns_max: {}\n\
             tcb_new_calls: {}\n\
             tcb_new_ns_total: {}\n\
             tcb_new_ns_max: {}\n\
             task_user_res_new_calls: {}\n\
             task_user_res_new_ns_total: {}\n\
             task_user_res_new_ns_max: {}\n\
             futex_wait_calls: {}\n\
             futex_wait_ns_total: {}\n\
             futex_wait_ns_max: {}\n\
             futex_wait_block_calls: {}\n\
             futex_wait_suspend_calls: {}\n\
             futex_wake_calls: {}\n\
             futex_wake_ns_total: {}\n\
             futex_wake_ns_max: {}\n\
             futex_wake_woken_total: {}\n\
             futex_wake_one_calls: {}\n\
             futex_wake_one_ns_total: {}\n\
             futex_wake_one_ns_max: {}\n\
             futex_wake_one_woken_total: {}\n\
             block_calls: {}\n\
             block_schedule_calls: {}\n\
             block_fast_return_calls: {}\n\
             suspend_calls: {}\n\
             suspend_schedule_calls: {}\n\
             preempt_calls: {}\n\
             preempt_schedule_calls: {}\n\
             first_run_calls: {}\n\
             first_run_ns_total: {}\n\
             first_run_ns_max: {}\n\
             ready_queue_pushes: {}\n\
             ready_queue_fetches: {}\n\
             ready_queue_max_len: {}\n\
             proc_smaps_read_calls: {}\n\
             proc_smaps_render_calls: {}\n\
             proc_smaps_render_ns_total: {}\n\
             proc_smaps_render_ns_max: {}\n\
             proc_smaps_render_areas_total: {}\n\
             proc_smaps_render_bytes_total: {}\n\
             mmap_calls: {}\n\
             mmap_ns_total: {}\n\
             mmap_ns_max: {}\n\
             munmap_calls: {}\n\
             munmap_ns_total: {}\n\
             munmap_ns_max: {}\n\
             mprotect_calls: {}\n\
             mprotect_ns_total: {}\n\
             mprotect_ns_max: {}\n",
            recycle.dealloc_calls,
            recycle.dealloc_scan_total,
            recycle.dealloc_scan_max,
            deferred.pushes,
            deferred.reaps,
            deferred.max_pending,
            deferred.current_pending,
            lifecycle.created,
            lifecycle.dropped,
            lifecycle.live_delta,
            perf.clone_thread_calls,
            perf.clone_thread_ns_total,
            perf.clone_thread_ns_max,
            perf.clone_process_calls,
            perf.clone_process_ns_total,
            perf.clone_process_ns_max,
            perf.exit_calls,
            perf.exit_ns_total,
            perf.exit_ns_max,
            perf.kstack_alloc_calls,
            perf.kstack_alloc_ns_total,
            perf.kstack_alloc_ns_max,
            perf.tcb_new_calls,
            perf.tcb_new_ns_total,
            perf.tcb_new_ns_max,
            perf.task_user_res_new_calls,
            perf.task_user_res_new_ns_total,
            perf.task_user_res_new_ns_max,
            perf.futex_wait_calls,
            perf.futex_wait_ns_total,
            perf.futex_wait_ns_max,
            perf.futex_wait_block_calls,
            perf.futex_wait_suspend_calls,
            perf.futex_wake_calls,
            perf.futex_wake_ns_total,
            perf.futex_wake_ns_max,
            perf.futex_wake_woken_total,
            perf.futex_wake_one_calls,
            perf.futex_wake_one_ns_total,
            perf.futex_wake_one_ns_max,
            perf.futex_wake_one_woken_total,
            perf.block_calls,
            perf.block_schedule_calls,
            perf.block_fast_return_calls,
            perf.suspend_calls,
            perf.suspend_schedule_calls,
            perf.preempt_calls,
            perf.preempt_schedule_calls,
            perf.first_run_calls,
            perf.first_run_ns_total,
            perf.first_run_ns_max,
            perf.ready_queue_pushes,
            perf.ready_queue_fetches,
            perf.ready_queue_max_len,
            perf.proc_smaps_read_calls,
            perf.proc_smaps_render_calls,
            perf.proc_smaps_render_ns_total,
            perf.proc_smaps_render_ns_max,
            perf.proc_smaps_render_areas_total,
            perf.proc_smaps_render_bytes_total,
            perf.mmap_calls,
            perf.mmap_ns_total,
            perf.mmap_ns_max,
            perf.munmap_calls,
            perf.munmap_ns_total,
            perf.munmap_ns_max,
            perf.mprotect_calls,
            perf.mprotect_ns_total,
            perf.mprotect_ns_max
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

    fn write(&self, buf: UserBuffer) -> SysResult<usize> {
        let len = buf.len();
        let mut data = Vec::new();
        for slice in buf.buffers.iter() {
            data.extend_from_slice(slice);
        }
        let command = core::str::from_utf8(&data).unwrap_or("").trim();
        if !matches!(command, "0" | "reset" | "clear") {
            return Err(SysError::EINVAL);
        }

        crate::task::reset_recycle_allocator_perf_stats();
        crate::task::reset_deferred_exited_task_stats();
        crate::task::task::reset_task_lifecycle_stats();
        crate::task::perf_stats::reset();

        let mut inner = self.get_fileinner();
        inner.offset = 0;
        if let Some(inode) = inner.dentry.get_inode() {
            inode.set_size(KAIRIX_PERF_INITIAL_SIZE);
        }
        Ok(len)
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
