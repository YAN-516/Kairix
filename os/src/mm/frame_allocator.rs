//! Implementation of [`FrameAllocator`] which
//! controls all the frames in the operating system.
use polyhal::consts::*;
use polyhal::print;
// use super::{PhysAddr, PhysPageNum};
use crate::sync::SpinNoIrqLock;
use alloc::vec::Vec;
use core::fmt::{self, Debug, Formatter};
use core::sync::atomic::{AtomicUsize, Ordering};
use lazy_static::*;
use log::{debug, error, info, warn};
use polyhal::arch::MEM_VECTOR_CAPACITY;
use polyhal::common::FrameTracker;
use polyhal::utils::addr::*;

static FRAME_ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static FRAME_FREE_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Snapshot of the physical frame allocator state.
#[derive(Debug, Clone, Copy)]
pub struct FrameStats {
    /// Cumulative successful frame allocation calls.
    pub alloc_count: usize,
    /// Cumulative frame frees.
    pub free_count: usize,
    /// Cumulative allocations minus frees.
    pub allocated_delta: usize,
    /// Pages currently available for allocation.
    pub free_pages: usize,
    /// Pages currently in use.
    pub used_pages: usize,
    /// Pages that have never been handed out.
    pub fresh_free_pages: usize,
    /// Freed pages waiting in the recycled list.
    pub recycled_pages: usize,
    /// Total pages managed by this allocator.
    pub total_pages: usize,
}

// /// manage a frame which has the same lifecycle as the tracker
// pub struct FrameTracker {
//     ///
//     pub ppn: PhysPageNum,
// }

// impl FrameTracker {
//     ///Create an empty `FrameTracker`
//     pub fn new(ppn: PhysPageNum) -> Self {
//         // page cleaning
//         let bytes_array = ppn.get_bytes_array();
//         for i in bytes_array {
//             *i = 0;
//         }
//         Self { ppn }
//     }

//     ///Create an empty `FrameTracker` while no pgtb
//     pub fn new_phy(ppn: PhysPageNum) -> Self {
//         println!("frame tracker new{}", ppn.0);
//         // page cleaning
//         let bytes_array = ppn.get_bytes_array_phy();
//         for i in bytes_array {
//             *i = 0;
//         }
//         Self { ppn }
//     }
// }

// impl Debug for FrameTracker {
//     fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
//         f.write_fmt(format_args!("FrameTracker:PPN={:#x}", self.ppn.0))
//     }
// }

// impl Drop for FrameTracker {
//     fn drop(&mut self) {
//         frame_dealloc(self.ppn);
//     }
// }

trait FrameAllocator {
    fn new() -> Self;
    fn alloc(&mut self) -> Option<PhysPageNum>;
    fn alloc_contiguous(&mut self, pages: usize, align_pages: usize) -> Option<FrameExtent>;
    fn dealloc(&mut self, ppn: PhysPageNum);
}

/// A physically contiguous range allocated without heap-backed metadata.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FrameExtent {
    pub(crate) start: PhysPageNum,
    pub(crate) pages: usize,
}

/// Contiguous physical page-number range managed by the allocator.
#[derive(Clone, Copy)]
struct FrameRange {
    start: usize,
    current: usize,
    end: usize,
}

const EMPTY_FRAME_RANGE: FrameRange = FrameRange {
    start: 0,
    current: 0,
    end: 0,
};
const RECYCLED_LIST_END: usize = usize::MAX;
const RECYCLED_LINK_COOKIE: usize = 0xd6e8_feb8_6659_fd93;

/// Physical frame allocator backed by platform-reported memory ranges.
pub struct StackFrameAllocator {
    ranges: [FrameRange; MEM_VECTOR_CAPACITY],
    range_count: usize,
    recycled_head: Option<usize>,
    recycled_tail: Option<usize>,
    recycled_insert_hint: Option<usize>,
    recycled_count: usize,
}

impl StackFrameAllocator {
    ///
    pub fn init(&mut self, l: PhysPageNum, r: PhysPageNum) {
        self.add_range(l, r);
    }

    fn add_range(&mut self, l: PhysPageNum, r: PhysPageNum) {
        if l >= r {
            return;
        }
        assert!(
            self.range_count < self.ranges.len(),
            "too many frame allocator regions"
        );
        self.ranges[self.range_count] = FrameRange {
            start: l.0,
            current: l.0,
            end: r.0,
        };
        self.range_count += 1;
    }

