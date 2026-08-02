//! The global allocator
use crate::config::MAX_CPU_NUM;
use crate::sync::{IrqGuard, SpinMutexGuard, SpinNoIrq, SpinNoIrqLock};
use polyhal::consts::{PAGE_SIZE, VIRT_ADDR_START};

use buddy_system_allocator::Heap;
use core::alloc::{GlobalAlloc, Layout};
use core::mem::size_of;
use core::ops::{Deref, DerefMut};
use core::ptr::{NonNull, addr_of_mut};
use core::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering};
use log::*;
use log::*;
use polyhal::print;

const KERNEL_HEAP_ORDER: usize = 32;
const KERNEL_HEAP_BOOTSTRAP_SIZE: usize = 1024 * 1024 * 1024;
const KERNEL_HEAP_GROW_CHUNK_SIZE: usize = 128 * 1024 * 1024;
const KERNEL_HEAP_MIN_FRAME_RESERVE: usize = 16 * 1024 * 1024;
const KERNEL_HEAP_MAX_PHYS_FRACTION: usize = 4;

// Small allocations come from per-CPU spans. The global buddy allocator only
// supplies and reclaims complete, naturally aligned spans; it never handles
// individual small objects.
const HEAP_SLAB_MIN_SIZE: usize = 16;
const HEAP_SLAB_MAX_SIZE: usize = PAGE_SIZE;
const HEAP_SLAB_CLASS_COUNT: usize = 9;
const HEAP_SLAB_SPAN_SIZE: usize = 64 * 1024;
const HEAP_SLAB_EMPTY_RESERVE: usize = 1;
const HEAP_SLAB_MAX_SLOTS: usize = HEAP_SLAB_SPAN_SIZE / HEAP_SLAB_MIN_SIZE;
const HEAP_SLAB_BITMAP_WORDS: usize = HEAP_SLAB_MAX_SLOTS.div_ceil(usize::BITS as usize);
const HEAP_SLAB_MAGIC: usize = 0x4b53_4c41_4253_504e;
const HEAP_SLAB_DEAD_MAGIC: usize = 0x4445_4144_5350_414e;
const _: () = assert!(HEAP_SLAB_MIN_SIZE << (HEAP_SLAB_CLASS_COUNT - 1) == HEAP_SLAB_MAX_SIZE);
const _: () = assert!(HEAP_SLAB_SPAN_SIZE.is_power_of_two());
const _: () = assert!(HEAP_SLAB_SPAN_SIZE >= PAGE_SIZE);
const _: () = assert!(HEAP_SLAB_SPAN_SIZE % PAGE_SIZE == 0);

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

/// Allocation-free snapshot of heap cache and global-lock behavior.
#[derive(Debug, Clone, Copy)]
pub struct HeapPerfStats {
    /// Successful caller-visible allocations.
    pub alloc_calls: usize,
    /// Caller-visible deallocations.
    pub free_calls: usize,
    /// Allocations served directly by a per-CPU slab span.
    pub cache_alloc_hits: usize,
    /// Allocations that had to obtain a new span for a per-CPU size class.
    pub cache_alloc_misses: usize,
    /// Deallocations returned to their owning per-CPU slab span.
    pub cache_free_hits: usize,
    /// Number of complete spans obtained from the global buddy heap.
    pub cache_refills: usize,
    /// Object slots supplied by all new spans.
    pub cache_refill_blocks: usize,
    /// Number of empty spans returned to the global buddy heap.
    pub cache_drains: usize,
    /// Object slots contained by all returned spans.
    pub cache_drain_blocks: usize,
    /// Free object bytes currently retained by all per-CPU spans.
    pub cache_bytes: usize,
    /// Successful blocks obtained directly from the global buddy heap.
    pub global_alloc_blocks: usize,
    /// Blocks returned directly to the global buddy heap.
    pub global_dealloc_blocks: usize,
    /// Acquisitions of the global buddy-heap lock.
    pub lock_acquisitions: usize,
    /// Acquisitions that failed their first lock attempt.
    pub lock_contended: usize,
    /// Retry-loop iterations spent waiting for the global lock.
    pub lock_spin_loops: usize,
    /// Cumulative timer ticks spent waiting for the global lock.
    pub lock_wait_ticks: usize,
    /// Longest observed global-lock wait in timer ticks.
    pub lock_max_wait_ticks: usize,
    /// CPU that observed `lock_max_wait_ticks`.
    pub lock_max_wait_cpu: usize,
    /// Contended direct allocation acquisitions.
    pub lock_contended_alloc: usize,
    /// Contended direct deallocation acquisitions.
    pub lock_contended_dealloc: usize,
    /// Contended heap growth acquisitions.
    pub lock_contended_grow: usize,
    /// Contended statistics acquisitions.
    pub lock_contended_stats: usize,
    /// Contended cache refill acquisitions.
    pub lock_contended_refill: usize,
    /// Contended cache drain acquisitions.
    pub lock_contended_drain: usize,
}

