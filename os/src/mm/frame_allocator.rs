//! Implementation of [`FrameAllocator`] which
//! controls all the frames in the operating system.
use polyhal::consts::*;
use polyhal::print;
// use super::{PhysAddr, PhysPageNum};
use crate::sync::SpinNoIrqLock;
use alloc::vec::Vec;
use core::fmt::{self, Debug, Formatter};
use core::panic::Location;
use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use lazy_static::*;
use log::{debug, error};
use polyhal::arch::MEM_VECTOR_CAPACITY;
use polyhal::common::FrameTracker;
use polyhal::utils::addr::*;

use crate::config::MAX_CPU_NUM;

static FRAME_ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static FRAME_FREE_COUNT: AtomicUsize = AtomicUsize::new(0);
static FRAME_CPU_CACHED_COUNT: AtomicUsize = AtomicUsize::new(0);
static ANON_FRAME_CACHED_COUNT: AtomicUsize = AtomicUsize::new(0);
static FRAME_STATE_PTR: AtomicUsize = AtomicUsize::new(0);
static FRAME_STATE_RANGE_COUNT: AtomicUsize = AtomicUsize::new(0);
static FRAME_STATE_RANGE_STARTS: [AtomicUsize; MEM_VECTOR_CAPACITY] =
    [const { AtomicUsize::new(0) }; MEM_VECTOR_CAPACITY];
static FRAME_STATE_RANGE_ENDS: [AtomicUsize; MEM_VECTOR_CAPACITY] =
    [const { AtomicUsize::new(0) }; MEM_VECTOR_CAPACITY];
static FRAME_STATE_RANGE_OFFSETS: [AtomicUsize; MEM_VECTOR_CAPACITY] =
    [const { AtomicUsize::new(0) }; MEM_VECTOR_CAPACITY];

const ANON_FRAME_CACHE_CAPACITY: usize = 16;
const FRAME_FREE_CACHE_CAPACITY: usize = 64;
const FRAME_FREE_CACHE_FLUSH: usize = FRAME_FREE_CACHE_CAPACITY / 2;

struct PerCpuAnonFrameCache {
    ppns: [usize; ANON_FRAME_CACHE_CAPACITY],
    len: usize,
}

impl PerCpuAnonFrameCache {
    const fn new() -> Self {
        Self {
            ppns: [0; ANON_FRAME_CACHE_CAPACITY],
            len: 0,
        }
    }

    fn pop(&mut self) -> Option<PhysPageNum> {
        self.len = self.len.checked_sub(1)?;
        Some(PhysPageNum(self.ppns[self.len]))
    }

    fn extend(&mut self, ppns: &[usize]) {
        debug_assert!(self.len + ppns.len() <= self.ppns.len());
        self.ppns[self.len..self.len + ppns.len()].copy_from_slice(ppns);
        self.len += ppns.len();
    }
}

#[repr(align(64))]
struct PerCpuAnonFrameCacheCell {
    inner: SpinNoIrqLock<PerCpuAnonFrameCache>,
}

impl PerCpuAnonFrameCacheCell {
    const fn new() -> Self {
        Self {
            inner: SpinNoIrqLock::new(PerCpuAnonFrameCache::new()),
        }
    }
}

static ANON_FRAME_CPU_CACHES: [PerCpuAnonFrameCacheCell; MAX_CPU_NUM] =
    [const { PerCpuAnonFrameCacheCell::new() }; MAX_CPU_NUM];

struct PerCpuFrameFreeCache {
    ppns: [usize; FRAME_FREE_CACHE_CAPACITY],
    allocation_sites: [Option<&'static Location<'static>>; FRAME_FREE_CACHE_CAPACITY],
    len: usize,
}

impl PerCpuFrameFreeCache {
    const fn new() -> Self {
        Self {
            ppns: [0; FRAME_FREE_CACHE_CAPACITY],
            allocation_sites: [None; FRAME_FREE_CACHE_CAPACITY],
            len: 0,
        }
    }