    fn ranges(&self) -> &[FrameRange] {
        &self.ranges[..self.range_count]
    }

    fn ranges_mut(&mut self) -> &mut [FrameRange] {
        &mut self.ranges[..self.range_count]
    }

    fn contains_ppn(&self, ppn: usize) -> bool {
        self.ranges()
            .iter()
            .any(|range| range.start <= ppn && ppn < range.end)
    }

    fn allocated_ppn(&self, ppn: usize) -> bool {
        self.ranges()
            .iter()
            .any(|range| range.start <= ppn && ppn < range.current)
    }

    fn free_pages(&self) -> usize {
        self.fresh_free_pages() + self.recycled_pages()
    }

    fn fresh_free_pages(&self) -> usize {
        self.ranges()
            .iter()
            .map(|range| range.end - range.current)
            .sum()
    }

    fn recycled_pages(&self) -> usize {
        self.recycled_count
    }

    fn total_pages(&self) -> usize {
        self.ranges()
            .iter()
            .map(|range| range.end - range.start)
            .sum()
    }

    fn recycled_link_ptr(ppn: usize) -> *mut usize {
        ((ppn << PAGE_SIZE_BITS) + VIRT_ADDR_START) as *mut usize
    }

    fn recycled_link_checksum(ppn: usize, next: usize) -> usize {
        next.rotate_left(17) ^ ppn.rotate_left(7) ^ RECYCLED_LINK_COOKIE
    }

    fn recycled_next(&self, ppn: usize) -> Result<Option<usize>, usize> {
        let link = Self::recycled_link_ptr(ppn);
        let next = unsafe { link.read() };
        let checksum = unsafe { link.add(1).read() };
        if checksum != Self::recycled_link_checksum(ppn, next) {
            return Err(next);
        }
        if next == RECYCLED_LIST_END {
            return Ok(None);
        }
        if next <= ppn || !self.contains_ppn(next) || !self.allocated_ppn(next) {
            return Err(next);
        }
        Ok(Some(next))
    }

    fn set_recycled_next(ppn: usize, next: Option<usize>) {
        let next = next.unwrap_or(RECYCLED_LIST_END);
        unsafe {
            let link = Self::recycled_link_ptr(ppn);
            link.write(next);
            link.add(1).write(Self::recycled_link_checksum(ppn, next));
        }
    }

    fn discard_corrupt_recycled_list(&mut self, ppn: usize, observed_next: usize) {
        error!(
            "corrupt recycled frame link: ppn={:#x} observed_next={:#x} discarded_pages={}",
            ppn, observed_next, self.recycled_count
        );
        self.recycled_head = None;
        self.recycled_tail = None;
        self.recycled_insert_hint = None;
        self.recycled_count = 0;
    }

    fn insert_recycled_range(&mut self, start: usize, end: usize) {
        if start >= end {
            return;
        }

        if self.recycled_head.is_none() {
            for ppn in start..end {
                let next = (ppn + 1 < end).then_some(ppn + 1);
                Self::set_recycled_next(ppn, next);
            }
            self.recycled_head = Some(start);
            self.recycled_tail = Some(end - 1);
            self.recycled_insert_hint = Some(end - 1);
            self.recycled_count = end - start;
            return;
        }

        let head = self.recycled_head.unwrap();
        let tail = self.recycled_tail.expect("recycled list missing tail");
        if end <= head {
            for ppn in start..end {
                let next = if ppn + 1 < end {
                    Some(ppn + 1)
                } else {
                    Some(head)
                };
                Self::set_recycled_next(ppn, next);
            }
            self.recycled_head = Some(start);
            self.recycled_insert_hint = Some(end - 1);
            self.recycled_count += end - start;
            return;
        }
        if start > tail {
            Self::set_recycled_next(tail, Some(start));
            for ppn in start..end {
                let next = (ppn + 1 < end).then_some(ppn + 1);
                Self::set_recycled_next(ppn, next);
            }
            self.recycled_tail = Some(end - 1);
            self.recycled_insert_hint = Some(end - 1);
            self.recycled_count += end - start;
            return;
        }

        let (mut previous, mut current) =
            if let Some(hint) = self.recycled_insert_hint.filter(|hint| *hint < start) {
                match self.recycled_next(hint) {
                    Ok(next) => (Some(hint), next),
                    Err(observed_next) => {
                        self.discard_corrupt_recycled_list(hint, observed_next);
                        self.insert_recycled_range(start, end);
                        return;
                    }
                }
            } else {
                (None, self.recycled_head)
            };
        let mut remaining = self.recycled_count;
        while let Some(ppn) = current {
            assert!(remaining > 0, "corrupt recycled frame list");
            if ppn >= start {
                break;
            }
            previous = current;
            current = match self.recycled_next(ppn) {
                Ok(next) => next,
                Err(observed_next) => {
                    self.discard_corrupt_recycled_list(ppn, observed_next);
                    self.insert_recycled_range(start, end);
                    return;
                }
            };
            remaining -= 1;
        }
        assert!(
            current.is_none_or(|ppn| ppn >= end),
            "recycled frame overlap"
        );

        for ppn in start..end {
            let next = if ppn + 1 < end {
                Some(ppn + 1)
            } else {
                current
            };
            Self::set_recycled_next(ppn, next);
        }
        if let Some(previous) = previous {
            Self::set_recycled_next(previous, Some(start));
        } else {
            self.recycled_head = Some(start);
        }
        if current.is_none() {
            self.recycled_tail = Some(end - 1);
        }
        self.recycled_insert_hint = Some(end - 1);
        self.recycled_count += end - start;
    }

