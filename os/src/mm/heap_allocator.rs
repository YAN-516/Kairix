//! The global allocator
use crate::sync::{IrqGuard, SpinMutexGuard, SpinNoIrq, SpinNoIrqLock};
use polyhal::consts::{PAGE_SIZE, VIRT_ADDR_START};

use buddy_system_allocator::Heap;
use core::alloc::{GlobalAlloc, Layout};
use core::mem::size_of;
use core::ops::{Deref, DerefMut};
use core::ptr::{NonNull, addr_of_mut};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use log::*;
use log::*;
use polyhal::print;

const KERNEL_HEAP_ORDER: usize = 32;
const KERNEL_HEAP_BOOTSTRAP_SIZE: usize = 2 * 1024 * 1024;
const KERNEL_HEAP_GROW_CHUNK_SIZE: usize = 2 * 1024 * 1024;
const KERNEL_HEAP_MIN_FRAME_RESERVE: usize = 16 * 1024 * 1024;
const KERNEL_HEAP_MAX_PHYS_FRACTION: usize = 4;

const HEAP_GROW_FAILURE_NONE: usize = 0;
const HEAP_GROW_FAILURE_DISABLED: usize = 1;
const HEAP_GROW_FAILURE_LAYOUT: usize = 2;
const HEAP_GROW_FAILURE_LIMIT: usize = 3;
const HEAP_GROW_FAILURE_FRAMES: usize = 4;

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
    /// Bytes supplied by dynamic physical-frame extents.
    pub grown: usize,
    /// Number of extents added after bootstrap.
    pub growth_count: usize,
    /// Number of failed attempts to grow the heap.
    pub growth_failures: usize,
    /// Current dynamic heap growth limit in bytes.
    pub growth_limit: usize,
}

/// Return the current kernel heap allocator statistics.
pub fn heap_stats() -> HeapStats {
    let heap = HEAP_ALLOCATOR.lock(HEAP_OP_STATS, 0, 0, 0);
    let user = heap.stats_alloc_user();
    let actual = heap.stats_alloc_actual();
    let total = heap.stats_total_bytes();
    HeapStats {
        user,
        actual,
        total,
        free: total.saturating_sub(actual),
        grown: HEAP_GROWN_BYTES.load(Ordering::Relaxed),
        growth_count: HEAP_GROW_COUNT.load(Ordering::Relaxed),
        growth_failures: HEAP_GROW_FAILURES.load(Ordering::Relaxed),
        growth_limit: HEAP_GROWTH_LIMIT.load(Ordering::Relaxed),
    }
}

/// 打印当前内核堆的使用统计信息（user / actual / total）
pub fn print_heap_stats() {
    let stats = heap_stats();
    debug!(
        "[MEMDEBUG] heap: user={} actual={} total={} free={}",
        stats.user, stats.actual, stats.total, stats.free
    );
    debug!(
        "[heap-grow] enabled={} bootstrap={} grown={} extents={} failures={} limit={} last_failure={}",
        HEAP_GROWTH_ENABLED.load(Ordering::Relaxed),
        KERNEL_HEAP_BOOTSTRAP_SIZE,
        stats.grown,
        stats.growth_count,
        stats.growth_failures,
        stats.growth_limit,
        heap_grow_failure_name(HEAP_GROW_LAST_FAILURE.load(Ordering::Relaxed))
    );
}

/// heap allocator instance
#[global_allocator]
static HEAP_ALLOCATOR: KernelHeapAllocator = KernelHeapAllocator {
    inner: SpinNoIrqLock::new(Heap::empty()),
};