    fn push(&mut self, ppn: PhysPageNum, allocation_site: &'static Location<'static>) {
        debug_assert!(self.len < FRAME_FREE_CACHE_CAPACITY);
        self.ppns[self.len] = ppn.0;
        self.allocation_sites[self.len] = Some(allocation_site);
        self.len += 1;
    }

    fn pop(&mut self) -> Option<(PhysPageNum, &'static Location<'static>)> {
        self.len = self.len.checked_sub(1)?;
        let allocation_site = self.allocation_sites[self.len]
            .take()
            .expect("cached frame missing allocation site");
        Some((PhysPageNum(self.ppns[self.len]), allocation_site))
    }
}

#[repr(align(64))]
struct PerCpuFrameFreeCacheCell {
    inner: SpinNoIrqLock<PerCpuFrameFreeCache>,
}

impl PerCpuFrameFreeCacheCell {
    const fn new() -> Self {
        Self {
            inner: SpinNoIrqLock::new(PerCpuFrameFreeCache::new()),
        }
    }
}

static FRAME_FREE_CPU_CACHES: [PerCpuFrameFreeCacheCell; MAX_CPU_NUM] =
    [const { PerCpuFrameFreeCacheCell::new() }; MAX_CPU_NUM];

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
    /// Freed pages available through per-CPU caches or the central buddy lists.
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
    fn dealloc(&mut self, ppn: PhysPageNum, allocation_site: &'static Location<'static>);
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
    metadata_start: usize,
}

const EMPTY_FRAME_RANGE: FrameRange = FrameRange {
    start: 0,
    current: 0,
    end: 0,
    metadata_start: 0,
};
const BUDDY_LIST_END: usize = usize::MAX;
const BUDDY_LINK_COOKIE: usize = 0xd6e8_feb8_6659_fd93;
const FRAME_BUDDY_ORDERS: usize = 32;
const FRAME_STATE_ALLOCATED: u8 = u8::MAX;
const FRAME_STATE_FREE_INTERIOR: u8 = u8::MAX - 1;
const FRAME_STATE_CPU_CACHED: u8 = u8::MAX - 2;

/// Physical frame allocator backed by platform-reported memory ranges.
pub struct BuddyFrameAllocator {
    ranges: [FrameRange; MEM_VECTOR_CAPACITY],
    range_count: usize,
    buddy_heads: [usize; FRAME_BUDDY_ORDERS],
    frame_states: Vec<AtomicU8>,
    recycled_count: usize,
}