/// Return the current kernel heap allocator statistics.
pub fn heap_stats() -> HeapStats {
    let heap = HEAP_ALLOCATOR.lock(HEAP_OP_STATS, 0, 0, 0);
    let user = heap_live_usage().0;
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
/// Serialize grow-on-demand without holding the heap allocator lock while
/// physical frames and direct-map coverage are prepared.
static HEAP_GROW_IN_PROGRESS: AtomicBool = AtomicBool::new(false);
/// Packed completion/success counters published in one atomic store. A single
/// state prevents allocators from observing half of a completed flight.
static HEAP_GROW_STATE: AtomicUsize = AtomicUsize::new(0);
const HEAP_GROW_STATE_COUNTER_BITS: usize = usize::BITS as usize / 2;
const HEAP_GROW_STATE_COUNTER_MASK: usize = (1usize << HEAP_GROW_STATE_COUNTER_BITS) - 1;
static HEAP_GROWN_BYTES: AtomicUsize = AtomicUsize::new(0);
// Bytes already committed to the heap or reserved by the single active grow.
// This keeps grow-on-demand within HEAP_GROWTH_LIMIT.
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

#[derive(Clone, Copy)]
struct HeapSlabClass {
    head: usize,
    active: usize,
    span_count: usize,
    empty_count: usize,
}

impl HeapSlabClass {
    const fn new() -> Self {
        Self {
            head: 0,
            active: 0,
            span_count: 0,
            empty_count: 0,
        }
    }
}

#[repr(C, align(64))]
struct HeapSlabSpan {
    magic: usize,
    owner_cpu: usize,
    class: usize,
    next: usize,
    slot_start: usize,
    slot_count: usize,
    free_count: usize,
    bitmap_hint: usize,
    free_bitmap: [usize; HEAP_SLAB_BITMAP_WORDS],
}

const _: () = assert!(size_of::<HeapSlabSpan>() < HEAP_SLAB_SPAN_SIZE);

struct PerCpuHeapCache {
    classes: [HeapSlabClass; HEAP_SLAB_CLASS_COUNT],
}

impl PerCpuHeapCache {
    const fn new() -> Self {
        Self {
            classes: [HeapSlabClass::new(); HEAP_SLAB_CLASS_COUNT],
        }
    }
}

#[repr(align(64))]
struct PerCpuHeapCacheCell {
    inner: SpinNoIrqLock<PerCpuHeapCache>,
}

impl PerCpuHeapCacheCell {
    const fn new() -> Self {
        Self {
            inner: SpinNoIrqLock::new(PerCpuHeapCache::new()),
        }
    }
}

static HEAP_CPU_CACHES: [PerCpuHeapCacheCell; MAX_CPU_NUM] =
    [const { PerCpuHeapCacheCell::new() }; MAX_CPU_NUM];

#[repr(align(64))]
struct PerCpuHeapStats {
    current_bytes: [AtomicIsize; HEAP_ALLOC_BUCKETS],
    current_rounded_bytes: [AtomicIsize; HEAP_ALLOC_BUCKETS],
    current_allocs: [AtomicIsize; HEAP_ALLOC_BUCKETS],
    alloc_count: [AtomicUsize; HEAP_ALLOC_BUCKETS],
    free_count: [AtomicUsize; HEAP_ALLOC_BUCKETS],
}

impl PerCpuHeapStats {
    const fn new() -> Self {
        Self {
            current_bytes: [const { AtomicIsize::new(0) }; HEAP_ALLOC_BUCKETS],
            current_rounded_bytes: [const { AtomicIsize::new(0) }; HEAP_ALLOC_BUCKETS],
            current_allocs: [const { AtomicIsize::new(0) }; HEAP_ALLOC_BUCKETS],
            alloc_count: [const { AtomicUsize::new(0) }; HEAP_ALLOC_BUCKETS],
            free_count: [const { AtomicUsize::new(0) }; HEAP_ALLOC_BUCKETS],
        }
    }
}

static HEAP_CPU_STATS: [PerCpuHeapStats; MAX_CPU_NUM] =
    [const { PerCpuHeapStats::new() }; MAX_CPU_NUM];

#[repr(align(64))]
struct PerCpuHeapPerf {
    cache_alloc_hits: AtomicUsize,
    cache_alloc_misses: AtomicUsize,
    cache_free_hits: AtomicUsize,
    cache_refills: AtomicUsize,
    cache_refill_blocks: AtomicUsize,
    cache_drains: AtomicUsize,
    cache_drain_blocks: AtomicUsize,
    cache_bytes: AtomicUsize,
    global_alloc_blocks: AtomicUsize,
    global_dealloc_blocks: AtomicUsize,
    lock_acquisitions: AtomicUsize,
    lock_contended: AtomicUsize,
    lock_spin_loops: AtomicUsize,
    lock_wait_ticks: AtomicUsize,
    lock_max_wait_ticks: AtomicUsize,
    lock_contended_ops: [AtomicUsize; HEAP_OP_COUNT],
    active_op: AtomicUsize,
    active_ptr: AtomicUsize,
    active_size: AtomicUsize,
    active_align: AtomicUsize,
}

impl PerCpuHeapPerf {
    const fn new() -> Self {
        Self {
            cache_alloc_hits: AtomicUsize::new(0),
            cache_alloc_misses: AtomicUsize::new(0),
            cache_free_hits: AtomicUsize::new(0),
            cache_refills: AtomicUsize::new(0),
            cache_refill_blocks: AtomicUsize::new(0),
            cache_drains: AtomicUsize::new(0),
            cache_drain_blocks: AtomicUsize::new(0),
            cache_bytes: AtomicUsize::new(0),
            global_alloc_blocks: AtomicUsize::new(0),
            global_dealloc_blocks: AtomicUsize::new(0),
            lock_acquisitions: AtomicUsize::new(0),
            lock_contended: AtomicUsize::new(0),
            lock_spin_loops: AtomicUsize::new(0),
            lock_wait_ticks: AtomicUsize::new(0),
            lock_max_wait_ticks: AtomicUsize::new(0),
            lock_contended_ops: [const { AtomicUsize::new(0) }; HEAP_OP_COUNT],
            active_op: AtomicUsize::new(HEAP_OP_NONE),
            active_ptr: AtomicUsize::new(0),
            active_size: AtomicUsize::new(0),
            active_align: AtomicUsize::new(0),
        }
    }
}

static HEAP_CPU_PERF: [PerCpuHeapPerf; MAX_CPU_NUM] =
    [const { PerCpuHeapPerf::new() }; MAX_CPU_NUM];

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
const HEAP_OP_CACHE_REFILL: usize = 6;
const HEAP_OP_CACHE_DRAIN: usize = 7;
const HEAP_OP_COUNT: usize = 8;
const HEAP_LOCK_TIMEOUT_SECS: u64 = 2;

fn record_heap_lock_owner(cpu: usize, operation: usize, ptr: usize, size: usize, align: usize) {
    let perf = &HEAP_CPU_PERF[cpu];
    perf.active_ptr.store(ptr, Ordering::Relaxed);
    perf.active_size.store(size, Ordering::Relaxed);
    perf.active_align.store(align, Ordering::Relaxed);
    perf.active_op.store(operation, Ordering::Release);
}

fn record_heap_lock_acquired(
    cpu: usize,
    operation: usize,
    contended: bool,
    spin_loops: usize,
    wait_ticks: usize,
) {
    let perf = &HEAP_CPU_PERF[cpu];
    perf.lock_acquisitions.fetch_add(1, Ordering::Relaxed);
    if !contended {
        return;
    }
    perf.lock_contended.fetch_add(1, Ordering::Relaxed);
    perf.lock_spin_loops
        .fetch_add(spin_loops, Ordering::Relaxed);
    perf.lock_wait_ticks
        .fetch_add(wait_ticks, Ordering::Relaxed);
    perf.lock_max_wait_ticks
        .fetch_max(wait_ticks, Ordering::Relaxed);
    if operation < HEAP_OP_COUNT {
        perf.lock_contended_ops[operation].fetch_add(1, Ordering::Relaxed);
    }
}

struct KernelHeapGuard<'a> {
    guard: SpinMutexGuard<'a, Heap<KERNEL_HEAP_ORDER>, SpinNoIrq>,
    _irq_guard: IrqGuard,
    owner_cpu: usize,
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
        let perf = &HEAP_CPU_PERF[self.owner_cpu];
        perf.active_ptr.store(0, Ordering::Relaxed);
        perf.active_size.store(0, Ordering::Relaxed);
        perf.active_align.store(0, Ordering::Relaxed);
        perf.active_op.store(HEAP_OP_NONE, Ordering::Release);
    }
}