static OOM_SNAPSHOT_PRINTED: AtomicBool = AtomicBool::new(false);
static HEAP_GROWTH_ENABLED: AtomicBool = AtomicBool::new(false);
static HEAP_GROWN_BYTES: AtomicUsize = AtomicUsize::new(0);
// Bytes already committed to the heap or reserved by an in-progress grow.
// This keeps concurrent lock-free grow attempts within HEAP_GROWTH_LIMIT.
static HEAP_GROW_ACCOUNTED_BYTES: AtomicUsize = AtomicUsize::new(0);
static HEAP_GROW_COUNT: AtomicUsize = AtomicUsize::new(0);
static HEAP_GROW_FAILURES: AtomicUsize = AtomicUsize::new(0);
static HEAP_GROW_LAST_FAILURE: AtomicUsize = AtomicUsize::new(HEAP_GROW_FAILURE_NONE);
static HEAP_GROWTH_LIMIT: AtomicUsize = AtomicUsize::new(0);
const HEAP_EXTENT_RECORD_CAPACITY: usize = 1024;
static HEAP_EXTENT_RECORD_COUNT: AtomicUsize = AtomicUsize::new(0);
static HEAP_EXTENT_RECORD_OVERFLOWS: AtomicUsize = AtomicUsize::new(0);
static HEAP_EXTENT_STARTS: [AtomicUsize; HEAP_EXTENT_RECORD_CAPACITY] =
    [const { AtomicUsize::new(0) }; HEAP_EXTENT_RECORD_CAPACITY];
static HEAP_EXTENT_ENDS: [AtomicUsize; HEAP_EXTENT_RECORD_CAPACITY] =
    [const { AtomicUsize::new(0) }; HEAP_EXTENT_RECORD_CAPACITY];
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
    inner: SpinNoIrqLock<Heap<KERNEL_HEAP_ORDER>>,
}

/// Allocation-free classification of a pointer against every heap extent.
#[derive(Clone, Copy)]
pub(crate) struct HeapPointerInfo {
    bootstrap: Option<(usize, usize)>,
    dynamic_extent: Option<(usize, usize, usize)>,
    nearest_lower_end: Option<usize>,
    nearest_upper_start: Option<usize>,
    recorded_extents: usize,
    record_overflows: usize,
}

impl core::fmt::Debug for HeapPointerInfo {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("HeapPointerInfo")
            .field("bootstrap", &self.bootstrap)
            .field("dynamic_extent", &self.dynamic_extent)
            .field("nearest_lower_end", &self.nearest_lower_end)
            .field("nearest_upper_start", &self.nearest_upper_start)
            .field("recorded_extents", &self.recorded_extents)
            .field("record_overflows", &self.record_overflows)
            .finish()
    }
}

pub(crate) fn heap_pointer_info(pointer: usize) -> HeapPointerInfo {
    let bootstrap_start = addr_of_mut!(HEAP_SPACE) as usize;
    let bootstrap_end = bootstrap_start + KERNEL_HEAP_BOOTSTRAP_SIZE;
    let bootstrap = (bootstrap_start <= pointer && pointer < bootstrap_end)
        .then_some((bootstrap_start, bootstrap_end));
    let recorded = HEAP_EXTENT_RECORD_COUNT
        .load(Ordering::Acquire)
        .min(HEAP_EXTENT_RECORD_CAPACITY);
    let mut dynamic_extent = None;
    let mut nearest_lower_end = None;
    let mut nearest_upper_start = None;
    for index in 0..recorded {
        let start = HEAP_EXTENT_STARTS[index].load(Ordering::Acquire);
        let end = HEAP_EXTENT_ENDS[index].load(Ordering::Relaxed);
        if start == 0 || end <= start {
            continue;
        }
        if start <= pointer && pointer < end {
            dynamic_extent = Some((index, start, end));
        }
        if end <= pointer && nearest_lower_end.is_none_or(|current| end > current) {
            nearest_lower_end = Some(end);
        }
        if start > pointer && nearest_upper_start.is_none_or(|current| start < current) {
            nearest_upper_start = Some(start);
        }
    }
    HeapPointerInfo {
        bootstrap,
        dynamic_extent,
        nearest_lower_end,
        nearest_upper_start,
        recorded_extents: HEAP_EXTENT_RECORD_COUNT.load(Ordering::Relaxed),
        record_overflows: HEAP_EXTENT_RECORD_OVERFLOWS.load(Ordering::Relaxed),
    }
}