impl BuddyFrameAllocator {
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
        let metadata_start = self.frame_states.len();
        self.frame_states
            .extend((0..(r.0 - l.0)).map(|_| AtomicU8::new(FRAME_STATE_ALLOCATED)));
        self.ranges[self.range_count] = FrameRange {
            start: l.0,
            current: l.0,
            end: r.0,
            metadata_start,
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

    fn range_for_ppn(&self, ppn: usize) -> Option<&FrameRange> {
        self.ranges()
            .iter()
            .find(|range| range.start <= ppn && ppn < range.end)
    }

    fn frame_state_index(&self, ppn: usize) -> Option<usize> {
        self.range_for_ppn(ppn)
            .map(|range| range.metadata_start + ppn - range.start)
    }

    fn frame_state(&self, ppn: usize) -> Option<u8> {
        self.frame_state_index(ppn)
            .map(|index| self.frame_states[index].load(Ordering::Acquire))
    }

    fn set_frame_state(&mut self, ppn: usize, state: u8) {
        let index = self
            .frame_state_index(ppn)
            .expect("frame state outside managed memory");
        self.frame_states[index].store(state, Ordering::Release);
    }

    fn fill_frame_states(&mut self, start: usize, pages: usize, state: u8) {
        if pages == 0 {
            return;
        }
        let range = *self
            .range_for_ppn(start)
            .expect("frame state range outside managed memory");
        let end = start
            .checked_add(pages)
            .expect("frame state range overflow");
        assert!(end <= range.end, "frame state range crosses memory region");
        let metadata_start = range.metadata_start + start - range.start;
        for frame_state in &self.frame_states[metadata_start..metadata_start + pages] {
            frame_state.store(state, Ordering::Release);
        }
    }

    fn buddy_link_ptr(ppn: usize) -> *mut usize {
        ((ppn << PAGE_SIZE_BITS) + VIRT_ADDR_START) as *mut usize
    }

    fn buddy_link_checksum(ppn: usize, previous: usize, next: usize, order: usize) -> usize {
        previous.rotate_left(11)
            ^ next.rotate_left(23)
            ^ ppn.rotate_left(7)
            ^ order.rotate_left(3)
            ^ BUDDY_LINK_COOKIE
    }

    fn write_buddy_node(&self, ppn: usize, previous: usize, next: usize, order: usize) {
        unsafe {
            let link = Self::buddy_link_ptr(ppn);
            link.write(previous);
            link.add(1).write(next);
            link.add(2).write(order);
            link.add(3)
                .write(Self::buddy_link_checksum(ppn, previous, next, order));
        }
    }

    fn read_buddy_node(&self, ppn: usize, expected_order: usize) -> (usize, usize) {
        let link = Self::buddy_link_ptr(ppn);
        let (previous, next, order, checksum) = unsafe {
            (
                link.read(),
                link.add(1).read(),
                link.add(2).read(),
                link.add(3).read(),
            )
        };
        let expected_checksum = Self::buddy_link_checksum(ppn, previous, next, order);
        let previous_valid = previous == BUDDY_LIST_END
            || (self.block_within_allocated_range(previous, expected_order)
                && self.frame_state(previous) == Some(expected_order as u8));
        let next_valid = next == BUDDY_LIST_END
            || (self.block_within_allocated_range(next, expected_order)
                && self.frame_state(next) == Some(expected_order as u8));
        if order != expected_order
            || checksum != expected_checksum
            || self.frame_state(ppn) != Some(expected_order as u8)
            || !previous_valid
            || !next_valid
        {
            error!(
                "[FRAME_BUDDY_CORRUPTION] ppn={:#x} expected_order={} observed_order={} previous={:#x} next={:#x} checksum={:#x} expected_checksum={:#x} state={:?}",
                ppn,
                expected_order,
                order,
                previous,
                next,
                checksum,
                expected_checksum,
                self.frame_state(ppn),
            );
            panic!("corrupt frame buddy node");
        }
        (previous, next)
    }

    fn buddy_push(&mut self, ppn: usize, order: usize) {
        debug_assert!(order < FRAME_BUDDY_ORDERS);
        let head = self.buddy_heads[order];
        self.set_frame_state(ppn, order as u8);
        self.write_buddy_node(ppn, BUDDY_LIST_END, head, order);
        if head != BUDDY_LIST_END {
            let (_, next) = self.read_buddy_node(head, order);
            self.write_buddy_node(head, ppn, next, order);
        }
        self.buddy_heads[order] = ppn;
        self.recycled_count += 1usize << order;
    }

    fn buddy_remove(&mut self, ppn: usize, order: usize) {
        let (previous, next) = self.read_buddy_node(ppn, order);
        if previous == BUDDY_LIST_END {
            assert_eq!(self.buddy_heads[order], ppn, "buddy head mismatch");
            self.buddy_heads[order] = next;
        } else {
            let (previous_previous, previous_next) = self.read_buddy_node(previous, order);
            assert_eq!(previous_next, ppn, "buddy previous link mismatch");
            self.write_buddy_node(previous, previous_previous, next, order);
        }
        if next != BUDDY_LIST_END {
            let (next_previous, next_next) = self.read_buddy_node(next, order);
            assert_eq!(next_previous, ppn, "buddy next link mismatch");
            self.write_buddy_node(next, previous, next_next, order);
        }
        self.set_frame_state(ppn, FRAME_STATE_FREE_INTERIOR);
        self.recycled_count -= 1usize << order;
    }

    fn block_within_allocated_range(&self, ppn: usize, order: usize) -> bool {
        let Some(range) = self.range_for_ppn(ppn) else {
            return false;
        };
        let pages = 1usize << order;
        ppn >= range.start
            && ppn % pages == 0
            && ppn
                .checked_add(pages)
                .is_some_and(|end| end <= range.current)
    }

    fn insert_buddy_block(&mut self, mut ppn: usize, mut order: usize) {
        while order + 1 < FRAME_BUDDY_ORDERS {
            let buddy = ppn ^ (1usize << order);
            if !self.block_within_allocated_range(buddy, order)
                || self.frame_state(buddy) != Some(order as u8)
            {
                break;
            }
            self.buddy_remove(buddy, order);
            ppn = ppn.min(buddy);
            order += 1;
        }
        self.buddy_push(ppn, order);
    }

    fn insert_recycled_range(&mut self, start: usize, end: usize) {
        if start >= end {
            return;
        }
        self.fill_frame_states(start, end - start, FRAME_STATE_FREE_INTERIOR);
        let mut current = start;
        while current < end {
            let remaining_order =
                usize::BITS as usize - 1 - (end - current).leading_zeros() as usize;
            let alignment_order = if current == 0 {
                FRAME_BUDDY_ORDERS - 1
            } else {
                current.trailing_zeros() as usize
            };
            let order = remaining_order
                .min(alignment_order)
                .min(FRAME_BUDDY_ORDERS - 1);
            self.insert_buddy_block(current, order);
            current += 1usize << order;
        }
    }

    fn dealloc_buddy(&mut self, ppn: usize) {
        if !self.contains_ppn(ppn) || !self.allocated_ppn(ppn) {
            panic!("Frame ppn={:#x} has not been allocated!", ppn);
        }
        assert!(
            matches!(
                self.frame_state(ppn),
                Some(FRAME_STATE_ALLOCATED | FRAME_STATE_CPU_CACHED)
            ),
            "Frame ppn={:#x} has already been freed (state={:?})",
            ppn,
            self.frame_state(ppn),
        );
        self.set_frame_state(ppn, FRAME_STATE_FREE_INTERIOR);
        self.insert_buddy_block(ppn, 0);
    }

    fn alloc_buddy_power_of_two(&mut self, target_order: usize) -> Option<usize> {
        let source_order = (target_order..FRAME_BUDDY_ORDERS)
            .find(|&order| self.buddy_heads[order] != BUDDY_LIST_END)?;
        let ppn = self.buddy_heads[source_order];
        self.buddy_remove(ppn, source_order);

        let mut order = source_order;
        while order > target_order {
            order -= 1;
            self.insert_buddy_block(ppn + (1usize << order), order);
        }
        self.fill_frame_states(ppn, 1usize << target_order, FRAME_STATE_ALLOCATED);
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
        let block_pages = pages.checked_next_power_of_two()?.max(align_pages);
        let order = block_pages.trailing_zeros() as usize;
        if order >= FRAME_BUDDY_ORDERS {
            return None;
        }
        let start = self.alloc_buddy_power_of_two(order)?;
        self.insert_recycled_range(start + pages, start + block_pages);
        Some(FrameExtent {
            start: PhysPageNum(start),
            pages,
        })
    }
}
impl FrameAllocator for BuddyFrameAllocator {
    fn new() -> Self {
        Self {
            ranges: [EMPTY_FRAME_RANGE; MEM_VECTOR_CAPACITY],
            range_count: 0,
            buddy_heads: [BUDDY_LIST_END; FRAME_BUDDY_ORDERS],
            frame_states: Vec::new(),
            recycled_count: 0,
        }
    }
    fn alloc(&mut self) -> Option<PhysPageNum> {
        if let Some(ppn) = self.alloc_buddy_power_of_two(0) {
            return Some(ppn.into());
        }
        for range in self.ranges_mut().iter_mut() {
            debug!("l:{:#x}, r:{:#x}", range.current, range.end);
            if range.current < range.end {
                range.current += 1;
                return Some((range.current - 1).into());
            }
        }
        None
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

    fn dealloc(&mut self, ppn: PhysPageNum, _allocation_site: &'static Location<'static>) {
        self.dealloc_buddy(ppn.0);
    }
}

type FrameAllocatorImpl = BuddyFrameAllocator;

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

#[inline]
fn frame_cpu_index() -> usize {
    polyhal::arch::hart_id().min(MAX_CPU_NUM - 1)
}

fn publish_frame_state_index(allocator: &FrameAllocatorImpl) {
    assert_eq!(
        FRAME_STATE_PTR.load(Ordering::Acquire),
        0,
        "frame state index published twice"
    );
    for (index, range) in allocator.ranges().iter().enumerate() {
        FRAME_STATE_RANGE_STARTS[index].store(range.start, Ordering::Relaxed);
        FRAME_STATE_RANGE_ENDS[index].store(range.end, Ordering::Relaxed);
        FRAME_STATE_RANGE_OFFSETS[index].store(range.metadata_start, Ordering::Relaxed);
    }
    FRAME_STATE_PTR.store(allocator.frame_states.as_ptr() as usize, Ordering::Release);
    FRAME_STATE_RANGE_COUNT.store(allocator.range_count, Ordering::Release);
}

fn lock_free_frame_state(ppn: usize) -> Option<&'static AtomicU8> {
    let range_count = FRAME_STATE_RANGE_COUNT.load(Ordering::Acquire);
    let state_ptr = FRAME_STATE_PTR.load(Ordering::Acquire) as *const AtomicU8;
    if state_ptr.is_null() {
        return None;
    }
    for index in 0..range_count {
        let start = FRAME_STATE_RANGE_STARTS[index].load(Ordering::Relaxed);
        let end = FRAME_STATE_RANGE_ENDS[index].load(Ordering::Relaxed);
        if start <= ppn && ppn < end {
            let offset = FRAME_STATE_RANGE_OFFSETS[index].load(Ordering::Relaxed) + ppn - start;
            return Some(unsafe { &*state_ptr.add(offset) });
        }
    }
    None
}

fn pop_cpu_free_frame() -> Option<PhysPageNum> {
    let mut cache = FRAME_FREE_CPU_CACHES[frame_cpu_index()].inner.lock();
    let (ppn, _) = cache.pop()?;
    let state = lock_free_frame_state(ppn.0).expect("cached frame outside managed memory");
    assert_eq!(
        state.compare_exchange(
            FRAME_STATE_CPU_CACHED,
            FRAME_STATE_ALLOCATED,
            Ordering::AcqRel,
            Ordering::Acquire,
        ),
        Ok(FRAME_STATE_CPU_CACHED),
        "cached frame state changed before allocation: ppn={:#x}",
        ppn.0,
    );
    FRAME_CPU_CACHED_COUNT.fetch_sub(1, Ordering::Relaxed);
    Some(ppn)
}

fn flush_cached_frames(ppns: &[usize], allocation_sites: &[Option<&'static Location<'static>>]) {
    if ppns.is_empty() {
        return;
    }
    let mut allocator = lock_frame_allocator();
    for (&ppn, &allocation_site) in ppns.iter().zip(allocation_sites) {
        allocator.dealloc(
            PhysPageNum(ppn),
            allocation_site.expect("cached frame missing allocation site"),
        );
    }
}

fn cache_deallocated_frame(ppn: PhysPageNum, allocation_site: &'static Location<'static>) {
    let state = lock_free_frame_state(ppn.0).expect("Frame outside managed memory");
    assert_eq!(
        state.compare_exchange(
            FRAME_STATE_ALLOCATED,
            FRAME_STATE_CPU_CACHED,
            Ordering::AcqRel,
            Ordering::Acquire,
        ),
        Ok(FRAME_STATE_ALLOCATED),
        "Frame ppn={:#x} has already been freed",
        ppn.0,
    );
    let mut flushed_ppns = [0usize; FRAME_FREE_CACHE_FLUSH];
    let mut flushed_sites = [None; FRAME_FREE_CACHE_FLUSH];
    let flushed_len = {
        let mut cache = FRAME_FREE_CPU_CACHES[frame_cpu_index()].inner.lock();
        let mut count = 0;
        if cache.len == FRAME_FREE_CACHE_CAPACITY {
            while count < FRAME_FREE_CACHE_FLUSH {
                let (cached_ppn, cached_site) = cache.pop().expect("full frame cache underflow");
                flushed_ppns[count] = cached_ppn.0;
                flushed_sites[count] = Some(cached_site);
                count += 1;
            }
        }
        cache.push(ppn, allocation_site);
        FRAME_CPU_CACHED_COUNT.fetch_add(1, Ordering::Relaxed);
        if count != 0 {
            FRAME_CPU_CACHED_COUNT.fetch_sub(count, Ordering::Relaxed);
        }
        count
    };
    if flushed_len != 0 {
        flush_cached_frames(&flushed_ppns[..flushed_len], &flushed_sites[..flushed_len]);
    }
}

fn drain_free_frame_caches() {
    for cache in &FRAME_FREE_CPU_CACHES {
        let mut drained_ppns = [0usize; FRAME_FREE_CACHE_CAPACITY];
        let mut drained_sites = [None; FRAME_FREE_CACHE_CAPACITY];
        let drained_len = {
            let mut cache = cache.inner.lock();
            let mut count = 0;
            while let Some((ppn, allocation_site)) = cache.pop() {
                drained_ppns[count] = ppn.0;
                drained_sites[count] = Some(allocation_site);
                count += 1;
            }
            if count != 0 {
                FRAME_CPU_CACHED_COUNT.fetch_sub(count, Ordering::Relaxed);
            }
            count
        };
        if drained_len != 0 {
            flush_cached_frames(&drained_ppns[..drained_len], &drained_sites[..drained_len]);
        }
    }
}

fn alloc_ppn_once() -> Option<PhysPageNum> {
    pop_cpu_free_frame().or_else(|| lock_frame_allocator().alloc())
}

fn alloc_ppn_with_reclaim() -> Option<PhysPageNum> {
    let ppn = if let Some(ppn) = alloc_ppn_once() {
        ppn
    } else {
        drain_free_frame_caches();
        drain_anon_frame_caches();
        if let Some(ppn) = alloc_ppn_once() {
            ppn
        } else {
            crate::mm::reclaim::try_reclaim_for_allocation(1);
            alloc_ppn_once()?
        }
    };
    FRAME_ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
    Some(ppn)
}

#[inline]
fn anon_frame_cpu_index() -> usize {
    frame_cpu_index()
}

fn alloc_profiled_ppn_once() -> Option<PhysPageNum> {
    if let Some(ppn) = pop_cpu_free_frame() {
        return Some(ppn);
    }
    let cache = &ANON_FRAME_CPU_CACHES[anon_frame_cpu_index()].inner;
    {
        let mut cache = cache.lock();
        if let Some(ppn) = cache.pop() {
            ANON_FRAME_CACHED_COUNT.fetch_sub(1, Ordering::Relaxed);
            return Some(ppn);
        }
    }

    // Reserve a bounded batch with one global-lock acquisition. The pages are
    // not cleared until they leave this CPU-local cache, so no stale contents
    // can become observable and the normal FrameTracker zeroing invariant is
    // unchanged. At most (capacity - 1) pages per CPU remain reserved.
    let mut refill = [0usize; ANON_FRAME_CACHE_CAPACITY];
    let refill_len = {
        let mut allocator = lock_frame_allocator();
        let mut count = 0;
        while count < refill.len() {
            let Some(ppn) = allocator.alloc() else {
                break;
            };
            refill[count] = ppn.0;
            count += 1;
        }
        count
    };
    if refill_len == 0 {
        return None;
    }

    let result = PhysPageNum(refill[refill_len - 1]);
    if refill_len > 1 {
        let mut cache = cache.lock();
        cache.extend(&refill[..refill_len - 1]);
        ANON_FRAME_CACHED_COUNT.fetch_add(refill_len - 1, Ordering::Relaxed);
    }
    Some(result)
}

#[track_caller]
fn drain_anon_frame_caches() {
    let allocation_site = Location::caller();
    for cache in &ANON_FRAME_CPU_CACHES {
        let mut drained = [0usize; ANON_FRAME_CACHE_CAPACITY];
        let drained_len = {
            let mut cache = cache.inner.lock();
            let mut count = 0;
            while let Some(ppn) = cache.pop() {
                drained[count] = ppn.0;
                count += 1;
            }
            if count != 0 {
                ANON_FRAME_CACHED_COUNT.fetch_sub(count, Ordering::Relaxed);
            }
            count
        };
        if drained_len == 0 {
            continue;
        }
        let mut allocator = lock_frame_allocator();
        for &ppn in &drained[..drained_len] {
            allocator.dealloc(PhysPageNum(ppn), allocation_site);
        }
    }
}

fn alloc_profiled_ppn_with_reclaim() -> Option<PhysPageNum> {
    let ppn = if let Some(ppn) = alloc_profiled_ppn_once() {
        ppn
    } else {
        // CPU caches are contention optimizations, never memory reservation
        // guarantees. Return every idle page before reclaim or OOM handling.
        drain_free_frame_caches();
        drain_anon_frame_caches();
        if let Some(ppn) = alloc_profiled_ppn_once() {
            ppn
        } else {
            crate::mm::reclaim::try_reclaim_for_allocation(1);
            alloc_profiled_ppn_once()?
        }
    };
    FRAME_ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
    Some(ppn)
}

/// initiate the frame allocator using memory regions reported by the platform
pub fn init_frame_allocator() {
    // Secondary stacks and per-CPU storage are reserved by polyhal before the
    // kernel starts. From this point onward the remaining regions have one
    // owner: FRAME_ALLOCATOR. A late early-allocation would otherwise overlap
    // live frames and silently corrupt page-cache FrameTrackers.
    polyhal::mem::freeze_early_allocator();
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
    publish_frame_state_index(&allocator);
}
/// allocate a frame
#[track_caller]
pub fn frame_alloc() -> Option<FrameTracker> {
    let ppn = alloc_ppn_with_reclaim()?;
    Some(FrameTracker::new(ppn))
}

/// Allocate a frame and initialize it from a byte prefix, clearing the tail.
/// The frame cannot escape before every byte is initialized, so full-page COW
/// and private-file copies avoid the redundant zero-before-overwrite pass
/// while partial final pages retain the normal zero-tail semantics.
#[track_caller]
pub(crate) fn frame_alloc_copy_from(source: &[u8]) -> Option<FrameTracker> {
    assert!(source.len() <= PAGE_SIZE, "frame copy exceeds one page");
    let ppn = alloc_ppn_with_reclaim()?;
    let frame = unsafe { FrameTracker::new_uninit(ppn) };
    let destination = frame.ppn.get_bytes_array();
    destination[..source.len()].copy_from_slice(source);
    if source.len() < destination.len() {
        destination[source.len()..].fill(0);
    }
    Some(frame)
}

/// One frame allocation split into allocator/reclaim work and mandatory page
/// clearing performed by `FrameTracker::new`. This preserves the normal zeroed
/// frame invariant while allowing anonymous-fault statistics to attribute the
/// two costs independently.
pub(crate) struct ProfiledFrameAlloc {
    pub(crate) frame: FrameTracker,
    pub(crate) alloc_ns: usize,
    pub(crate) zero_ns: usize,
}

#[track_caller]
pub(crate) fn frame_alloc_profiled() -> Option<ProfiledFrameAlloc> {
    let alloc_started_ns = polyhal::timer::current_time().as_nanos() as usize;
    let ppn = alloc_profiled_ppn_with_reclaim()?;
    let alloc_ns =
        (polyhal::timer::current_time().as_nanos() as usize).saturating_sub(alloc_started_ns);
    let zero_started_ns = polyhal::timer::current_time().as_nanos() as usize;
    let frame = FrameTracker::new(ppn);
    let zero_ns =
        (polyhal::timer::current_time().as_nanos() as usize).saturating_sub(zero_started_ns);
    Some(ProfiledFrameAlloc {
        frame,
        alloc_ns,
        zero_ns,
    })
}

/// Allocate physically contiguous frames.
#[track_caller]
pub fn frame_alloc_contiguous(pages: usize) -> Option<Vec<FrameTracker>> {
    let extent = if let Some(extent) = lock_frame_allocator().alloc_contiguous(pages, 1) {
        extent
    } else {
        drain_free_frame_caches();
        drain_anon_frame_caches();
        if let Some(extent) = lock_frame_allocator().alloc_contiguous(pages, 1) {
            extent
        } else {
            crate::mm::reclaim::try_reclaim_for_allocation(pages);
            lock_frame_allocator().alloc_contiguous(pages, 1)?
        }
    };
    FRAME_ALLOC_COUNT.fetch_add(extent.pages, Ordering::Relaxed);
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
    let cached_pages = FRAME_CPU_CACHED_COUNT
        .load(Ordering::Relaxed)
        .saturating_add(ANON_FRAME_CACHED_COUNT.load(Ordering::Relaxed));
    let extent = {
        let mut allocator = lock_frame_allocator();
        if allocator
            .free_pages()
            .saturating_add(cached_pages)
            .saturating_sub(pages)
            < min_free_pages
        {
            return None;
        }
        allocator.alloc_contiguous(pages, align_pages)
    };
    let extent = if let Some(extent) = extent {
        extent
    } else {
        drain_free_frame_caches();
        drain_anon_frame_caches();
        let mut allocator = lock_frame_allocator();
        if allocator.free_pages().saturating_sub(pages) < min_free_pages {
            return None;
        }
        allocator.alloc_contiguous(pages, align_pages)?
    };
    FRAME_ALLOC_COUNT.fetch_add(extent.pages, Ordering::Relaxed);
    Some(extent)
}

///传给hal里的物理页分配器，返回物理页号
pub fn frame_alloc_hal() -> Option<PhysPageNum> {
    alloc_ppn_with_reclaim()
}

/// deallocate a frame
#[track_caller]
pub fn frame_dealloc(ppn: PhysPageNum) {
    frame_dealloc_with_site(ppn, Location::caller());
}

/// Deallocate a tracked frame while retaining its original allocation site
/// for use-after-free diagnostics.
pub fn frame_dealloc_with_site(ppn: PhysPageNum, allocation_site: &'static Location<'static>) {
    cache_deallocated_frame(ppn, allocation_site);
    FRAME_FREE_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Get the total physical memory size in bytes
pub fn get_total_memory() -> usize {
    lock_frame_allocator().total_pages() * PAGE_SIZE
}

/// Get the free physical memory size in bytes
pub fn get_free_memory() -> usize {
    let cached_pages = FRAME_CPU_CACHED_COUNT
        .load(Ordering::Relaxed)
        .saturating_add(ANON_FRAME_CACHED_COUNT.load(Ordering::Relaxed));
    lock_frame_allocator()
        .free_pages()
        .saturating_add(cached_pages)
        * PAGE_SIZE
}
fn frame_stats_from_allocator(allocator: &FrameAllocatorImpl) -> FrameStats {
    let alloc = FRAME_ALLOC_COUNT.load(Ordering::Relaxed);
    let free = FRAME_FREE_COUNT.load(Ordering::Relaxed);
    let cached_pages = FRAME_CPU_CACHED_COUNT
        .load(Ordering::Relaxed)
        .saturating_add(ANON_FRAME_CACHED_COUNT.load(Ordering::Relaxed));
    let free_pages = allocator.free_pages().saturating_add(cached_pages);
    let total_pages = allocator.total_pages();
    FrameStats {
        alloc_count: alloc,
        free_count: free,
        allocated_delta: alloc.saturating_sub(free),
        free_pages,
        used_pages: total_pages.saturating_sub(free_pages),
        fresh_free_pages: allocator.fresh_free_pages(),
        recycled_pages: allocator.recycled_pages().saturating_add(cached_pages),
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
