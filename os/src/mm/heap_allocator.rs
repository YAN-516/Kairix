//! The global allocator
// use crate::config::KERNEL_HEAP_SIZE;
use polyhal::consts::{KERNEL_HEAP_SIZE, PAGE_SIZE};

use buddy_system_allocator::LockedHeap;
use core::alloc::{GlobalAlloc, Layout};
use core::ptr::addr_of_mut;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use log::*;
use log::*;
use polyhal::{print, println};

/// Snapshot of the kernel heap allocator state.
#[derive(Debug, Clone, Copy)]
pub struct HeapStats {
    /// Bytes requested by users of the allocator.
    pub user: usize,
    /// Bytes actually consumed after allocator rounding.
    pub actual: usize,
    /// Total bytes owned by the kernel heap.
    pub total: usize,
    /// Bytes not currently allocated from the kernel heap.
    pub free: usize,
}

/// Return the current kernel heap allocator statistics.
pub fn heap_stats() -> HeapStats {
    let heap = HEAP_ALLOCATOR.inner.lock();
    let user = heap.stats_alloc_user();
    let actual = heap.stats_alloc_actual();
    let total = heap.stats_total_bytes();
    HeapStats {
        user,
        actual,
        total,
        free: total.saturating_sub(actual),
    }
}

/// 打印当前内核堆的使用统计信息（user / actual / total）
pub fn print_heap_stats() {
    let stats = heap_stats();
    debug!(
        "[MEMDEBUG] heap: user={} actual={} total={} free={}",
        stats.user, stats.actual, stats.total, stats.free
    );
}

/// heap allocator instance
#[global_allocator]
static HEAP_ALLOCATOR: KernelHeapAllocator = KernelHeapAllocator {
    inner: LockedHeap::empty(),
};

static OOM_SNAPSHOT_PRINTED: AtomicBool = AtomicBool::new(false);
const HEAP_ALLOC_BUCKETS: usize = 20;
const HEAP_FIRST_BUCKET_MAX: usize = 16;

static HEAP_BUCKET_CURRENT_BYTES: [AtomicUsize; HEAP_ALLOC_BUCKETS] =
    [const { AtomicUsize::new(0) }; HEAP_ALLOC_BUCKETS];
static HEAP_BUCKET_CURRENT_ROUNDED_BYTES: [AtomicUsize; HEAP_ALLOC_BUCKETS] =
    [const { AtomicUsize::new(0) }; HEAP_ALLOC_BUCKETS];
static HEAP_BUCKET_CURRENT_ALLOCS: [AtomicUsize; HEAP_ALLOC_BUCKETS] =
    [const { AtomicUsize::new(0) }; HEAP_ALLOC_BUCKETS];
static HEAP_BUCKET_ALLOC_COUNT: [AtomicUsize; HEAP_ALLOC_BUCKETS] =
    [const { AtomicUsize::new(0) }; HEAP_ALLOC_BUCKETS];
static HEAP_BUCKET_FREE_COUNT: [AtomicUsize; HEAP_ALLOC_BUCKETS] =
    [const { AtomicUsize::new(0) }; HEAP_ALLOC_BUCKETS];

struct KernelHeapAllocator {
    inner: LockedHeap<32>,
}

unsafe impl GlobalAlloc for KernelHeapAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { GlobalAlloc::alloc(&self.inner, layout) };
        if ptr.is_null() {
            print_heap_alloc_error_snapshot_once(layout);
        } else {
            record_heap_alloc(layout);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        record_heap_dealloc(layout);
        unsafe {
            GlobalAlloc::dealloc(&self.inner, ptr, layout);
        }
    }
}

fn heap_bucket_index(size: usize) -> usize {
    let mut max_size = HEAP_FIRST_BUCKET_MAX;
    let size = size.max(1);
    for bucket in 0..HEAP_ALLOC_BUCKETS - 1 {
        if size <= max_size {
            return bucket;
        }
        max_size <<= 1;
    }
    HEAP_ALLOC_BUCKETS - 1
}