fn heap_range_is_owned(pointer: usize, size: usize) -> bool {
    let Some(required_end) = pointer.checked_add(size.max(1)) else {
        return false;
    };

    // The buddy allocator may coalesce free buddies supplied by separate
    // add_to_heap calls.  A valid allocation can therefore span multiple
    // adjacent dynamic extents; requiring it to fit in one extent produces a
    // false corruption report for large allocations.  Walk the union of all
    // published extents while still rejecting any range that crosses a hole.
    let bootstrap_start = addr_of_mut!(HEAP_SPACE) as usize;
    let bootstrap_end = bootstrap_start + KERNEL_HEAP_BOOTSTRAP_SIZE;
    let recorded = HEAP_EXTENT_RECORD_COUNT
        .load(Ordering::Acquire)
        .min(HEAP_EXTENT_RECORD_CAPACITY);
    let mut covered_end = pointer;

    loop {
        let mut next_end = covered_end;
        if bootstrap_start <= covered_end && covered_end < bootstrap_end {
            next_end = bootstrap_end;
        }
        for index in 0..recorded {
            let start = HEAP_EXTENT_STARTS[index].load(Ordering::Acquire);
            let end = HEAP_EXTENT_ENDS[index].load(Ordering::Relaxed);
            if start != 0 && start <= covered_end && covered_end < end {
                next_end = next_end.max(end);
            }
        }

        if next_end >= required_end {
            return true;
        }
        if next_end == covered_end {
            return false;
        }
        covered_end = next_end;
    }
}

fn record_heap_extent(start: usize, end: usize) {
    let index = HEAP_EXTENT_RECORD_COUNT.fetch_add(1, Ordering::AcqRel);
    if index >= HEAP_EXTENT_RECORD_CAPACITY {
        HEAP_EXTENT_RECORD_OVERFLOWS.fetch_add(1, Ordering::Relaxed);
        return;
    }
    HEAP_EXTENT_ENDS[index].store(end, Ordering::Relaxed);
    HEAP_EXTENT_STARTS[index].store(start, Ordering::Release);
}

const HEAP_OP_NONE: usize = 0;
const HEAP_OP_ALLOC: usize = 1;
const HEAP_OP_DEALLOC: usize = 2;
const HEAP_OP_GROW: usize = 3;
const HEAP_OP_STATS: usize = 4;
const HEAP_OP_INIT: usize = 5;
const HEAP_LOCK_TIMEOUT_SECS: u64 = 2;

static HEAP_OWNER_OP: AtomicUsize = AtomicUsize::new(HEAP_OP_NONE);
static HEAP_OWNER_PTR: AtomicUsize = AtomicUsize::new(0);
static HEAP_OWNER_SIZE: AtomicUsize = AtomicUsize::new(0);
static HEAP_OWNER_ALIGN: AtomicUsize = AtomicUsize::new(0);

struct KernelHeapGuard<'a> {
    guard: SpinMutexGuard<'a, Heap<KERNEL_HEAP_ORDER>, SpinNoIrq>,
    _irq_guard: IrqGuard,
}

impl Deref for KernelHeapGuard<'_> {
    type Target = Heap<KERNEL_HEAP_ORDER>;

    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl DerefMut for KernelHeapGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

impl Drop for KernelHeapGuard<'_> {
    fn drop(&mut self) {
        HEAP_OWNER_PTR.store(0, Ordering::Relaxed);
        HEAP_OWNER_SIZE.store(0, Ordering::Relaxed);
        HEAP_OWNER_ALIGN.store(0, Ordering::Relaxed);
        HEAP_OWNER_OP.store(HEAP_OP_NONE, Ordering::Release);
    }
}

