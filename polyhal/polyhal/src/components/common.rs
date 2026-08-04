use crate::PhysAddr;
use crate::utils::addr::*;
use core::panic::Location;
use lazyinit::LazyInit;
use log::warn;

/// Page Allocation trait for privoids that page allocation
pub trait PageAlloc: Sync {
    /// Allocate a physical page
    fn alloc(&self) -> Option<PhysPageNum>;
    /// Release a physical page
    fn dealloc(&self, paddr: PhysPageNum, allocation_site: &'static Location<'static>);
}

#[derive(Debug)]

/// manage a frame which has the same lifecycle as the tracker
pub struct FrameTracker {
    ///
    pub ppn: PhysPageNum,
    allocation_site: &'static Location<'static>,
}

impl FrameTracker {
    ///Create an empty `FrameTracker`
    #[track_caller]
    pub fn new(ppn: PhysPageNum) -> Self {
        // Clear the complete page with the architecture/compiler memset path.
        // The previous byte iterator is semantically correct but can turn into
        // a byte-at-a-time loop on targets without vectorization.
        let bytes_array = ppn.get_bytes_array();
        unsafe {
            core::ptr::write_bytes(bytes_array.as_mut_ptr(), 0, bytes_array.len());
        }
        Self {
            ppn,
            allocation_site: Location::caller(),
        }
    }

    /// Construct a tracker without clearing the physical page.
    ///
    /// # Safety
    ///
    /// The caller must initialize every byte before the frame is mapped,
    /// published, or otherwise made observable outside the kernel. Prefer a
    /// subsystem helper that enforces this invariant over calling this
    /// constructor directly.
    #[track_caller]
    pub unsafe fn new_uninit(ppn: PhysPageNum) -> Self {
        Self {
            ppn,
            allocation_site: Location::caller(),
        }
    }
}

// impl Debug for FrameTracker {
//     fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
//         f.write_fmt(format_args!("FrameTracker:PPN={:#x}", self.ppn.0))
//     }
// }

impl Drop for FrameTracker {
    fn drop(&mut self) {
        frame_dealloc(self.ppn, self.allocation_site);
    }
}

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
    *CPU_NUM
}

/// alloc a persistent memory page
#[inline]
#[track_caller]
pub(crate) fn frame_alloc() -> Option<FrameTracker> {
    let ppn = PAGE_ALLOC.alloc()?;
    Some(FrameTracker::new(ppn))
}

/// release a frame
#[inline]
pub(crate) fn frame_dealloc(ppn: PhysPageNum, allocation_site: &'static Location<'static>) {
    // warn!("recycle {:#x}", ppn.0);

    PAGE_ALLOC.dealloc(ppn, allocation_site)
}