fn heap_bucket_min(bucket: usize) -> usize {
    if bucket == 0 {
        1
    } else {
        (HEAP_FIRST_BUCKET_MAX << (bucket - 1)) + 1
    }
}

fn heap_bucket_max(bucket: usize) -> usize {
    if bucket + 1 == HEAP_ALLOC_BUCKETS {
        usize::MAX
    } else {
        HEAP_FIRST_BUCKET_MAX << bucket
    }
}

fn record_heap_alloc(layout: Layout) {
    let size = layout.size().max(1);
    let rounded = rounded_request_bytes(layout).unwrap_or(size);
    let bucket = heap_bucket_index(size);
    HEAP_BUCKET_CURRENT_BYTES[bucket].fetch_add(size, Ordering::Relaxed);
    HEAP_BUCKET_CURRENT_ROUNDED_BYTES[bucket].fetch_add(rounded, Ordering::Relaxed);
    HEAP_BUCKET_CURRENT_ALLOCS[bucket].fetch_add(1, Ordering::Relaxed);
    HEAP_BUCKET_ALLOC_COUNT[bucket].fetch_add(1, Ordering::Relaxed);
}

fn record_heap_dealloc(layout: Layout) {
    let size = layout.size().max(1);
    let rounded = rounded_request_bytes(layout).unwrap_or(size);
    let bucket = heap_bucket_index(size);
    HEAP_BUCKET_CURRENT_BYTES[bucket].fetch_sub(size, Ordering::Relaxed);
    HEAP_BUCKET_CURRENT_ROUNDED_BYTES[bucket].fetch_sub(rounded, Ordering::Relaxed);
    HEAP_BUCKET_CURRENT_ALLOCS[bucket].fetch_sub(1, Ordering::Relaxed);
    HEAP_BUCKET_FREE_COUNT[bucket].fetch_add(1, Ordering::Relaxed);
}

fn rounded_request_bytes(layout: Layout) -> Option<usize> {
    layout
        .size()
        .max(layout.align())
        .max(1)
        .checked_next_power_of_two()
}

fn heap_alloc_failure_hint(
    layout: Layout,
    heap: HeapStats,
    rounded: Option<usize>,
) -> &'static str {
    if layout.size() > heap.total {
        "single allocation is larger than the whole kernel heap"
    } else if rounded.is_some_and(|request| request > heap.total) {
        "allocation order/alignment is larger than the whole kernel heap"
    } else if rounded.is_some_and(|request| request > heap.free) || layout.size() > heap.free {
        "kernel heap is exhausted; check unreleased heap objects or large live buffers"
    } else {
        "kernel heap has enough aggregate free bytes; suspect buddy fragmentation or alignment/order pressure"
    }
}

fn print_heap_bucket_snapshot() {
    for bucket in 0..HEAP_ALLOC_BUCKETS {
        let current_bytes = HEAP_BUCKET_CURRENT_BYTES[bucket].load(Ordering::Relaxed);
        let current_allocs = HEAP_BUCKET_CURRENT_ALLOCS[bucket].load(Ordering::Relaxed);
        if current_bytes == 0 && current_allocs == 0 {
            continue;
        }
        let min = heap_bucket_min(bucket);
        let max = heap_bucket_max(bucket);
        let rounded_bytes = HEAP_BUCKET_CURRENT_ROUNDED_BYTES[bucket].load(Ordering::Relaxed);
        if max == usize::MAX {
            println!(
                "[OOM] heap_bucket: size=[{},inf) current_bytes={} rounded_bytes={} current_allocs={} alloc_count={} free_count={}",
                min,
                current_bytes,
                rounded_bytes,
                current_allocs,
                HEAP_BUCKET_ALLOC_COUNT[bucket].load(Ordering::Relaxed),
                HEAP_BUCKET_FREE_COUNT[bucket].load(Ordering::Relaxed)
            );
        } else {
            println!(
                "[OOM] heap_bucket: size=[{},{}] current_bytes={} rounded_bytes={} current_allocs={} alloc_count={} free_count={}",
                min,
                max,
                current_bytes,
                rounded_bytes,
                current_allocs,
                HEAP_BUCKET_ALLOC_COUNT[bucket].load(Ordering::Relaxed),
                HEAP_BUCKET_FREE_COUNT[bucket].load(Ordering::Relaxed)
            );
        }
    }
}