impl KernelHeapAllocator {
    fn lock(&self, operation: usize, ptr: usize, size: usize, align: usize) -> KernelHeapGuard<'_> {
        // The buddy allocator may legitimately scan a long free list while
        // coalescing. Retry counts run at different rates on different harts
        // and caused false deadlock panics, so this lock uses a wall-clock
        // bound while preserving the generic 0x1000000 detector elsewhere.
        let irq_guard = IrqGuard::new();
        let start = polyhal::timer::get_ticks();
        let timeout_ticks = polyhal::timer::get_freq().saturating_mul(HEAP_LOCK_TIMEOUT_SECS);
        loop {
            if let Some(guard) = self.inner.try_lock() {
                HEAP_OWNER_PTR.store(ptr, Ordering::Relaxed);
                HEAP_OWNER_SIZE.store(size, Ordering::Relaxed);
                HEAP_OWNER_ALIGN.store(align, Ordering::Relaxed);
                HEAP_OWNER_OP.store(operation, Ordering::Release);
                return KernelHeapGuard {
                    guard,
                    _irq_guard: irq_guard,
                };
            }

            let elapsed = polyhal::timer::get_ticks().wrapping_sub(start);
            if elapsed >= timeout_ticks {
                panic!(
                    "KernelHeapAllocator lock timeout: waiter_hart={} elapsed_ticks={} owner_hart={} owner_line={} owner_op={} owner_ptr={:#x} owner_size={} owner_align={}",
                    polyhal::arch::hart_id(),
                    elapsed,
                    self.inner.owner_hart(),
                    self.inner.owner_line(),
                    HEAP_OWNER_OP.load(Ordering::Acquire),
                    HEAP_OWNER_PTR.load(Ordering::Relaxed),
                    HEAP_OWNER_SIZE.load(Ordering::Relaxed),
                    HEAP_OWNER_ALIGN.load(Ordering::Relaxed),
                );
            }
            core::hint::spin_loop();
        }
    }
}

unsafe impl GlobalAlloc for KernelHeapAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let alloc_from_heap = || {
            let mut heap = self.lock(HEAP_OP_ALLOC, 0, layout.size(), layout.align());
            heap.alloc(layout)
                .ok()
                .map_or(core::ptr::null_mut(), |allocation| allocation.as_ptr())
        };
        let mut ptr = alloc_from_heap();
        if ptr.is_null() && grow_heap(layout) {
            ptr = alloc_from_heap();
        }
        if ptr.is_null() {
            print_heap_alloc_error_snapshot_once(layout);
        } else {
            let address = ptr as usize;
            let allocated_size = rounded_request_bytes(layout).unwrap_or(layout.size().max(1));
            if !heap_range_is_owned(address, allocated_size) || address % layout.align() != 0 {
                let info = heap_pointer_info(address);
                log::error!(
                    "[KERNEL_HEAP_RETURN_CORRUPTION] cpu={} ptr={:#x} size={} align={} info={:?}",
                    polyhal::arch::hart_id(),
                    address,
                    layout.size(),
                    layout.align(),
                    info,
                );
                panic!("kernel heap returned an address outside its registered extents");
            }
            record_heap_alloc(layout);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let address = ptr as usize;
        let allocated_size = rounded_request_bytes(layout).unwrap_or(layout.size().max(1));
        if !heap_range_is_owned(address, allocated_size) || address % layout.align() != 0 {
            let info = heap_pointer_info(address);
            log::error!(
                "[KERNEL_HEAP_INVALID_FREE] cpu={} ptr={:#x} size={} align={} info={:?}",
                polyhal::arch::hart_id(),
                address,
                layout.size(),
                layout.align(),
                info,
            );
            panic!("kernel heap received an address outside its registered extents");
        }
        unsafe {
            self.lock(HEAP_OP_DEALLOC, address, layout.size(), layout.align())
                .dealloc(NonNull::new_unchecked(ptr), layout);
        }
        record_heap_dealloc(layout);
    }
}

