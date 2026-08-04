use core::sync::atomic::{AtomicUsize, Ordering};

use crate::utils::MutexNoIrq;

// LoongArch exposes at most ten ASID bits. Keeping one common-sized pool makes
// the ownership and retirement rules identical on both supported targets.
pub(crate) const MAX_ASIDS: usize = 1 << 10;
const WORD_BITS: usize = usize::BITS as usize;
const BITMAP_WORDS: usize = MAX_ASIDS.div_ceil(WORD_BITS);

static RESIDENT_CPU_MASKS: [AtomicUsize; MAX_ASIDS] = [const { AtomicUsize::new(0) }; MAX_ASIDS];
static ALLOCATOR: MutexNoIrq<AsidAllocator> = MutexNoIrq::new(AsidAllocator::new());

struct AsidAllocator {
    bitmap: [usize; BITMAP_WORDS],
    limit: usize,
    next: usize,
}

impl AsidAllocator {
    const fn new() -> Self {
        Self {
            bitmap: [0; BITMAP_WORDS],
            limit: 0,
            next: 1,
        }
    }

    fn initialize(&mut self) {
        if self.limit != 0 {
            return;
        }
        let bits = hardware_asid_bits().min(10);
        self.limit = if bits == 0 { 1 } else { 1usize << bits };
    }

    fn is_allocated(&self, asid: usize) -> bool {
        self.bitmap[asid / WORD_BITS] & (1usize << (asid % WORD_BITS)) != 0
    }

    fn set_allocated(&mut self, asid: usize, allocated: bool) {
        let word = &mut self.bitmap[asid / WORD_BITS];
        let bit = 1usize << (asid % WORD_BITS);
        if allocated {
            *word |= bit;
        } else {
            *word &= !bit;
        }
    }

    fn allocate(&mut self) -> usize {
        self.initialize();
        if self.limit <= 1 {
            return 0;
        }

        for _ in 1..self.limit {
            let asid = self.next;
            self.next += 1;
            if self.next == self.limit {
                self.next = 1;
            }
            if !self.is_allocated(asid) {
                self.set_allocated(asid, true);
                return asid;
            }
        }

        // ASID 0 is the architectural fallback. Page-table switches using it
        // retain the old full-flush behavior, so pool exhaustion is safe.
        0
    }
}

fn hardware_asid_bits() -> usize {
    #[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
    {
        super::hardware_asid_bits()
    }
    #[cfg(not(any(target_arch = "riscv64", target_arch = "loongarch64")))]
    {
        0
    }
}

pub(crate) fn allocate() -> usize {
    #[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
    {
        ALLOCATOR.lock().allocate()
    }
    #[cfg(not(any(target_arch = "riscv64", target_arch = "loongarch64")))]
    {
        0
    }
}

/// Remember every CPU on which translations tagged with `asid` may reside.
/// Returns true the first time this ASID is installed on the current CPU.
pub(crate) fn record_current_cpu(asid: usize) -> bool {
    if asid == 0 || asid >= MAX_ASIDS {
        return false;
    }
    let cpu = crate::arch::hart_id();
    if cpu < usize::BITS as usize {
        let bit = 1usize << cpu;
        return RESIDENT_CPU_MASKS[asid].fetch_or(bit, Ordering::AcqRel) & bit == 0;
    }
    false
}

/// Invalidate all possible old translations before making an ASID reusable.
///
/// The allocator remains locked through the synchronous shootdown, preventing
/// another page table from acquiring the ASID until every resident CPU has
/// acknowledged a full local TLB invalidation. IPI handlers never take this
/// lock, so the wait cannot deadlock with the remote invalidation path.
pub(crate) fn retire(asid: usize) {
    if asid == 0 {
        return;
    }
    assert!(asid < MAX_ASIDS, "ASID {} exceeds allocator capacity", asid);

    let mut allocator = ALLOCATOR.lock();
    assert!(allocator.is_allocated(asid), "retiring free ASID {}", asid);
    let resident_mask = RESIDENT_CPU_MASKS[asid].swap(0, Ordering::AcqRel);
    if resident_mask != 0 {
        crate::multicore::shootdown_tlb_cpus(resident_mask);
    }
    allocator.set_allocated(asid, false);
}