    fn pop_recycled(&mut self) -> Option<usize> {
        let ppn = self.recycled_head?;
        match self.recycled_next(ppn) {
            Ok(next) => {
                self.recycled_head = next;
                self.recycled_count -= 1;
            }
            Err(observed_next) => {
                self.discard_corrupt_recycled_list(ppn, observed_next);
                return None;
            }
        }
        // A head allocation does not invalidate a hint that points at another
        // live node (normally the tail of a monotonic deallocation batch).
        // Keeping that hint makes interleaved alloc/free workloads retain
        // amortized O(1) insertion instead of rescanning the whole free list.
        if self.recycled_insert_hint == Some(ppn) {
            self.recycled_insert_hint = None;
        }
        if self.recycled_head.is_none() {
            self.recycled_tail = None;
            self.recycled_insert_hint = None;
        }
        Some(ppn)
    }

    fn align_up_ppn(ppn: usize, align_pages: usize) -> Option<usize> {
        debug_assert!(align_pages.is_power_of_two());
        ppn.checked_add(align_pages - 1)
            .map(|value| value & !(align_pages - 1))
    }

    fn alloc_fresh_contiguous(&mut self, pages: usize, align_pages: usize) -> Option<FrameExtent> {
        for range_idx in 0..self.range_count {
            let range = self.ranges[range_idx];
            let base = Self::align_up_ppn(range.current, align_pages)?;
            let end = base.checked_add(pages)?;
            if end > range.end {
                continue;
            }

            self.ranges[range_idx].current = end;
            self.insert_recycled_range(range.current, base);
            FRAME_ALLOC_COUNT.fetch_add(pages, Ordering::Relaxed);
            return Some(FrameExtent {
                start: PhysPageNum(base),
                pages,
            });
        }
        None
    }

    fn alloc_recycled_contiguous(
        &mut self,
        pages: usize,
        align_pages: usize,
    ) -> Option<FrameExtent> {
        let mut current = self.recycled_head;
        let mut previous = None;
        let mut before_run = None;
        let mut run_start = 0usize;
        let mut run_pages = 0usize;
        let mut remaining = self.recycled_count;
        while let Some(ppn) = current {
            assert!(remaining > 0, "corrupt recycled frame list");
            let next = match self.recycled_next(ppn) {
                Ok(next) => next,
                Err(observed_next) => {
                    self.discard_corrupt_recycled_list(ppn, observed_next);
                    return None;
                }
            };
            if run_pages > 0 && run_start.checked_add(run_pages) == Some(ppn) {
                run_pages += 1;
            } else if ppn % align_pages == 0 {
                run_start = ppn;
                run_pages = 1;
                before_run = previous;
            } else {
                run_pages = 0;
            }

            if run_pages == pages {
                if let Some(before_run) = before_run {
                    Self::set_recycled_next(before_run, next);
                } else {
                    self.recycled_head = next;
                }
                self.recycled_count -= pages;
                self.recycled_insert_hint = None;
                if next.is_none() {
                    self.recycled_tail = before_run;
                }
                if self.recycled_count == 0 {
                    self.recycled_head = None;
                    self.recycled_tail = None;
                }
                FRAME_ALLOC_COUNT.fetch_add(pages, Ordering::Relaxed);
                return Some(FrameExtent {
                    start: PhysPageNum(run_start),
                    pages,
                });
            }
            previous = current;
            current = next;
            remaining -= 1;
        }
        None
    }
}
impl FrameAllocator for StackFrameAllocator {
    fn new() -> Self {
        Self {
            ranges: [EMPTY_FRAME_RANGE; MEM_VECTOR_CAPACITY],
            range_count: 0,
            recycled_head: None,
            recycled_tail: None,
            recycled_insert_hint: None,
            recycled_count: 0,
        }
    }
    fn alloc(&mut self) -> Option<PhysPageNum> {
        if let Some(ppn) = self.pop_recycled() {
            // warn!("alloc recycled {:#x}", ppn);
            FRAME_ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
            Some(ppn.into())
        } else {
            for range in self.ranges_mut().iter_mut() {
                debug!("l:{:#x}, r:{:#x}", range.current, range.end);
                if range.current < range.end {
                    range.current += 1;
                    FRAME_ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
                    return Some((range.current - 1).into());
                }
            }
            None
        }
    }