fn print_heap_alloc_error_snapshot(layout: Layout) {
    let heap = heap_stats();
    let rounded = rounded_request_bytes(layout);
    println!(
        "[OOM] kernel_heap_alloc failed: request_size={} align={} rounded_order_bytes={} heap_total={} heap_free={} page_size={}",
        layout.size(),
        layout.align(),
        rounded.unwrap_or(0),
        heap.total,
        heap.free,
        PAGE_SIZE
    );
    println!(
        "[OOM] heap: user={} actual={} free={} total={} hint={}",
        heap.user,
        heap.actual,
        heap.free,
        heap.total,
        heap_alloc_failure_hint(layout, heap, rounded)
    );
    print_heap_bucket_snapshot();

    if let Some(frame) = crate::mm::try_frame_stats() {
        println!(
            "[OOM] frames: used_pages={} free_pages={} fresh_free_pages={} recycled_pages={} total_pages={} free_bytes={} total_bytes={} alloc_count={} free_count={} delta={}",
            frame.used_pages,
            frame.free_pages,
            frame.fresh_free_pages,
            frame.recycled_pages,
            frame.total_pages,
            frame.free_pages * PAGE_SIZE,
            frame.total_pages * PAGE_SIZE,
            frame.alloc_count,
            frame.free_count,
            frame.allocated_delta
        );
    } else {
        println!("[OOM] frames: allocator_lock_busy=true");
    }

    if let Some(cache) = crate::fs::page::pagecache::PAGE_CACHE.try_lock() {
        let stats = cache.stats();
        println!(
            "[OOM] page_cache: pages={} dirty={} disk_pages={} disk_dirty={} disk_limit={} tmpfs={} tmpfs_swapped={} fat32={} ext4={} unknown={} lru_order={} lru_gen={} next_gen={}",
            stats.pages,
            stats.dirty_pages,
            stats.disk_pages,
            stats.dirty_disk_pages,
            stats.max_disk_pages,
            stats.tmpfs_pages,
            stats.swapped_tmpfs_pages,
            stats.fat32_pages,
            stats.ext4_pages,
            stats.unknown_pages,
            stats.lru_order_entries,
            stats.lru_gen_entries,
            stats.next_gen
        );
    } else {
        println!("[OOM] page_cache: lock_busy=true");
    }
    let page_cache_atomic = crate::fs::page::pagecache::atomic_stats();
    println!(
        "[OOM] page_cache_atomic: pages={} tmpfs={} fat32={} ext4={} unknown={} insert_count={} remove_count={}",
        page_cache_atomic.pages,
        page_cache_atomic.tmpfs_pages,
        page_cache_atomic.fat32_pages,
        page_cache_atomic.ext4_pages,
        page_cache_atomic.unknown_pages,
        page_cache_atomic.insert_count,
        page_cache_atomic.remove_count
    );
    let tmpfs_inode = crate::fs::tmpfs::inode::tmpfs_inode_stats();
    println!(
        "[OOM] tmpfs_inode: created={} dropped={} current={} file={} dir={} link={} special={} xattrs={} xattr_bytes={} xattr_set_count={} xattr_remove_count={} symlink_bytes={}",
        tmpfs_inode.created,
        tmpfs_inode.dropped,
        tmpfs_inode.current,
        tmpfs_inode.file_inodes,
        tmpfs_inode.dir_inodes,
        tmpfs_inode.link_inodes,
        tmpfs_inode.special_inodes,
        tmpfs_inode.xattrs,
        tmpfs_inode.xattr_bytes,
        tmpfs_inode.xattr_set_count,
        tmpfs_inode.xattr_remove_count,
        tmpfs_inode.symlink_bytes
    );

    let proc_mem = crate::task::manager::process_memory_retention_stats();
    println!(
        "[OOM] process_mem: processes={} lock_busy={} locked_processes={} zombie_processes={} user_areas={} user_data_frames={} elf={} heap={} stack={} mmap={} shm={} other={} max_data_frames={} max_data_frames_pid={} max_data_frames_zombie={}",
        proc_mem.processes,
        proc_mem.lock_busy,
        proc_mem.locked_processes,
        proc_mem.zombie_processes,
        proc_mem.user_areas,
        proc_mem.user_data_frames,
        proc_mem.elf_frames,
        proc_mem.heap_frames,
        proc_mem.stack_frames,
        proc_mem.mmap_frames,
        proc_mem.shm_frames,
        proc_mem.other_frames,
        proc_mem.max_data_frames,
        proc_mem.max_data_frames_pid,
        proc_mem.max_data_frames_zombie
    );
    println!(
        "[OOM] process_refs: fd_slots={} open_files={} child_refs={} max_open_files={} max_open_files_pid={} max_fd_slots={} max_fd_slots_pid={} max_process_strong_count={} max_process_strong_count_pid={}",
        proc_mem.fd_slots,
        proc_mem.open_files,
        proc_mem.child_refs,
        proc_mem.max_open_files,
        proc_mem.max_open_files_pid,
        proc_mem.max_fd_slots,
        proc_mem.max_fd_slots_pid,
        proc_mem.max_process_strong_count,
        proc_mem.max_process_strong_count_pid
    );
    let task_retention = crate::task::task_retention_stats();
    let processor_stats = crate::task::processor::processor_task_stats();
    println!(
        "[OOM] task_retention: process_table_lock_busy={} processes={} locked_processes={} zombie_processes={} child_refs={} max_child_refs={} max_child_refs_pid={} task_slots={} zombie_task_slots={} max_task_slots={} max_task_slots_pid={} max_task_strong_count={} max_task_strong_count_pid={} max_task_strong_count_tid={} ready_queue_tasks={} current_tasks={} locked_processors={} timer_queue_tasks={} timer_queue_lock_busy={}",
        task_retention.process_table_lock_busy,
        task_retention.processes,
        task_retention.locked_processes,
        task_retention.zombie_processes,
        task_retention.child_refs,
        task_retention.max_child_refs,
        task_retention.max_child_refs_pid,
        task_retention.task_slots,
        task_retention.zombie_task_slots,
        task_retention.max_task_slots,
        task_retention.max_task_slots_pid,
        task_retention.max_task_strong_count,
        task_retention.max_task_strong_count_pid,
        task_retention.max_task_strong_count_tid,
        task_retention.ready_queue_tasks,
        processor_stats.current_tasks,
        processor_stats.locked_processors,
        task_retention.timer_queue_tasks,
        task_retention.timer_queue_lock_busy
    );
    let task_lifecycle = crate::task::task::task_lifecycle_stats();
    println!(
        "[OOM] task_lifecycle: created={} dropped={} live_delta={} deferred_exited_tasks={}",
        task_lifecycle.created,
        task_lifecycle.dropped,
        task_lifecycle.live_delta,
        crate::task::deferred_exited_task_count()
    );
    let id_stats = crate::task::task_id_stats();
    println!(
        "[OOM] task_ids: kstack_current={} kstack_live={} kstack_recycled={} kstack_handle_alloc={} kstack_handle_drop={} kstack_handle_delta={} pid_current={} pid_live={} pid_recycled={} pid_handle_alloc={} pid_handle_drop={} pid_handle_delta={} raw_pid_alloc={} raw_pid_dealloc={} raw_pid_delta={}",
        id_stats.kstack_current,
        id_stats.kstack_live,
        id_stats.kstack_recycled,
        id_stats.kstack_alloc_handles,
        id_stats.kstack_drop_handles,
        id_stats
            .kstack_alloc_handles
            .saturating_sub(id_stats.kstack_drop_handles),
        id_stats.pid_current,
        id_stats.pid_live,
        id_stats.pid_recycled,
        id_stats.pid_handle_alloc,
        id_stats.pid_handle_drop,
        id_stats
            .pid_handle_alloc
            .saturating_sub(id_stats.pid_handle_drop),
        id_stats.raw_pid_alloc,
        id_stats.raw_pid_dealloc,
        id_stats
            .raw_pid_alloc
            .saturating_sub(id_stats.raw_pid_dealloc)
    );
    let process_registry = crate::task::process::process_registry_stats();
    println!(
        "[OOM] process_registry: created={} dropped={} live_delta={} registry_entries={} registry_live={} registry_dead={} hidden_processes={} hidden_zombies={} hidden_task_slots={} hidden_open_files={} hidden_child_refs={} hidden_locked={} max_hidden_strong_count={} max_hidden_strong_count_pid={} lock_busy={} pid_table_lock_busy={}",
        process_registry.created,
        process_registry.dropped,
        process_registry.live_delta,
        process_registry.registry_entries,
        process_registry.registry_live,
        process_registry.registry_dead,
        process_registry.hidden_processes,
        process_registry.hidden_zombies,
        process_registry.hidden_task_slots,
        process_registry.hidden_open_files,
        process_registry.hidden_child_refs,
        process_registry.hidden_locked,
        process_registry.max_hidden_strong_count,
        process_registry.max_hidden_strong_count_pid,
        process_registry.lock_busy,
        process_registry.pid_table_lock_busy
    );
    let tid_stats = crate::task::manager::tid2task_stats();
    println!(
        "[OOM] tid2task: entries={} live={} dead={} lock_busy={}",
        tid_stats.entries, tid_stats.live, tid_stats.dead, tid_stats.lock_busy
    );
    let futex_stats = crate::syscall::futex::stats();
    println!(
        "[OOM] futex: queues={} waiters={} lock_busy={}",
        futex_stats.queues, futex_stats.waiters, futex_stats.lock_busy
    );
    let pipe_stats = crate::fs::pipe::pipe_stats();
    println!(
        "[OOM] pipe: buffers_current={} buffers_created={} buffers_dropped={} pages_current={} pages_peak={} pages_allocated={} pages_dropped={} bytes_current={}",
        pipe_stats.buffers_current,
        pipe_stats.buffers_created,
        pipe_stats.buffers_dropped,
        pipe_stats.pages_current,
        pipe_stats.pages_peak,
        pipe_stats.pages_allocated,
        pipe_stats.pages_dropped,
        pipe_stats.bytes_current
    );

    let dcache = crate::fs::vfs::dcache::GLOBAL_DCACHE.try_stats();
    println!(
        "[OOM] dcache: entries={} pinned={} lru_entries={} max_size={} path_bytes={} lru_path_bytes={} pinned_path_bytes={} tmp_entries={} tmp_path_bytes={} ltp_tmp_entries={} ltp_tmp_path_bytes={} max_path_len={} lock_busy={}",
        dcache.entries,
        dcache.pinned,
        dcache.lru_entries,
        dcache.max_size,
        dcache.path_bytes,
        dcache.lru_path_bytes,
        dcache.pinned_path_bytes,
        dcache.tmp_entries,
        dcache.tmp_path_bytes,
        dcache.ltp_tmp_entries,
        dcache.ltp_tmp_path_bytes,
        dcache.max_path_len,
        dcache.lock_busy
    );

    let new_mount = crate::syscall::try_new_mount_stats();
    println!(
        "[OOM] new_mount: fs_contexts={} fs_context_pids={} max_contexts_per_pid={} max_contexts_pid={} mount_attrs={} lock_busy={}",
        new_mount.fs_contexts,
        new_mount.fs_context_pids,
        new_mount.max_contexts_per_pid,
        new_mount.max_contexts_pid,
        new_mount.mount_attrs,
        new_mount.lock_busy
    );

    let fs = crate::fs::try_fs_retention_stats();
    println!(
        "[OOM] fs_retention: filesystems={} superblocks={} locked_super_tables={} lock_busy={}",
        fs.filesystems, fs.superblocks, fs.locked_super_tables, fs.lock_busy
    );
    println!(
        "[OOM] inode_holes: punched_hole_pages={}",
        crate::fs::vfs::inode::punched_hole_page_count()
    );

    let lwext4_alloc = lwext4_rust::allocation_stats();
    println!(
        "[OOM] lwext4_alloc: current_user={} current_actual={} peak_user={} peak_actual={} alloc_count={} free_count={} delta={}",
        lwext4_alloc.current_user,
        lwext4_alloc.current_actual,
        lwext4_alloc.peak_user,
        lwext4_alloc.peak_actual,
        lwext4_alloc.alloc_count,
        lwext4_alloc.free_count,
        lwext4_alloc
            .alloc_count
            .saturating_sub(lwext4_alloc.free_count)
    );

    if let Some(pending) = crate::fs::writeback::try_pending_count() {
        println!("[OOM] writeback: pending_files={}", pending);
    } else {
        println!("[OOM] writeback: queue_lock_busy=true");
    }

    if let Some(swap) = crate::mm::swap::try_stats() {
        println!(
            "[OOM] swap: enabled={} used_slots={} free_slots={} total_slots={} alloc_count={} free_count={}",
            swap.enabled,
            swap.used_slots,
            swap.free_slots,
            swap.total_slots,
            swap.alloc_count,
            swap.free_count
        );
    } else {
        println!("[OOM] swap: lock_busy=true");
    }
}