impl KernelHeapAllocator {
    fn lock(&self, operation: usize, ptr: usize, size: usize, align: usize) -> KernelHeapGuard<'_> {
        // The buddy allocator may legitimately scan a long free list while
        // coalescing. Retry counts run at different rates on different harts
        // and caused false deadlock panics, so this lock uses a wall-clock
        // bound while preserving the generic 0x1000000 detector elsewhere.
        let irq_guard = IrqGuard::new();
        let cpu = heap_cpu_index();
        if let Some(guard) = self.inner.try_lock_for_diagnostics() {
            record_heap_lock_acquired(cpu, operation, false, 0, 0);
            record_heap_lock_owner(cpu, operation, ptr, size, align);
            return KernelHeapGuard {
                guard,
                _irq_guard: irq_guard,
                owner_cpu: cpu,
            };
        }

        let start = polyhal::timer::get_ticks();
        let timeout_ticks = polyhal::timer::get_freq().saturating_mul(HEAP_LOCK_TIMEOUT_SECS);
        let mut spin_loops = 0usize;
        loop {
            // Test-and-test-and-set: while another CPU owns the heap, poll
            // with shared loads. Repeated failed CAS operations would keep
            // taking the lock cache line exclusively and amplify contention.
            while self.inner.is_locked() {
                spin_loops = spin_loops.saturating_add(1);
                if spin_loops & 0x3ff == 0 {
                    let elapsed = polyhal::timer::get_ticks().wrapping_sub(start);
                    if elapsed >= timeout_ticks {
                        let owner_cpu = self.inner.owner_hart();
                        let owner = HEAP_CPU_PERF.get(owner_cpu);
                        panic!(
                            "KernelHeapAllocator lock timeout: waiter_hart={} elapsed_ticks={} owner_hart={} owner_line={} owner_op={} owner_ptr={:#x} owner_size={} owner_align={}",
                            polyhal::arch::hart_id(),
                            elapsed,
                            owner_cpu,
                            self.inner.owner_line(),
                            owner.map_or(HEAP_OP_NONE, |perf| perf
                                .active_op
                                .load(Ordering::Acquire)),
                            owner.map_or(0, |perf| perf.active_ptr.load(Ordering::Relaxed)),
                            owner.map_or(0, |perf| perf.active_size.load(Ordering::Relaxed)),
                            owner.map_or(0, |perf| perf.active_align.load(Ordering::Relaxed)),
                        );
                    }
                }
                core::hint::spin_loop();
            }

            if let Some(guard) = self.inner.try_lock_for_diagnostics() {
                let elapsed = polyhal::timer::get_ticks().wrapping_sub(start);
                record_heap_lock_acquired(
                    cpu,
                    operation,
                    true,
                    spin_loops,
                    elapsed.min(usize::MAX as u64) as usize,
                );
                record_heap_lock_owner(cpu, operation, ptr, size, align);
                return KernelHeapGuard {
                    guard,
                    _irq_guard: irq_guard,
                    owner_cpu: cpu,
                };
            }

            // Another waiter won the race after the read-side polling loop.
            spin_loops = spin_loops.saturating_add(1);
            core::hint::spin_loop();
        }
    }
}

