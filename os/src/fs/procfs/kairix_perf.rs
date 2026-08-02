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
use core::fmt::Write as _;
use core::sync::atomic::Ordering;
use spin::{Mutex, MutexGuard};

const KAIRIX_PERF_INITIAL_SIZE: usize = 8192;
const BUILDSTORM_KERNEL_DIAG_VERSION: &str = "2026-08-02.15";

#[derive(Default)]
struct BuildstormExt4Stats {
    mount_acquisitions: usize,
    mount_contentions: usize,
    mount_wait_ns: usize,
    namespace_acquisitions: usize,
    namespace_contentions: usize,
    namespace_wait_ns: usize,
    active_readers: usize,
    peak_readers: usize,
    waiting_writers: usize,
    active_writers: usize,
    stage3_mounts: usize,
    journal_acquisitions: u64,
    journal_contentions: u64,
    transaction_context_acquisitions: u64,
    transaction_context_contentions: u64,
    inode_acquisitions: u64,
    inode_contentions: u64,
    block_group_acquisitions: u64,
    block_group_contentions: u64,
    superblock_acquisitions: u64,
    superblock_contentions: u64,
    active_transactions: usize,
    peak_transactions: usize,
    active_inode_readers: usize,
    peak_inode_readers: usize,
    active_inode_writers: usize,
    peak_inode_writers: usize,
    inode_shard_samples: usize,
    active_block_groups: usize,
    peak_block_groups: usize,
}

