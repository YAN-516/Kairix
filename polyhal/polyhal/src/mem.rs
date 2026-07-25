use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, Ordering};

use arrayvec::ArrayVec;
use fdt_parser::{Fdt, FdtError};
use lazyinit::LazyInit;

use crate::{
    PhysAddr,
    arch::{MEM_VECTOR_CAPACITY, consts::VIRT_ADDR_START},
    common::CPU_NUM,
};

/// Memory Area
///
/// Memory Area with [MEM_VECTOR_CAPACITY].
static mut MEM_AREA: ArrayVec<(usize, usize), MEM_VECTOR_CAPACITY> = ArrayVec::new_const();

// The early allocator is shared by the boot CPU (secondary stacks) and by
// secondary CPUs (per-CPU areas).  Starting a secondary makes those paths run
// concurrently, so mutating MEM_AREA without serialization can hand out
// overlapping storage and can leave the later frame allocator with a torn
// region boundary.
static EARLY_ALLOC_LOCK: AtomicBool = AtomicBool::new(false);
static EARLY_ALLOC_FROZEN: AtomicBool = AtomicBool::new(false);

struct EarlyAllocGuard;

impl EarlyAllocGuard {
    fn lock() -> Self {
        while EARLY_ALLOC_LOCK
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        Self
    }
}

impl Drop for EarlyAllocGuard {
    fn drop(&mut self) {
        EARLY_ALLOC_LOCK.store(false, Ordering::Release);
    }
}

/// Device Tree Infomation
///
/// [DTB_INFO] is a lazy init value
static DTB_INFO: LazyInit<(PhysAddr, usize)> = LazyInit::new();

/// Allocation-free classification of a physical address against boot memory.
#[derive(Clone, Copy, Debug)]
pub struct MemoryAddressInfo {
    pub region_count: usize,
    pub containing_region: Option<(usize, usize)>,
    pub nearest_lower_end: Option<usize>,
    pub nearest_upper_start: Option<usize>,
    pub dtb_region: Option<(usize, usize)>,
}

/// Classify one physical address without taking an allocator lock.
pub fn memory_address_info(address: usize) -> MemoryAddressInfo {
    let mut region_count = 0usize;
    let mut containing_region = None;
    let mut nearest_lower_end = None;
    let mut nearest_upper_start = None;
    for &(start, size) in get_mem_areas() {
        region_count += 1;
        let Some(end) = start.checked_add(size) else {
            continue;
        };
        if start <= address && address < end {
            containing_region = Some((start, end));
        }
        if end <= address && nearest_lower_end.is_none_or(|current| end > current) {
            nearest_lower_end = Some(end);
        }
        if start > address && nearest_upper_start.is_none_or(|current| start < current) {
            nearest_upper_start = Some(start);
        }
    }
    let dtb_region = DTB_INFO
        .get()
        .and_then(|(start, size)| start.0.checked_add(*size).map(|end| (start.0, end)));
    MemoryAddressInfo {
        region_count,
        containing_region,
        nearest_lower_end,
        nearest_upper_start,
        dtb_region,
    }
}

/// Init Device Tree Binary Pointer
///
/// # Arguments
///
/// - `dtb_ptr` is the pointer to the device tree binary.
///
pub fn init_dtb_once(dtb_ptr: PhysAddr) -> Result<(), FdtError<'static>> {
    // Validate Device Tree
    let ptr = NonNull::new(dtb_ptr.floor().get_mut());
    let fdt = Fdt::from_ptr(ptr.unwrap())?;
    DTB_INFO.init_once((dtb_ptr, fdt.total_size()));
    fdt.memory()
        .flat_map(|x| x.regions())
        .for_each(|mm| unsafe {
            #[cfg(not(target_arch = "riscv64"))]
            add_memory_region(mm.address as _, mm.address as usize + mm.size);
            #[cfg(target_arch = "riscv64")]
            {
                let mut start = mm.address as _;
                let end = mm.address as usize + mm.size;

                // TODO: using dynamic to skip memory
                start += 0x200_000;

                add_memory_region(start, end);
            }
        });
    Ok(())
}

/// Get Flattened Device Tree
pub fn get_fdt() -> Result<Fdt<'static>, FdtError<'static>> {
    if !DTB_INFO.is_inited() {
        return Err(FdtError::BadPtr);
    }
    unsafe { Fdt::from_ptr(NonNull::new_unchecked(DTB_INFO.0.floor().get_mut())) }
}