unsafe impl GlobalAlloc for KernelHeapAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = if let Some(class) = heap_slab_class(layout) {
            unsafe { alloc_from_cpu_slab(self, layout, class) }
        } else {
            alloc_from_global_heap(self, layout, HEAP_OP_ALLOC)
        };
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
        if let Some(class) = heap_slab_class(layout) {
            unsafe {
                dealloc_to_cpu_slab(self, ptr, class);
            }
        } else {
            unsafe {
                self.lock(HEAP_OP_DEALLOC, address, layout.size(), layout.align())
                    .dealloc(NonNull::new_unchecked(ptr), layout);
            }
            HEAP_CPU_PERF[heap_cpu_index()]
                .global_dealloc_blocks
                .fetch_add(1, Ordering::Relaxed);
        }
        record_heap_dealloc(layout);
    }
}

#[inline]
fn heap_cpu_index() -> usize {
    polyhal::arch::hart_id().min(MAX_CPU_NUM - 1)
}

#[inline]
fn heap_slab_class(layout: Layout) -> Option<usize> {
    let size = rounded_request_bytes(layout)?.max(HEAP_SLAB_MIN_SIZE);
    if size > HEAP_SLAB_MAX_SIZE {
        return None;
    }
    let class = size.trailing_zeros() as usize - HEAP_SLAB_MIN_SIZE.trailing_zeros() as usize;
    (class < HEAP_SLAB_CLASS_COUNT).then_some(class)
}

#[inline]
fn heap_slab_class_size(class: usize) -> usize {
    HEAP_SLAB_MIN_SIZE << class
}

#[inline]
fn heap_slab_span_layout() -> Layout {
    unsafe { Layout::from_size_align_unchecked(HEAP_SLAB_SPAN_SIZE, HEAP_SLAB_SPAN_SIZE) }
}

fn alloc_from_global_heap(
    allocator: &KernelHeapAllocator,
    layout: Layout,
    operation: usize,
) -> *mut u8 {
    loop {
        let observed_growth_state = HEAP_GROW_STATE.load(Ordering::Acquire);
        let ptr = {
            let mut heap = allocator.lock(operation, 0, layout.size(), layout.align());
            heap.alloc(layout)
                .ok()
                .map_or(core::ptr::null_mut(), |allocation| allocation.as_ptr())
        };
        if !ptr.is_null() {
            HEAP_CPU_PERF[heap_cpu_index()]
                .global_alloc_blocks
                .fetch_add(1, Ordering::Relaxed);
            return ptr;
        }
        if !grow_heap(layout, observed_growth_state) {
            return core::ptr::null_mut();
        }
        // The current CPU either published one new extent or waited for the
        // single active grower. Retry before considering another growth flight.
    }
}

#[inline]
fn heap_slab_span_base(pointer: usize) -> usize {
    pointer & !(HEAP_SLAB_SPAN_SIZE - 1)
}

unsafe fn initialize_heap_slab_span(address: usize, owner_cpu: usize, class: usize) -> usize {
    debug_assert_eq!(address & (HEAP_SLAB_SPAN_SIZE - 1), 0);
    let class_size = heap_slab_class_size(class);
    let slot_start = (address + size_of::<HeapSlabSpan>() + class_size - 1) & !(class_size - 1);
    let slot_count = (address + HEAP_SLAB_SPAN_SIZE - slot_start) / class_size;
    assert!(slot_count != 0 && slot_count <= HEAP_SLAB_MAX_SLOTS);

    let span = address as *mut HeapSlabSpan;
    unsafe {
        span.write(HeapSlabSpan {
            magic: HEAP_SLAB_MAGIC,
            owner_cpu,
            class,
            next: 0,
            slot_start,
            slot_count,
            free_count: slot_count,
            bitmap_hint: 0,
            free_bitmap: [0; HEAP_SLAB_BITMAP_WORDS],
        });
        for index in 0..slot_count {
            (*span).free_bitmap[index / usize::BITS as usize] |=
                1usize << (index % usize::BITS as usize);
        }
    }
    slot_count
}