fn print_heap_alloc_error_snapshot_once(layout: Layout) {
    if !OOM_SNAPSHOT_PRINTED.swap(true, Ordering::Relaxed) {
        print_heap_alloc_error_snapshot(layout);
    }
}

#[alloc_error_handler]
/// panic when heap allocation error occurs
pub fn handle_alloc_error(layout: Layout) -> ! {
    print_heap_alloc_error_snapshot_once(layout);
    panic!("Heap allocation error, layout = {:?}", layout);
}
/// heap space ([u8; KERNEL_HEAP_SIZE])
static mut HEAP_SPACE: [u8; KERNEL_HEAP_SIZE] = [0; KERNEL_HEAP_SIZE];
/// initiate heap allocator
pub fn init_heap() {
    unsafe {
        HEAP_ALLOCATOR
            .inner
            .lock()
            .init(addr_of_mut!(HEAP_SPACE) as usize, KERNEL_HEAP_SIZE);
    }
}

#[allow(unused)]
#[allow(missing_docs)]
pub fn heap_test() {
    use alloc::boxed::Box;
    use alloc::vec::Vec;
    unsafe extern "C" {
        safe fn sbss();
        safe fn ebss();
    }
    let bss_range = sbss as usize..ebss as usize;
    let a = Box::new(5);
    assert_eq!(*a, 5);
    assert!(bss_range.contains(&(a.as_ref() as *const _ as usize)));
    drop(a);
    let mut v: Vec<usize> = Vec::new();
    for i in 0..500 {
        v.push(i);
    }
    for (i, val) in v.iter().take(500).enumerate() {
        assert_eq!(*val, i);
    }
    assert!(bss_range.contains(&(v.as_ptr() as usize)));
    drop(v);
    println!("heap_test passed!");
}
