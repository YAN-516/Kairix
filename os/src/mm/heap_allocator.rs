//! The global allocator
// use crate::config::KERNEL_HEAP_SIZE;
use polyhal::consts::{KERNEL_HEAP_SIZE, PAGE_SIZE};

use buddy_system_allocator::LockedHeap;
use core::alloc::{GlobalAlloc, Layout};
use core::ptr::addr_of_mut;
use core::sync::atomic::{AtomicBool, Ordering};
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

struct KernelHeapAllocator {
    inner: LockedHeap<32>,
}

unsafe impl GlobalAlloc for KernelHeapAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { GlobalAlloc::alloc(&self.inner, layout) };
        if ptr.is_null() {
            print_heap_alloc_error_snapshot_once(layout);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe {
            GlobalAlloc::dealloc(&self.inner, ptr, layout);
        }
    }
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
            "[OOM] page_cache: pages={} dirty={} disk_pages={} disk_dirty={} disk_limit={} tmpfs={} tmpfs_swapped={} fat32={} ext4={} unknown={}",
            stats.pages,
            stats.dirty_pages,
            stats.disk_pages,
            stats.dirty_disk_pages,
            stats.max_disk_pages,
            stats.tmpfs_pages,
            stats.swapped_tmpfs_pages,
            stats.fat32_pages,
            stats.ext4_pages,
            stats.unknown_pages
        );
    } else {
        println!("[OOM] page_cache: lock_busy=true");
    }

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

    let dcache = crate::fs::vfs::dcache::GLOBAL_DCACHE.try_stats();
    println!(
        "[OOM] dcache: entries={} pinned={} lru_entries={} max_size={} lock_busy={}",
        dcache.entries, dcache.pinned, dcache.lru_entries, dcache.max_size, dcache.lock_busy
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