unsafe fn heap_slab_take_from_span(span: &mut HeapSlabSpan) -> Option<usize> {
    if span.free_count == 0 {
        return None;
    }
    for offset in 0..HEAP_SLAB_BITMAP_WORDS {
        let word_index = (span.bitmap_hint + offset) % HEAP_SLAB_BITMAP_WORDS;
        let word = &mut span.free_bitmap[word_index];
        if *word == 0 {
            continue;
        }
        let bit = word.trailing_zeros() as usize;
        let index = word_index * usize::BITS as usize + bit;
        assert!(
            index < span.slot_count,
            "kernel slab bitmap contains an invalid free slot"
        );
        *word &= !(1usize << bit);
        span.free_count -= 1;
        span.bitmap_hint = if *word == 0 {
            (word_index + 1) % HEAP_SLAB_BITMAP_WORDS
        } else {
            word_index
        };
        return Some(span.slot_start + index * heap_slab_class_size(span.class));
    }
    panic!("kernel slab free-count/bitmap mismatch");
}

unsafe fn heap_slab_take_from_class(class: &mut HeapSlabClass) -> Option<usize> {
    if class.active != 0 {
        let span = unsafe { &mut *(class.active as *mut HeapSlabSpan) };
        let was_empty = span.free_count == span.slot_count;
        if let Some(pointer) = unsafe { heap_slab_take_from_span(span) } {
            if was_empty {
                class.empty_count -= 1;
            }
            return Some(pointer);
        }
        class.active = 0;
    }

    let mut current = class.head;
    while current != 0 {
        let span = unsafe { &mut *(current as *mut HeapSlabSpan) };
        assert_eq!(span.magic, HEAP_SLAB_MAGIC, "kernel slab list corruption");
        let was_empty = span.free_count == span.slot_count;
        if let Some(pointer) = unsafe { heap_slab_take_from_span(span) } {
            if was_empty {
                class.empty_count -= 1;
            }
            class.active = current;
            return Some(pointer);
        }
        current = span.next;
    }
    None
}

unsafe fn heap_slab_insert(class: &mut HeapSlabClass, span_address: usize) {
    let span = unsafe { &mut *(span_address as *mut HeapSlabSpan) };
    span.next = class.head;
    class.head = span_address;
    class.active = span_address;
    class.span_count += 1;
    class.empty_count += 1;
}

unsafe fn heap_slab_remove(class: &mut HeapSlabClass, span_address: usize) {
    let mut previous = 0usize;
    let mut current = class.head;
    while current != 0 {
        let span = unsafe { &mut *(current as *mut HeapSlabSpan) };
        if current == span_address {
            if previous == 0 {
                class.head = span.next;
            } else {
                unsafe { (*(previous as *mut HeapSlabSpan)).next = span.next };
            }
            if class.active == current {
                class.active = 0;
            }
            class.span_count -= 1;
            class.empty_count -= 1;
            span.next = 0;
            return;
        }
        previous = current;
        current = span.next;
    }
    panic!("kernel slab span is missing from its owner list");
}

unsafe fn alloc_from_cpu_slab(
    allocator: &KernelHeapAllocator,
    layout: Layout,
    class: usize,
) -> *mut u8 {
    let cpu = heap_cpu_index();
    let cached = {
        let mut cache = HEAP_CPU_CACHES[cpu].inner.lock();
        unsafe { heap_slab_take_from_class(&mut cache.classes[class]) }
    };
    if let Some(pointer) = cached {
        let perf = &HEAP_CPU_PERF[cpu];
        perf.cache_alloc_hits.fetch_add(1, Ordering::Relaxed);
        perf.cache_bytes
            .fetch_sub(heap_slab_class_size(class), Ordering::Relaxed);
        return pointer as *mut u8;
    }

    HEAP_CPU_PERF[cpu]
        .cache_alloc_misses
        .fetch_add(1, Ordering::Relaxed);
    let span_layout = heap_slab_span_layout();
    let span_address =
        alloc_from_global_heap(allocator, span_layout, HEAP_OP_CACHE_REFILL) as usize;
    if span_address == 0 {
        return core::ptr::null_mut();
    }

    // Scheduling may have moved the caller while the global span was being
    // obtained. Assign the complete span to the CPU executing now.
    let refill_cpu = heap_cpu_index();
    let slot_count = unsafe { initialize_heap_slab_span(span_address, refill_cpu, class) };
    let pointer = {
        let mut cache = HEAP_CPU_CACHES[refill_cpu].inner.lock();
        unsafe {
            heap_slab_insert(&mut cache.classes[class], span_address);
            heap_slab_take_from_class(&mut cache.classes[class])
                .expect("a new kernel slab span must contain at least one slot")
        }
    };
    let perf = &HEAP_CPU_PERF[refill_cpu];
    perf.cache_refills.fetch_add(1, Ordering::Relaxed);
    perf.cache_refill_blocks
        .fetch_add(slot_count, Ordering::Relaxed);
    perf.cache_bytes.fetch_add(
        (slot_count - 1) * heap_slab_class_size(class),
        Ordering::Relaxed,
    );

    debug_assert!(pointer % layout.align() == 0);
    pointer as *mut u8
}

