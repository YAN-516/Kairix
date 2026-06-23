use lazyinit::LazyInit;
use crate::utils::addr::*;
use crate::PhysAddr;
use log::warn;

/// Page Allocation trait for privoids that page allocation
pub trait PageAlloc: Sync {
    /// Allocate a physical page
    fn alloc(&self) -> Option<PhysPageNum>;
    /// Release a physical page
    fn dealloc(&self, paddr: PhysPageNum);
}

#[derive(Debug)]

/// manage a frame which has the same lifecycle as the tracker
pub struct FrameTracker {
    ///
    pub ppn: PhysPageNum,
}

impl FrameTracker {
    ///Create an empty `FrameTracker`
    pub fn new(ppn: PhysPageNum) -> Self {
        // page cleaning
        let bytes_array = ppn.get_bytes_array();
        for i in bytes_array {
            *i = 0;
        }
        Self { ppn }
    }

}

impl Drop for FrameTracker {
    fn drop(&mut self) {
        frame_dealloc(self.ppn);
    }
}
// impl Debug for FrameTracker {
//     fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
//         f.write_fmt(format_args!("FrameTracker:PPN={:#x}", self.ppn.0))
//     }
// }



static PAGE_ALLOC: LazyInit<&dyn PageAlloc> = LazyInit::new();

/// Init arch with page allocator, like log crate
/// Please initialize the allocator before calling this function.
pub fn init(page_alloc: &'static dyn PageAlloc) {
    PAGE_ALLOC.init_once(page_alloc);
}

/// Store the number of cpu, this will fill up by startup function.
pub(crate) static CPU_NUM: LazyInit<usize> = LazyInit::new();

/// Get the number of cpus
pub fn get_cpu_num() -> usize {
    CPU_NUM.get().copied().unwrap_or(1)
}

/// alloc a persistent memory page
#[inline]
pub(crate) fn frame_alloc() -> Option<FrameTracker> {
    let ppn = PAGE_ALLOC.alloc()?;
    Some(FrameTracker::new(ppn))
}

/// release a frame
#[inline]
pub(crate) fn frame_dealloc(ppn: PhysPageNum) {
    // warn!("recycle {:#x}", ppn.0);

    PAGE_ALLOC.dealloc(ppn)
}