/// Allocate Memory From [MEM_AREA]
///
/// # Safety
///
/// - Ensure call this function in the primary core when booting
/// - Ensure no alignment required
pub unsafe fn alloc(alloc_size: usize) -> *mut u8 {
    let _guard = EarlyAllocGuard::lock();
    assert!(
        !EARLY_ALLOC_FROZEN.load(Ordering::Acquire),
        "early physical-memory allocation after frame-allocator handoff"
    );
    unsafe {
        for (start, size) in MEM_AREA.iter_mut() {
            if *size >= alloc_size {
                let ptr = *start;
                *start += alloc_size;
                *size -= alloc_size;
                return ptr as _;
            }
        }
        unreachable!()
    }
}

/// Stop early physical-memory allocation before the remaining regions are
/// handed to the kernel frame allocator.
///
/// All secondary stacks and per-CPU areas must have been reserved first.
/// Keeping this transition explicit prevents a delayed secondary CPU from
/// allocating storage out of a range already owned by the frame allocator.
pub fn freeze_early_allocator() {
    let _guard = EarlyAllocGuard::lock();
    EARLY_ALLOC_FROZEN.store(true, Ordering::Release);
}

/// Parse Information from the device tree binary or Multiboot
///
/// Display information when booting
/// Initialize the variables and memory from device tree
#[inline]
pub fn parse_system_info() {
    display_info!();
    println!(include_str!("./banner.txt"));
    display_info!("Platform Arch", "{}", env!("HAL_ENV_ARCH"));
    if let Ok(fdt) = get_fdt() {
        display_info!("Boot HART ID", "{}", fdt.boot_cpuid_phys());
        display_info!("Boot HART Count", "{}", fdt.find_nodes("/cpus/cpu").count());
        CPU_NUM.init_once(fdt.find_nodes("/cpus/cpu").count());
        fdt.chosen().inspect(|chosen| {
            display_info!("Boot Args", "{}", chosen.bootargs().unwrap_or(""));
        });
        fdt.memory().flat_map(|x| x.regions()).for_each(|mm| {
            display_info!(
                "Platform Memory Region",
                "{:#p} - {:#018x}",
                mm.address,
                mm.address as usize + mm.size
            );
        });
    } else {
        display_info!("Boot HART Count", "{} (fallback)", 1);
        CPU_NUM.init_once(1);
    }
    get_mem_areas().for_each(|(address, size)| {
        display_info!(
            "Platform Memory Available",
            "{:#018x} - {:#018x}",
            address,
            address + size
        );
    });
}

/// Retrieves an iterator over the registered memory areas.
///
/// # Returns
///
/// An iterator yielding references to tuples `(start, end)`, where:
/// - `start` is the starting address of a memory area.
/// - `end` is the ending address of a memory area.
///
/// # Safety
///
/// - The caller must ensure that `MEM_AREA` is properly initialized before calling this function.
/// - Since this function returns an iterator over a static memory region, concurrent modification  
///   of `MEM_AREA` while iterating may lead to undefined behavior.
pub fn get_mem_areas<'a>() -> impl Iterator<Item = &'a (usize, usize)> {
    unsafe { MEM_AREA.iter() }
}

/// Adds a memory region to the memblock.
///
/// # Parameters
/// - `start` - The starting address of the memory region.
/// - `end` - The ending address of the memory region.
///
/// # Safety
///
/// - This function must be called from a single thread; concurrent access is **not** safe.
/// - The caller must ensure that [MEM_VECTOR_CAPACITY] is sufficient to accommodate the memory region,  
///   otherwise, this function may result in out-of-bounds memory access or undefined behavior.
pub unsafe fn add_memory_region(start: usize, end: usize) {
    if end - start == 0 {
        return;
    }
    extern "C" {
        fn _skernel();
        fn _end();
    }
    let (dtb_s, dtb_e) = DTB_INFO
        .get()
        .map(|x| (x.0.0, x.0.0 + x.1))
        .unwrap_or((0, 0));
    let (self_s, self_e) = (
        _skernel as usize - VIRT_ADDR_START,
        _end as usize - VIRT_ADDR_START,
    );
    unsafe {
        if start <= self_s && self_e <= end {
            if self_s - start > 0 {
                add_memory_region(start, self_s);
            }
            if end - self_e > 0 {
                add_memory_region(self_e, end);
            }
        } else if start <= dtb_s && dtb_e <= end {
            if dtb_s - start > 0 {
                add_memory_region(start, dtb_s);
            }
            if end - dtb_e > 0 {
                add_memory_region(dtb_e, end);
            }
        } else {
            MEM_AREA.push((start, end - start));
        }
    }
}