unsafe fn dealloc_to_cpu_slab(allocator: &KernelHeapAllocator, ptr: *mut u8, class: usize) {
    let pointer = ptr as usize;
    let span_address = heap_slab_span_base(pointer);
    let span = unsafe { &*(span_address as *const HeapSlabSpan) };
    assert_eq!(
        span.magic, HEAP_SLAB_MAGIC,
        "kernel slab free has an invalid span"
    );
    assert_eq!(
        span.class, class,
        "kernel slab free has the wrong size class"
    );
    assert!(
        span.owner_cpu < MAX_CPU_NUM,
        "kernel slab span has an invalid owner"
    );
    let owner_cpu = span.owner_cpu;
    let class_size = heap_slab_class_size(class);
    let mut reclaim = false;
    let slot_count;

    {
        let mut cache = HEAP_CPU_CACHES[owner_cpu].inner.lock();
        let slab_class = &mut cache.classes[class];
        let span = unsafe { &mut *(span_address as *mut HeapSlabSpan) };
        assert_eq!(
            span.magic, HEAP_SLAB_MAGIC,
            "kernel slab span was reclaimed too early"
        );
        assert!(pointer >= span.slot_start);
        let offset = pointer - span.slot_start;
        assert_eq!(
            offset % class_size,
            0,
            "kernel slab free is not slot-aligned"
        );
        let slot = offset / class_size;
        assert!(
            slot < span.slot_count,
            "kernel slab free is outside the span slots"
        );
        let word = &mut span.free_bitmap[slot / usize::BITS as usize];
        let mask = 1usize << (slot % usize::BITS as usize);
        assert_eq!(*word & mask, 0, "kernel slab double free: ptr={pointer:#x}");

        let was_full = span.free_count == 0;
        *word |= mask;
        span.bitmap_hint = slot / usize::BITS as usize;
        span.free_count += 1;
        slot_count = span.slot_count;
        if was_full {
            slab_class.active = span_address;
        }
        if span.free_count == span.slot_count {
            slab_class.empty_count += 1;
            if slab_class.empty_count > HEAP_SLAB_EMPTY_RESERVE && slab_class.span_count > 1 {
                unsafe { heap_slab_remove(slab_class, span_address) };
                reclaim = true;
            }
        }
    }

    let perf = &HEAP_CPU_PERF[owner_cpu];
    perf.cache_free_hits.fetch_add(1, Ordering::Relaxed);
    perf.cache_bytes.fetch_add(class_size, Ordering::Relaxed);
    if !reclaim {
        return;
    }

    perf.cache_drains.fetch_add(1, Ordering::Relaxed);
    perf.cache_drain_blocks
        .fetch_add(slot_count, Ordering::Relaxed);
    perf.cache_bytes
        .fetch_sub(slot_count * class_size, Ordering::Relaxed);
    unsafe {
        (*(span_address as *mut HeapSlabSpan)).magic = HEAP_SLAB_DEAD_MAGIC;
        allocator
            .lock(
                HEAP_OP_CACHE_DRAIN,
                span_address,
                HEAP_SLAB_SPAN_SIZE,
                HEAP_SLAB_SPAN_SIZE,
            )
            .dealloc(
                NonNull::new_unchecked(span_address as *mut u8),
                heap_slab_span_layout(),
            );
    }
    perf.global_dealloc_blocks.fetch_add(1, Ordering::Relaxed);
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

fn heap_growth_completions(state: usize) -> usize {
    state >> HEAP_GROW_STATE_COUNTER_BITS
}

fn heap_growth_successes(state: usize) -> usize {
    state & HEAP_GROW_STATE_COUNTER_MASK
}

fn next_heap_growth_state(state: usize, succeeded: bool) -> usize {
    let completions = heap_growth_completions(state).wrapping_add(1) & HEAP_GROW_STATE_COUNTER_MASK;
    let successes = heap_growth_successes(state).wrapping_add(usize::from(succeeded))
        & HEAP_GROW_STATE_COUNTER_MASK;
    (completions << HEAP_GROW_STATE_COUNTER_BITS) | successes
}

fn heap_growth_succeeded_since(observed_state: usize) -> bool {
    heap_growth_successes(HEAP_GROW_STATE.load(Ordering::Acquire))
        != heap_growth_successes(observed_state)
}

/// Join or lead one heap-growth flight.
///
/// The caller snapshots `HEAP_GROW_STATE` before its failed allocation.
/// If another CPU completes a successful growth after that snapshot, this
/// function returns without reserving another extent so the caller retries the
/// newly enlarged heap first.
fn grow_heap(layout: Layout, observed_state: usize) -> bool {
    loop {
        if HEAP_GROW_STATE.load(Ordering::Acquire) != observed_state {
            return heap_growth_succeeded_since(observed_state);
        }

        if HEAP_GROW_IN_PROGRESS
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            // A grower may have completed between our sequence check and the
            // ownership CAS. Do not turn that race into a second 128 MiB grow.
            if HEAP_GROW_STATE.load(Ordering::Acquire) != observed_state {
                HEAP_GROW_IN_PROGRESS.store(false, Ordering::Release);
                return heap_growth_succeeded_since(observed_state);
            }

            let succeeded = grow_heap_once(layout);
            HEAP_GROW_STATE.store(
                next_heap_growth_state(observed_state, succeeded),
                Ordering::Release,
            );
            HEAP_GROW_IN_PROGRESS.store(false, Ordering::Release);
            return succeeded;
        }

        // Global allocation can be entered below arbitrary kernel locks, so
        // waiting here must remain allocation-free and cannot block through a
        // scheduler primitive. This loop adds no IRQ masking of its own; it
        // preserves whatever interrupt state the allocation caller had.
        while HEAP_GROW_IN_PROGRESS.load(Ordering::Acquire)
            && HEAP_GROW_STATE.load(Ordering::Acquire) == observed_state
        {
            core::hint::spin_loop();
        }
    }
}