fn record_heap_grow_failure(reason: usize) -> bool {
    HEAP_GROW_FAILURES.fetch_add(1, Ordering::Relaxed);
    HEAP_GROW_LAST_FAILURE.store(reason, Ordering::Relaxed);
    false
}

fn heap_grow_failure_name(reason: usize) -> &'static str {
    match reason {
        HEAP_GROW_FAILURE_NONE => "none",
        HEAP_GROW_FAILURE_DISABLED => "disabled",
        HEAP_GROW_FAILURE_LAYOUT => "unsupported_layout",
        HEAP_GROW_FAILURE_LIMIT => "heap_limit",
        HEAP_GROW_FAILURE_FRAMES => "frame_reserve_or_fragmentation",
        _ => "unknown",
    }
}

fn heap_growth_size(layout: Layout) -> Option<usize> {
    let required = layout
        .size()
        .max(layout.align())
        .max(size_of::<usize>())
        .checked_next_power_of_two()?;
    if required.trailing_zeros() as usize >= KERNEL_HEAP_ORDER {
        return None;
    }
    Some(required.max(KERNEL_HEAP_GROW_CHUNK_SIZE))
}

fn reserve_heap_growth(bytes: usize) -> bool {
    let limit = HEAP_GROWTH_LIMIT.load(Ordering::Acquire);
    let mut accounted = HEAP_GROW_ACCOUNTED_BYTES.load(Ordering::Acquire);
    loop {
        let Some(next) = accounted.checked_add(bytes) else {
            return false;
        };
        if next > limit {
            return false;
        }
        match HEAP_GROW_ACCOUNTED_BYTES.compare_exchange_weak(
            accounted,
            next,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return true,
            Err(current) => accounted = current,
        }
    }
}