fn aggregate_ext4_stats(stats: &crate::fs::lwext4::Lwext4LockStats) -> BuildstormExt4Stats {
    let mut result = BuildstormExt4Stats::default();
    for mount in &stats.mounts {
        result.mount_acquisitions = result.mount_acquisitions.saturating_add(mount.acquisitions);
        result.mount_contentions = result.mount_contentions.saturating_add(mount.contentions);
        result.mount_wait_ns = result.mount_wait_ns.saturating_add(mount.total_wait_ns);
        result.namespace_acquisitions = result
            .namespace_acquisitions
            .saturating_add(mount.namespace_acquisitions);
        result.namespace_contentions = result
            .namespace_contentions
            .saturating_add(mount.namespace_contentions);
        result.namespace_wait_ns = result
            .namespace_wait_ns
            .saturating_add(mount.namespace_total_wait_ns);
        result.active_readers = result
            .active_readers
            .saturating_add(mount.lock.active_readers);
        result.peak_readers = result.peak_readers.max(mount.lock.max_active_readers);
        result.waiting_writers = result
            .waiting_writers
            .saturating_add(mount.lock.waiting_writers);
        result.active_writers = result
            .active_writers
            .saturating_add(usize::from(mount.lock.writer_active));

        let Some(stage3) = mount.stage3 else {
            continue;
        };
        result.stage3_mounts = result.stage3_mounts.saturating_add(1);
        result.journal_acquisitions = result
            .journal_acquisitions
            .saturating_add(stage3.journal_acquisitions);
        result.journal_contentions = result
            .journal_contentions
            .saturating_add(stage3.journal_contentions);
        result.transaction_context_acquisitions = result
            .transaction_context_acquisitions
            .saturating_add(stage3.transaction_context_acquisitions);
        result.transaction_context_contentions = result
            .transaction_context_contentions
            .saturating_add(stage3.transaction_context_contentions);
        result.inode_acquisitions = result
            .inode_acquisitions
            .saturating_add(stage3.inode_read_acquisitions)
            .saturating_add(stage3.inode_write_acquisitions);
        result.inode_contentions = result
            .inode_contentions
            .saturating_add(stage3.inode_contentions);
        result.block_group_acquisitions = result
            .block_group_acquisitions
            .saturating_add(stage3.block_group_acquisitions);
        result.block_group_contentions = result
            .block_group_contentions
            .saturating_add(stage3.block_group_contentions);
        result.superblock_acquisitions = result
            .superblock_acquisitions
            .saturating_add(stage3.superblock_acquisitions);
        result.superblock_contentions = result
            .superblock_contentions
            .saturating_add(stage3.superblock_contentions);
        result.active_transactions = result
            .active_transactions
            .saturating_add(stage3.active_transactions as usize);
        result.peak_transactions = result
            .peak_transactions
            .max(stage3.max_active_transactions as usize);
        result.active_inode_readers = result
            .active_inode_readers
            .saturating_add(stage3.active_inode_readers as usize);
        result.peak_inode_readers = result
            .peak_inode_readers
            .max(stage3.max_active_inode_readers as usize);
        result.active_inode_writers = result
            .active_inode_writers
            .saturating_add(stage3.active_inode_writers as usize);
        result.peak_inode_writers = result
            .peak_inode_writers
            .max(stage3.max_active_inode_writers as usize);
        result.inode_shard_samples = result
            .inode_shard_samples
            .saturating_add(stage3.inode_sample_count as usize);
        result.active_block_groups = result
            .active_block_groups
            .saturating_add(stage3.active_block_groups as usize);
        result.peak_block_groups = result
            .peak_block_groups
            .max(stage3.max_active_block_groups as usize);
    }
    result
}

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
        let now_ns = polyhal::timer::current_time().as_nanos() as usize;
        let (online_mask, online_cpus, capacity_ns) =
            crate::task::manager::online_cpu_capacity_ns(now_ns);
        let user_ns = crate::task::task::global_user_runtime_ns(now_ns).min(capacity_ns);
        let idle_ns = crate::task::processor::total_idle_time_ns_at(now_ns)
            .min(capacity_ns.saturating_sub(user_ns));
        let kernel_ns = capacity_ns.saturating_sub(user_ns.saturating_add(idle_ns));
        let perf = crate::task::perf_stats::snapshot();
        let mprotect = crate::task::perf_stats::mprotect_phase_snapshot();
        let anon_fault = crate::task::perf_stats::anon_fault_phase_snapshot();
        let readahead = crate::fs::lwext4::file::ext4_readahead_stats();
        let block = crate::drivers::block::virtio_blk::virtio_block_completion_stats();
        let page_cache = crate::fs::page::pagecache::atomic_stats();
        let page_cache_lock = crate::fs::page::pagecache::PAGE_CACHE.stats();
        let ext4_lock = crate::fs::lwext4::lwext4_lock_stats();
        let ext4 = aggregate_ext4_stats(&ext4_lock);
        let mprotect_vma_ns = mprotect
            .preflight_ns_total
            .saturating_add(mprotect.vma_update_ns_total);

        let mut info = format!(
            "buildstorm_kernel_diag_version: {}\n\
             global_cpu_time: now_ns={} online_mask={:#x} online_cpus={} capacity_ns={} user_ns={} kernel_ns={} idle_ns={}\n",
            BUILDSTORM_KERNEL_DIAG_VERSION,
            now_ns,
            online_mask,
            online_cpus,
            capacity_ns,
            user_ns,
            kernel_ns,
            idle_ns,
        );
        writeln!(
            info,
            "mprotect_perf: mprotect_calls={} mprotect_total_ns={} mprotect_vm_lock_ns={} mprotect_vma_ns={} mprotect_pte_ns={} mprotect_tlb_ns={} mprotect_unaccounted_ns={} mprotect_context_switches={} mprotect_ptes_walked={} mprotect_ptes_changed={}",
            mprotect.calls,
            mprotect.elapsed_ns_total,
            mprotect.vm_lock_ns_total,
            mprotect_vma_ns,
            mprotect.pte_walk_ns_total,
            mprotect.tlb_ns_total,
            mprotect.unaccounted_ns_total,
            mprotect.context_switches_total,
            mprotect.ptes_walked,
            mprotect.ptes_changed,
        )
        .unwrap();
        writeln!(
            info,
            "fault_perf: fault_file_shared_pages={} fault_file_private_copies={} fault_file_zero_pages={} fault_anon_calls={} fault_anon_total_ns={} fault_anon_total_ns_max={} fault_anon_heap_calls={} fault_anon_stack_calls={} fault_anon_mmap_calls={} fault_anon_shared_calls={} fault_anon_elf_calls={} fault_anon_frame_alloc_ns={} fault_anon_frame_alloc_ns_max={} fault_anon_zero_ns={} fault_anon_zero_ns_max={} fault_anon_publish_ns={} fault_anon_page_table_ns={} fault_anon_page_table_ns_max={} fault_anon_icache_ns={} fault_anon_tlb_ns={} fault_anon_tlb_ns_max={}",
            perf.file_fault_shared_pages,
            perf.file_fault_private_copies,
            perf.file_fault_zero_pages,
            anon_fault.calls,
            anon_fault.total_ns,
            anon_fault.total_ns_max,
            anon_fault.heap_calls,
            anon_fault.stack_calls,
            anon_fault.mmap_calls,
            anon_fault.shared_calls,
            anon_fault.elf_calls,
            anon_fault.frame_alloc_ns,
            anon_fault.frame_alloc_ns_max,
            anon_fault.zero_ns,
            anon_fault.zero_ns_max,
            anon_fault.publish_ns,
            anon_fault.page_table_ns,
            anon_fault.page_table_ns_max,
            anon_fault.icache_ns,
            anon_fault.tlb_ns,
            anon_fault.tlb_ns_max,
        )
        .unwrap();
        writeln!(
            info,
            "readahead_perf: readahead_queued={} readahead_completed={} readahead_pages={} readahead_dropped={} readahead_retries={} readahead_active={}",
            readahead.queued,
            readahead.completed,
            readahead.pages_loaded,
            readahead.dropped,
            readahead.retries,
            readahead.active,
        )
        .unwrap();
        writeln!(
            info,
            "block_perf: block_requests={} block_completions={} block_requested_sectors={} block_completed_sectors={}",
            block.requests,
            block.completions,
            block.requested_sectors,
            block.completed_sectors,
        )
        .unwrap();
        writeln!(
            info,
            "futex_perf: futex_wait_calls={} futex_wake_calls={} futex_wake_one_calls={} futex_wait_block_calls={} futex_wait_suspend_calls={} task_block_calls={} task_block_schedules={} task_suspend_calls={} task_suspend_schedules={} task_preempt_calls={} task_preempt_schedules={}",
            perf.futex_wait_calls,
            perf.futex_wake_calls,
            perf.futex_wake_one_calls,
            perf.futex_wait_block_calls,
            perf.futex_wait_suspend_calls,
            perf.block_calls,
            perf.block_schedule_calls,
            perf.suspend_calls,
            perf.suspend_schedule_calls,
            perf.preempt_calls,
            perf.preempt_schedule_calls,
        )
        .unwrap();
        writeln!(
            info,
            "page_cache_perf: pagecache_pages={} pagecache_ext4_pages={} pagecache_inserts={} pagecache_removes={} pagecache_shards={} pagecache_inner_busy_shards={} pagecache_locked_shards={} pagecache_handoff_shards={} pagecache_waiters={} pagecache_live_waiters={}",
            page_cache.pages,
            page_cache.ext4_pages,
            page_cache.insert_count,
            page_cache.remove_count,
            page_cache_lock.shards,
            page_cache_lock.inner_busy_shards,
            page_cache_lock.locked_shards,
            page_cache_lock.handoff_shards,
            page_cache_lock.waiters,
            page_cache_lock.live_waiters,
        )
        .unwrap();
        writeln!(
            info,
            "ext4_perf: ext4_global_acquisitions={} ext4_global_contentions={} ext4_global_wait_ns={} ext4_mounts={} ext4_mount_registry_busy={} ext4_mount_acquisitions={} ext4_mount_contentions={} ext4_mount_wait_ns={} ext4_namespace_acquisitions={} ext4_namespace_contentions={} ext4_namespace_wait_ns={} ext4_active_readers={} ext4_peak_readers={} ext4_waiting_writers={} ext4_active_writers={} ext4_stage3_mounts={} ext4_journal_acquisitions={} ext4_journal_contentions={} ext4_transaction_context_acquisitions={} ext4_transaction_context_contentions={} ext4_inode_acquisitions={} ext4_inode_contentions={} ext4_inode_shard_samples={} ext4_active_inode_readers={} ext4_peak_inode_readers={} ext4_active_inode_writers={} ext4_peak_inode_writers={} ext4_block_group_acquisitions={} ext4_block_group_contentions={} ext4_active_block_groups={} ext4_peak_block_groups={} ext4_superblock_acquisitions={} ext4_superblock_contentions={} ext4_active_transactions={} ext4_peak_transactions={}",
            ext4_lock.acquisitions,
            ext4_lock.contentions,
            ext4_lock.total_wait_ns,
            ext4_lock.mounts.len(),
            usize::from(ext4_lock.mount_registry_busy),
            ext4.mount_acquisitions,
            ext4.mount_contentions,
            ext4.mount_wait_ns,
            ext4.namespace_acquisitions,
            ext4.namespace_contentions,
            ext4.namespace_wait_ns,
            ext4.active_readers,
            ext4.peak_readers,
            ext4.waiting_writers,
            ext4.active_writers,
            ext4.stage3_mounts,
            ext4.journal_acquisitions,
            ext4.journal_contentions,
            ext4.transaction_context_acquisitions,
            ext4.transaction_context_contentions,
            ext4.inode_acquisitions,
            ext4.inode_contentions,
            ext4.inode_shard_samples,
            ext4.active_inode_readers,
            ext4.peak_inode_readers,
            ext4.active_inode_writers,
            ext4.peak_inode_writers,
            ext4.block_group_acquisitions,
            ext4.block_group_contentions,
            ext4.active_block_groups,
            ext4.peak_block_groups,
            ext4.superblock_acquisitions,
            ext4.superblock_contentions,
            ext4.active_transactions,
            ext4.peak_transactions,
        )
        .unwrap();

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