fn grow_heap_once(layout: Layout) -> bool {
    if !HEAP_GROWTH_ENABLED.load(Ordering::Acquire) {
        return record_heap_grow_failure(HEAP_GROW_FAILURE_DISABLED);
    }

    let Some(bytes) = heap_growth_size(layout) else {
        return record_heap_grow_failure(HEAP_GROW_FAILURE_LAYOUT);
    };
    if !reserve_heap_growth(bytes) {
        return record_heap_grow_failure(HEAP_GROW_FAILURE_LIMIT);
    }

    // Contiguous-frame discovery can split and merge central buddy extents. It
    // must happen without holding the global heap lock so unrelated small
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
    let stats = &HEAP_CPU_STATS[heap_cpu_index()];
    stats.current_bytes[bucket].fetch_add(size as isize, Ordering::Relaxed);
    stats.current_rounded_bytes[bucket].fetch_add(rounded as isize, Ordering::Relaxed);
    stats.current_allocs[bucket].fetch_add(1, Ordering::Relaxed);
    stats.alloc_count[bucket].fetch_add(1, Ordering::Relaxed);
}

fn record_heap_dealloc(layout: Layout) {
    let size = layout.size().max(1);
    let rounded = rounded_request_bytes(layout).unwrap_or(size);
    let bucket = heap_bucket_index(size);
    let stats = &HEAP_CPU_STATS[heap_cpu_index()];
    stats.current_bytes[bucket].fetch_sub(size as isize, Ordering::Relaxed);
    stats.current_rounded_bytes[bucket].fetch_sub(rounded as isize, Ordering::Relaxed);
    stats.current_allocs[bucket].fetch_sub(1, Ordering::Relaxed);
    stats.free_count[bucket].fetch_add(1, Ordering::Relaxed);
}

fn heap_bucket_totals(bucket: usize) -> (usize, usize, usize, usize, usize) {
    let mut current_bytes = 0isize;
    let mut current_rounded_bytes = 0isize;
    let mut current_allocs = 0isize;
    let mut alloc_count = 0usize;
    let mut free_count = 0usize;
    for stats in &HEAP_CPU_STATS {
        current_bytes =
            current_bytes.saturating_add(stats.current_bytes[bucket].load(Ordering::Relaxed));
        current_rounded_bytes = current_rounded_bytes
            .saturating_add(stats.current_rounded_bytes[bucket].load(Ordering::Relaxed));
        current_allocs =
            current_allocs.saturating_add(stats.current_allocs[bucket].load(Ordering::Relaxed));
        alloc_count = alloc_count.saturating_add(stats.alloc_count[bucket].load(Ordering::Relaxed));
        free_count = free_count.saturating_add(stats.free_count[bucket].load(Ordering::Relaxed));
    }
    (
        current_bytes.max(0) as usize,
        current_rounded_bytes.max(0) as usize,
        current_allocs.max(0) as usize,
        alloc_count,
        free_count,
    )
}

fn heap_live_usage() -> (usize, usize, usize) {
    let mut current_bytes = 0usize;
    let mut current_rounded_bytes = 0usize;
    let mut current_allocs = 0usize;
    for bucket in 0..HEAP_ALLOC_BUCKETS {
        let totals = heap_bucket_totals(bucket);
        current_bytes = current_bytes.saturating_add(totals.0);
        current_rounded_bytes = current_rounded_bytes.saturating_add(totals.1);
        current_allocs = current_allocs.saturating_add(totals.2);
    }
    (current_bytes, current_rounded_bytes, current_allocs)
}