    fn alloc_contiguous(&mut self, pages: usize, align_pages: usize) -> Option<FrameExtent> {
        if !align_pages.is_power_of_two() {
            return None;
        }
        if pages == 0 {
            return Some(FrameExtent {
                start: PhysPageNum(0),
                pages: 0,
            });
        }
        if pages == 1 && align_pages == 1 {
            return self.alloc().map(|start| FrameExtent { start, pages });
        }

        self.alloc_fresh_contiguous(pages, align_pages)
            .or_else(|| self.alloc_recycled_contiguous(pages, align_pages))
    }

    fn dealloc(&mut self, ppn: PhysPageNum) {
        let ppn = ppn.0;
        // validity check
        if !self.contains_ppn(ppn) || !self.allocated_ppn(ppn) {
            panic!("Frame ppn={:#x} has not been allocated!", ppn);
        }
        // recycle
        self.insert_recycled_range(ppn, ppn + 1);
        FRAME_FREE_COUNT.fetch_add(1, Ordering::Relaxed);
    }
}

type FrameAllocatorImpl = StackFrameAllocator;

lazy_static! {
    /// frame allocator instance through lazy_static!
    pub static ref FRAME_ALLOCATOR: SpinNoIrqLock<FrameAllocatorImpl> =
        SpinNoIrqLock::new(FrameAllocatorImpl::new());
}

fn lock_frame_allocator()
-> crate::sync::SpinMutexGuard<'static, FrameAllocatorImpl, crate::sync::SpinNoIrq> {
    const FRAME_LOCK_RETRY_LIMIT: usize = 0x1000000;
    let mut retries = 0usize;
    loop {
        if let Some(allocator) = FRAME_ALLOCATOR.try_lock() {
            return allocator;
        }
        let owner = FRAME_ALLOCATOR.owner_hart();
        assert_ne!(
            owner,
            polyhal::arch::hart_id(),
            "recursive frame allocator lock acquisition on hart {}",
            owner
        );
        retries += 1;
        if retries == FRAME_LOCK_RETRY_LIMIT {
            panic!(
                "FrameAllocator: deadlock detected after {:#x} retries on hart {} owner_hart={} owner_line={}",
                retries,
                polyhal::arch::hart_id(),
                FRAME_ALLOCATOR.owner_hart(),
                FRAME_ALLOCATOR.owner_line(),
            );
        }
        // A different hart may be walking recycled metadata and may itself be
        // temporarily descheduled by the host. Do not stay inside the generic
        // no-IRQ deadlock-detector loop while waiting for that bounded work.
        core::hint::spin_loop();
    }
}

fn alloc_ppn_with_reclaim() -> Option<PhysPageNum> {
    if let Some(ppn) = lock_frame_allocator().alloc() {
        return Some(ppn);
    }
    crate::mm::reclaim::try_reclaim_for_allocation(1);
    lock_frame_allocator().alloc()
}

/// initiate the frame allocator using memory regions reported by the platform
pub fn init_frame_allocator() {
    let mut allocator = FRAME_ALLOCATOR.lock();
    let mut initialized = false;
    for &(start, size) in polyhal::mem::get_mem_areas() {
        let end = start + size;
        let left = PhysAddr::from(start).ceil();
        let right = PhysAddr::from(end).floor();
        if left >= right {
            continue;
        }
        allocator.init(left, right);
        initialized = true;
        polyhal::println!("frame region {:#x} --- {:#x}", left.0, right.0);
    }
    assert!(initialized, "no usable frame allocator region");
}
/// allocate a frame
pub fn frame_alloc() -> Option<FrameTracker> {
    let ppn = alloc_ppn_with_reclaim()?;
    Some(FrameTracker::new(ppn))
}

