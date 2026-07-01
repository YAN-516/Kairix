//! Implementation of [`FrameAllocator`] which
//! controls all the frames in the operating system.
use polyhal::consts::*;
use polyhal::{print, println};
// use super::{PhysAddr, PhysPageNum};
use crate::sync::SpinNoIrqLock;
use alloc::vec::Vec;
use core::fmt::{self, Debug, Formatter};
use core::sync::atomic::{AtomicUsize, Ordering};
use lazy_static::*;
use log::{debug, error, info, warn};
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
    fn alloc_contiguous(&mut self, pages: usize) -> Option<Vec<PhysPageNum>>;
    fn dealloc(&mut self, ppn: PhysPageNum);
}
/// Contiguous physical page-number range managed by the allocator.
#[derive(Clone, Copy)]
struct FrameRange {
    start: usize,
    current: usize,
    end: usize,
}

/// Physical frame allocator backed by platform-reported memory ranges.
pub struct StackFrameAllocator {
    ranges: Vec<FrameRange>,
    recycled: Vec<usize>,
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
        self.ranges.push(FrameRange {
            start: l.0,
            current: l.0,
            end: r.0,
        });
    }

    fn contains_ppn(&self, ppn: usize) -> bool {
        self.ranges
            .iter()
            .any(|range| range.start <= ppn && ppn < range.end)
    }

    fn allocated_ppn(&self, ppn: usize) -> bool {
        self.ranges
            .iter()
            .any(|range| range.start <= ppn && ppn < range.current)
    }

    fn free_pages(&self) -> usize {
        self.fresh_free_pages() + self.recycled_pages()
    }

    fn fresh_free_pages(&self) -> usize {
        self.ranges
            .iter()
            .map(|range| range.end - range.current)
            .sum()
    }

    fn recycled_pages(&self) -> usize {
        self.recycled.len()
    }

    fn total_pages(&self) -> usize {
        self.ranges
            .iter()
            .map(|range| range.end - range.start)
            .sum()
    }
}
impl FrameAllocator for StackFrameAllocator {
    fn new() -> Self {
        Self {
            ranges: Vec::new(),
            recycled: Vec::new(),
        }
    }
    fn alloc(&mut self) -> Option<PhysPageNum> {
        if let Some(ppn) = self.recycled.pop() {
            // warn!("alloc recycled {:#x}", ppn);
            FRAME_ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
            Some(ppn.into())
        } else {
            for range in self.ranges.iter_mut() {
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

    fn alloc_contiguous(&mut self, pages: usize) -> Option<Vec<PhysPageNum>> {
        if pages == 0 {
            return Some(Vec::new());
        }
        if pages == 1 {
            return self.alloc().map(|ppn| alloc::vec![ppn]);
        }

        let mut positions = Vec::with_capacity(pages);
        'candidate: for idx in 0..self.recycled.len() {
            let base = self.recycled[idx];
            if base.checked_add(pages - 1).is_none() {
                continue;
            }
            positions.clear();
            for ppn in base..base + pages {
                let Some(pos) = self.recycled.iter().position(|&v| v == ppn) else {
                    continue 'candidate;
                };
                positions.push(pos);
            }

            positions.sort_unstable_by(|a, b| b.cmp(a));
            let mut ppns = Vec::with_capacity(pages);
            for pos in positions.iter() {
                ppns.push(self.recycled.swap_remove(*pos));
            }
            ppns.sort_unstable();
            FRAME_ALLOC_COUNT.fetch_add(pages, Ordering::Relaxed);
            return Some(ppns.into_iter().map(PhysPageNum).collect());
        }

        for range in self.ranges.iter_mut() {
            if range.current + pages <= range.end {
                let base = range.current;
                range.current += pages;
                FRAME_ALLOC_COUNT.fetch_add(pages, Ordering::Relaxed);
                return Some((base..base + pages).map(PhysPageNum).collect());
            }
        }
        None
    }

    fn dealloc(&mut self, ppn: PhysPageNum) {
        let ppn = ppn.0;
        // validity check
        if !self.contains_ppn(ppn)
            || !self.allocated_ppn(ppn)
            || self.recycled.iter().any(|&v| v == ppn)
        {
            panic!("Frame ppn={:#x} has not been allocated!", ppn);
        }
        // recycle
        self.recycled.push(ppn);
        FRAME_FREE_COUNT.fetch_add(1, Ordering::Relaxed);
    }
}

type FrameAllocatorImpl = StackFrameAllocator;

lazy_static! {
    /// frame allocator instance through lazy_static!
    pub static ref FRAME_ALLOCATOR: SpinNoIrqLock<FrameAllocatorImpl> =
        SpinNoIrqLock::new(FrameAllocatorImpl::new());
}

fn alloc_ppn_with_reclaim() -> Option<PhysPageNum> {
    if let Some(ppn) = FRAME_ALLOCATOR.lock().alloc() {
        return Some(ppn);
    }
    crate::mm::reclaim::try_reclaim_for_allocation(1);
    FRAME_ALLOCATOR.lock().alloc()
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
        println!("frame region {:#x} --- {:#x}", left.0, right.0);
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
    let ppns = if let Some(ppns) = FRAME_ALLOCATOR.lock().alloc_contiguous(pages) {
        ppns
    } else {
        crate::mm::reclaim::try_reclaim_for_allocation(pages);
        FRAME_ALLOCATOR.lock().alloc_contiguous(pages)?
    };
    Some(ppns.into_iter().map(FrameTracker::new).collect())
}

///传给hal里的物理页分配器，返回物理页号
pub fn frame_alloc_hal() -> Option<PhysPageNum> {
    alloc_ppn_with_reclaim()
}

/// deallocate a frame
pub fn frame_dealloc(ppn: PhysPageNum) {
    // println!("dealloc ppn {:#x}", ppn.0);
    FRAME_ALLOCATOR.lock().dealloc(ppn);
}

/// Get the total physical memory size in bytes
pub fn get_total_memory() -> usize {
    FRAME_ALLOCATOR.lock().total_pages() * PAGE_SIZE
}

/// Get the free physical memory size in bytes
pub fn get_free_memory() -> usize {
    FRAME_ALLOCATOR.lock().free_pages() * PAGE_SIZE
}
/// Return the current physical frame allocator statistics.
pub fn frame_stats() -> FrameStats {
    let allocator = FRAME_ALLOCATOR.lock();
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
        println!("{:#x}", frame.ppn.0);
        v.push(frame);
    }
    v.clear();
    for i in 0..5 {
        let frame = frame_alloc().unwrap();
        println!("{:#x}", frame.ppn.0);
        v.push(frame);
    }
    drop(v);
    println!("frame_allocator_test passed!");
}