/// Return cumulative heap fast-path and global-lock counters without taking
/// either the heap lock or a per-CPU cache lock.
pub fn heap_perf_stats() -> HeapPerfStats {
    let mut stats = HeapPerfStats {
        alloc_calls: 0,
        free_calls: 0,
        cache_alloc_hits: 0,
        cache_alloc_misses: 0,
        cache_free_hits: 0,
        cache_refills: 0,
        cache_refill_blocks: 0,
        cache_drains: 0,
        cache_drain_blocks: 0,
        cache_bytes: 0,
        global_alloc_blocks: 0,
        global_dealloc_blocks: 0,
        lock_acquisitions: 0,
        lock_contended: 0,
        lock_spin_loops: 0,
        lock_wait_ticks: 0,
        lock_max_wait_ticks: 0,
        lock_max_wait_cpu: usize::MAX,
        lock_contended_alloc: 0,
        lock_contended_dealloc: 0,
        lock_contended_grow: 0,
        lock_contended_stats: 0,
        lock_contended_refill: 0,
        lock_contended_drain: 0,
    };

    for cpu in 0..MAX_CPU_NUM {
        for bucket in 0..HEAP_ALLOC_BUCKETS {
            stats.alloc_calls = stats
                .alloc_calls
                .saturating_add(HEAP_CPU_STATS[cpu].alloc_count[bucket].load(Ordering::Relaxed));
            stats.free_calls = stats
                .free_calls
                .saturating_add(HEAP_CPU_STATS[cpu].free_count[bucket].load(Ordering::Relaxed));
        }

        let perf = &HEAP_CPU_PERF[cpu];
        stats.cache_alloc_hits = stats
            .cache_alloc_hits
            .saturating_add(perf.cache_alloc_hits.load(Ordering::Relaxed));
        stats.cache_alloc_misses = stats
            .cache_alloc_misses
            .saturating_add(perf.cache_alloc_misses.load(Ordering::Relaxed));
        stats.cache_free_hits = stats
            .cache_free_hits
            .saturating_add(perf.cache_free_hits.load(Ordering::Relaxed));
        stats.cache_refills = stats
            .cache_refills
            .saturating_add(perf.cache_refills.load(Ordering::Relaxed));
        stats.cache_refill_blocks = stats
            .cache_refill_blocks
            .saturating_add(perf.cache_refill_blocks.load(Ordering::Relaxed));
        stats.cache_drains = stats
            .cache_drains
            .saturating_add(perf.cache_drains.load(Ordering::Relaxed));
        stats.cache_drain_blocks = stats
            .cache_drain_blocks
            .saturating_add(perf.cache_drain_blocks.load(Ordering::Relaxed));
        stats.cache_bytes = stats
            .cache_bytes
            .saturating_add(perf.cache_bytes.load(Ordering::Relaxed));
        stats.global_alloc_blocks = stats
            .global_alloc_blocks
            .saturating_add(perf.global_alloc_blocks.load(Ordering::Relaxed));
        stats.global_dealloc_blocks = stats
            .global_dealloc_blocks
            .saturating_add(perf.global_dealloc_blocks.load(Ordering::Relaxed));
        stats.lock_acquisitions = stats
            .lock_acquisitions
            .saturating_add(perf.lock_acquisitions.load(Ordering::Relaxed));
        stats.lock_contended = stats
            .lock_contended
            .saturating_add(perf.lock_contended.load(Ordering::Relaxed));
        stats.lock_spin_loops = stats
            .lock_spin_loops
            .saturating_add(perf.lock_spin_loops.load(Ordering::Relaxed));
        stats.lock_wait_ticks = stats
            .lock_wait_ticks
            .saturating_add(perf.lock_wait_ticks.load(Ordering::Relaxed));
        let max_wait = perf.lock_max_wait_ticks.load(Ordering::Relaxed);
        if max_wait > stats.lock_max_wait_ticks {
            stats.lock_max_wait_ticks = max_wait;
            stats.lock_max_wait_cpu = cpu;
        }
        stats.lock_contended_alloc = stats
            .lock_contended_alloc
            .saturating_add(perf.lock_contended_ops[HEAP_OP_ALLOC].load(Ordering::Relaxed));
        stats.lock_contended_dealloc = stats
            .lock_contended_dealloc
            .saturating_add(perf.lock_contended_ops[HEAP_OP_DEALLOC].load(Ordering::Relaxed));
        stats.lock_contended_grow = stats
            .lock_contended_grow
            .saturating_add(perf.lock_contended_ops[HEAP_OP_GROW].load(Ordering::Relaxed));
        stats.lock_contended_stats = stats
            .lock_contended_stats
            .saturating_add(perf.lock_contended_ops[HEAP_OP_STATS].load(Ordering::Relaxed));
        stats.lock_contended_refill = stats
            .lock_contended_refill
            .saturating_add(perf.lock_contended_ops[HEAP_OP_CACHE_REFILL].load(Ordering::Relaxed));
        stats.lock_contended_drain = stats
            .lock_contended_drain
            .saturating_add(perf.lock_contended_ops[HEAP_OP_CACHE_DRAIN].load(Ordering::Relaxed));
    }
    stats
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
        let (current_bytes, rounded_bytes, current_allocs, alloc_count, free_count) =
            heap_bucket_totals(bucket);
        if current_bytes == 0 && current_allocs == 0 {
            continue;
        }
        let min = heap_bucket_min(bucket);
        let max = heap_bucket_max(bucket);
        if max == usize::MAX {
            log::error!(
                "[OOM] heap_bucket: size=[{},inf) current_bytes={} rounded_bytes={} current_allocs={} alloc_count={} free_count={}",
                min,
                current_bytes,
                rounded_bytes,
                current_allocs,
                alloc_count,
                free_count
            );
        } else {
            log::error!(
                "[OOM] heap_bucket: size=[{},{}] current_bytes={} rounded_bytes={} current_allocs={} alloc_count={} free_count={}",
                min,
                max,
                current_bytes,
                rounded_bytes,
                current_allocs,
                alloc_count,
                free_count
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

/// Static heap used until the physical frame allocator is available.
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
