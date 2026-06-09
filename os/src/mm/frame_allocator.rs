//! Implementation of [`FrameAllocator`] which
//! controls all the frames in the operating system.
use polyhal::consts::*;
use polyhal::{print, println};
// use super::{PhysAddr, PhysPageNum};
use crate::config::MEMORY_END;
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
/// an implementation for frame allocator
pub struct StackFrameAllocator {
    start: usize,
    current: usize,
    end: usize,
    recycled: Vec<usize>,
}

impl StackFrameAllocator {
    ///
    pub fn init(&mut self, l: PhysPageNum, r: PhysPageNum) {
        self.start = l.0;
        self.current = l.0;
        self.end = r.0;
        // println!("last {} Physical Frames.", self.end - self.current);
    }
}
impl FrameAllocator for StackFrameAllocator {
    fn new() -> Self {
        Self {
            start: 0,
            current: 0,
            end: 0,
            recycled: Vec::new(),
        }
    }
    fn alloc(&mut self) -> Option<PhysPageNum> {
        debug!("l:{:#x}, r:{:#x}", self.current, self.end);
        if let Some(ppn) = self.recycled.pop() {
            // warn!("alloc recycled {:#x}", ppn);
            FRAME_ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
            Some(ppn.into())
        } else if self.current == self.end {
            None
        } else {
            self.current += 1;
            FRAME_ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
            Some((self.current - 1).into())
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

        if self.current + pages > self.end {
            return None;
        }
        let base = self.current;
        self.current += pages;
        FRAME_ALLOC_COUNT.fetch_add(pages, Ordering::Relaxed);
        Some((base..base + pages).map(PhysPageNum).collect())
    }

    fn dealloc(&mut self, ppn: PhysPageNum) {
        let ppn = ppn.0;
        // validity check
        if ppn < self.start || ppn >= self.current || self.recycled.iter().any(|&v| v == ppn) {
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

/// initiate the frame allocator using `ekernel` and `MEMORY_END`
pub fn init_frame_allocator() {
    unsafe extern "C" {
        safe fn ekernel();
    }
    FRAME_ALLOCATOR.lock().init(
        PhysAddr::from(ekernel as usize - VIRT_ADDR_START).ceil(),
        PhysAddr::from(MEMORY_END).floor(),
    );
    println!(
        "left frame {:#x} --- right frame {:#x}",
        PhysAddr::from(ekernel as usize - VIRT_ADDR_START).ceil().0,
        PhysAddr::from(MEMORY_END).floor().0
    );
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
    use crate::config::MEMORY_END;
    // QEMU virt DRAM starts at 0x8000_0000
    MEMORY_END - 0x8000_0000
}

/// Get the free physical memory size in bytes
pub fn get_free_memory() -> usize {
    let allocator = FRAME_ALLOCATOR.lock();
    let free_pages = allocator.end - allocator.current + allocator.recycled.len();
    free_pages * PAGE_SIZE
}

/// 打印当前物理页帧分配器的统计信息（累计 alloc / free / delta）
pub fn print_frame_stats() {
    let alloc = FRAME_ALLOC_COUNT.load(Ordering::Relaxed);
    let free = FRAME_FREE_COUNT.load(Ordering::Relaxed);
    let free_mem = get_free_memory();
    let total_mem = get_total_memory();
    error!(
        "[MEMDEBUG] frames: alloc={} free={} delta={} | memory: free={} total={}",
        alloc,
        free,
        alloc.saturating_sub(free),
        free_mem,
        total_mem
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