fn grow_heap(layout: Layout) -> bool {
    if !HEAP_GROWTH_ENABLED.load(Ordering::Acquire) {
        return record_heap_grow_failure(HEAP_GROW_FAILURE_DISABLED);
    }

    let Some(bytes) = heap_growth_size(layout) else {
        return record_heap_grow_failure(HEAP_GROW_FAILURE_LAYOUT);
    };
    if !reserve_heap_growth(bytes) {
        return record_heap_grow_failure(HEAP_GROW_FAILURE_LIMIT);
    }

    // Contiguous-frame discovery can walk a large recycled-frame list.  It
    // must happen without holding the global buddy-heap lock so unrelated
    // allocations on other CPUs can continue.
    let frame = crate::mm::frame_stats();
    let reserve_pages = (frame.total_pages / 8)
        .max(KERNEL_HEAP_MIN_FRAME_RESERVE / PAGE_SIZE)
        .min(frame.total_pages / 2);
    let pages = bytes / PAGE_SIZE;
    let Some(extent) =
        crate::mm::frame_allocator::frame_alloc_heap_extent(pages, pages, reserve_pages)
    else {
        HEAP_GROW_ACCOUNTED_BYTES.fetch_sub(bytes, Ordering::AcqRel);
        return record_heap_grow_failure(HEAP_GROW_FAILURE_FRAMES);
    };

    let Some(phys_start) = extent.start.0.checked_mul(PAGE_SIZE) else {
        HEAP_GROW_ACCOUNTED_BYTES.fetch_sub(bytes, Ordering::AcqRel);
        return record_heap_grow_failure(HEAP_GROW_FAILURE_LAYOUT);
    };
    let Some(virt_start) = phys_start.checked_add(VIRT_ADDR_START) else {
        HEAP_GROW_ACCOUNTED_BYTES.fetch_sub(bytes, Ordering::AcqRel);
        return record_heap_grow_failure(HEAP_GROW_FAILURE_LAYOUT);
    };
    let Some(virt_end) = virt_start.checked_add(extent.pages * PAGE_SIZE) else {
        HEAP_GROW_ACCOUNTED_BYTES.fetch_sub(bytes, Ordering::AcqRel);
        return record_heap_grow_failure(HEAP_GROW_FAILURE_LAYOUT);
    };
    crate::mm::vm_set::assert_kernel_heap_extent_direct_mapped(extent.start.0, extent.pages);
    {
        let mut heap = HEAP_ALLOCATOR.lock(HEAP_OP_GROW, virt_start, bytes, PAGE_SIZE);
        unsafe {
            heap.add_to_heap(virt_start, virt_end);
        }
        record_heap_extent(virt_start, virt_end);
    }
    HEAP_GROWN_BYTES.fetch_add(extent.pages * PAGE_SIZE, Ordering::Relaxed);
    HEAP_GROW_COUNT.fetch_add(1, Ordering::Relaxed);
    HEAP_GROW_LAST_FAILURE.store(HEAP_GROW_FAILURE_NONE, Ordering::Relaxed);
    true
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
            log::error!(
                "[OOM] heap_bucket: size=[{},inf) current_bytes={} rounded_bytes={} current_allocs={} alloc_count={} free_count={}",
                min,
                current_bytes,
                rounded_bytes,
                current_allocs,
                HEAP_BUCKET_ALLOC_COUNT[bucket].load(Ordering::Relaxed),
                HEAP_BUCKET_FREE_COUNT[bucket].load(Ordering::Relaxed)
            );
        } else {
            log::error!(
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
    log::error!(
        "[OOM] kernel_heap_alloc failed: request_size={} align={} rounded_order_bytes={} heap_total={} heap_free={} page_size={}",
        layout.size(),
        layout.align(),
        rounded.unwrap_or(0),
        heap.total,
        heap.free,
        PAGE_SIZE
    );
    log::error!(
        "[OOM] heap: user={} actual={} free={} total={} hint={}",
        heap.user,
        heap.actual,
        heap.free,
        heap.total,
        heap_alloc_failure_hint(layout, heap, rounded)
    );
    log::error!(
        "[OOM] heap_growth: enabled={} bootstrap={} grown={} extents={} failures={} limit={} last_failure={}",
        HEAP_GROWTH_ENABLED.load(Ordering::Relaxed),
        KERNEL_HEAP_BOOTSTRAP_SIZE,
        heap.grown,
        heap.growth_count,
        heap.growth_failures,
        heap.growth_limit,
        heap_grow_failure_name(HEAP_GROW_LAST_FAILURE.load(Ordering::Relaxed))
    );
    print_heap_bucket_snapshot();

    if let Some(frame) = crate::mm::try_frame_stats() {
        log::error!(
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
        log::error!("[OOM] frames: allocator_lock_busy=true");
    }

    if let Some(stats) = crate::fs::page::pagecache::PAGE_CACHE.try_snapshot() {
        log::error!(
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
        log::error!("[OOM] page_cache: lock_busy=true");
    }
    let page_cache_atomic = crate::fs::page::pagecache::atomic_stats();
    log::error!(
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
    log::error!(
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
    log::error!(
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
    log::error!(
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
    log::error!(
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
    log::error!(
        "[OOM] task_lifecycle: created={} dropped={} live_delta={} deferred_exited_tasks={}",
        task_lifecycle.created,
        task_lifecycle.dropped,
        task_lifecycle.live_delta,
        crate::task::deferred_exited_task_count()
    );
    let id_stats = crate::task::task_id_stats();
    log::error!(
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
    log::error!(
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
    log::error!(
        "[OOM] tid2task: entries={} live={} dead={} lock_busy={}",
        tid_stats.entries,
        tid_stats.live,
        tid_stats.dead,
        tid_stats.lock_busy
    );
    let futex_stats = crate::syscall::futex::stats();
    log::error!(
        "[OOM] futex: queues={} waiters={} lock_busy={}",
        futex_stats.queues,
        futex_stats.waiters,
        futex_stats.lock_busy
    );
    let pipe_stats = crate::fs::pipe::pipe_stats();
    log::error!(
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
    log::error!(
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
    log::error!(
        "[OOM] new_mount: fs_contexts={} fs_context_pids={} max_contexts_per_pid={} max_contexts_pid={} mount_attrs={} lock_busy={}",
        new_mount.fs_contexts,
        new_mount.fs_context_pids,
        new_mount.max_contexts_per_pid,
        new_mount.max_contexts_pid,
        new_mount.mount_attrs,
        new_mount.lock_busy
    );

    let fs = crate::fs::try_fs_retention_stats();
    log::error!(
        "[OOM] fs_retention: filesystems={} superblocks={} locked_super_tables={} lock_busy={}",
        fs.filesystems,
        fs.superblocks,
        fs.locked_super_tables,
        fs.lock_busy
    );
    log::error!(
        "[OOM] inode_holes: punched_hole_pages={}",
        crate::fs::vfs::inode::punched_hole_page_count()
    );

    let lwext4_alloc = lwext4_rust::allocation_stats();
    log::error!(
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
        log::error!("[OOM] writeback: pending_files={}", pending);
    } else {
        log::error!("[OOM] writeback: queue_lock_busy=true");
    }

    if let Some(swap) = crate::mm::swap::try_stats() {
        log::error!(
            "[OOM] swap: enabled={} used_slots={} free_slots={} total_slots={} alloc_count={} free_count={}",
            swap.enabled,
            swap.used_slots,
            swap.free_slots,
            swap.total_slots,
            swap.alloc_count,
            swap.free_count
        );
    } else {
        log::error!("[OOM] swap: lock_busy=true");
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

#[repr(C, align(4096))]
struct BootstrapHeap([u8; KERNEL_HEAP_BOOTSTRAP_SIZE]);

/// Small static heap used until the physical frame allocator is available.
static mut HEAP_SPACE: BootstrapHeap = BootstrapHeap([0; KERNEL_HEAP_BOOTSTRAP_SIZE]);

/// initiate heap allocator
pub fn init_heap() {
    unsafe {
        HEAP_ALLOCATOR
            .lock(HEAP_OP_INIT, 0, KERNEL_HEAP_BOOTSTRAP_SIZE, PAGE_SIZE)
            .init(
                addr_of_mut!(HEAP_SPACE) as usize,
                KERNEL_HEAP_BOOTSTRAP_SIZE,
            );
    }
}

/// Enable grow-on-demand after the physical frame allocator is initialized.
pub fn enable_heap_growth() {
    let total_frame_bytes = crate::mm::frame_stats()
        .total_pages
        .saturating_mul(PAGE_SIZE);
    HEAP_GROWTH_LIMIT.store(
        total_frame_bytes / KERNEL_HEAP_MAX_PHYS_FRACTION,
        Ordering::Relaxed,
    );
    HEAP_GROWTH_ENABLED.store(true, Ordering::Release);
}

#[allow(unused)]
#[allow(missing_docs)]
pub fn heap_test() {
    use alloc::boxed::Box;
    use alloc::vec::Vec;
    let a = Box::new(5);
    assert_eq!(*a, 5);
    drop(a);
    let mut v: Vec<usize> = Vec::new();
    for i in 0..500 {
        v.push(i);
    }
    for (i, val) in v.iter().take(500).enumerate() {
        assert_eq!(*val, i);
    }
    drop(v);

    assert!(HEAP_GROWTH_ENABLED.load(Ordering::Acquire));
    let mut dynamic = Vec::new();
    dynamic.resize(KERNEL_HEAP_BOOTSTRAP_SIZE + PAGE_SIZE, 0x5au8);
    assert!(dynamic.iter().step_by(PAGE_SIZE).all(|byte| *byte == 0x5a));
    assert!(heap_stats().grown >= KERNEL_HEAP_GROW_CHUNK_SIZE);
    drop(dynamic);
    polyhal::println!("heap_test passed!");
}