/// Allocate physically contiguous frames.
pub fn frame_alloc_contiguous(pages: usize) -> Option<Vec<FrameTracker>> {
    let extent = if let Some(extent) = lock_frame_allocator().alloc_contiguous(pages, 1) {
        extent
    } else {
        crate::mm::reclaim::try_reclaim_for_allocation(pages);
        lock_frame_allocator().alloc_contiguous(pages, 1)?
    };
    let mut frames = Vec::with_capacity(extent.pages);
    for ppn in extent.start.0..extent.start.0 + extent.pages {
        frames.push(FrameTracker::new(PhysPageNum(ppn)));
    }
    Some(frames)
}

/// Allocate an aligned extent for the grow-only kernel heap.
///
/// This path never allocates heap metadata and never invokes reclaim, so it is
/// safe to call from the global allocator while the heap lock is held.
pub(crate) fn frame_alloc_heap_extent(
    pages: usize,
    align_pages: usize,
    min_free_pages: usize,
) -> Option<FrameExtent> {
    let mut allocator = lock_frame_allocator();
    if allocator.free_pages().saturating_sub(pages) < min_free_pages {
        return None;
    }
    allocator.alloc_contiguous(pages, align_pages)
}

///传给hal里的物理页分配器，返回物理页号
pub fn frame_alloc_hal() -> Option<PhysPageNum> {
    alloc_ppn_with_reclaim()
}

/// deallocate a frame
pub fn frame_dealloc(ppn: PhysPageNum) {
    // println!("dealloc ppn {:#x}", ppn.0);
    lock_frame_allocator().dealloc(ppn);
}

/// Get the total physical memory size in bytes
pub fn get_total_memory() -> usize {
    lock_frame_allocator().total_pages() * PAGE_SIZE
}

/// Get the free physical memory size in bytes
pub fn get_free_memory() -> usize {
    lock_frame_allocator().free_pages() * PAGE_SIZE
}
fn frame_stats_from_allocator(allocator: &FrameAllocatorImpl) -> FrameStats {
    let alloc = FRAME_ALLOC_COUNT.load(Ordering::Relaxed);
    let free = FRAME_FREE_COUNT.load(Ordering::Relaxed);
    let free_pages = allocator.free_pages();
    let total_pages = allocator.total_pages();
    FrameStats {
        alloc_count: alloc,
        free_count: free,
        allocated_delta: alloc.saturating_sub(free),
        free_pages,
        used_pages: total_pages.saturating_sub(free_pages),
        fresh_free_pages: allocator.fresh_free_pages(),
        recycled_pages: allocator.recycled_pages(),
        total_pages,
    }
}

/// Return the current physical frame allocator statistics.
pub fn frame_stats() -> FrameStats {
    let allocator = lock_frame_allocator();
    frame_stats_from_allocator(&allocator)
}

/// Try to return physical frame statistics without blocking on the allocator lock.
pub fn try_frame_stats() -> Option<FrameStats> {
    FRAME_ALLOCATOR
        .try_lock()
        .map(|allocator| frame_stats_from_allocator(&allocator))
}

/// 打印当前物理页帧分配器的统计信息（累计 alloc / free / delta）
pub fn print_frame_stats() {
    let stats = frame_stats();
    debug!(
        "[MEMDEBUG] frames: alloc={} free={} delta={} pages: used={} free={} fresh_free={} recycled={} total={} bytes: free={} total={}",
        stats.alloc_count,
        stats.free_count,
        stats.allocated_delta,
        stats.used_pages,
        stats.free_pages,
        stats.fresh_free_pages,
        stats.recycled_pages,
        stats.total_pages,
        stats.free_pages * PAGE_SIZE,
        stats.total_pages * PAGE_SIZE
    );
}

#[allow(unused)]
/// a simple test for frame allocator
pub fn frame_allocator_test() {
    let mut v: Vec<FrameTracker> = Vec::new();
    for i in 0..5 {
        let frame = frame_alloc().unwrap();
        polyhal::println!("{:#x}", frame.ppn.0);
        v.push(frame);
    }
    v.clear();
    for i in 0..5 {
        let frame = frame_alloc().unwrap();
        polyhal::println!("{:#x}", frame.ppn.0);
        v.push(frame);
    }
    drop(v);
    polyhal::println!("frame_allocator_test passed!");
}
