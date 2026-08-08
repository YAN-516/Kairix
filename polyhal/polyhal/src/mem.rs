use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, Ordering};

use arrayvec::ArrayVec;
use fdt_parser::{Fdt, FdtError};
use lazyinit::LazyInit;

use crate::{
    arch::{consts::VIRT_ADDR_START, MEM_VECTOR_CAPACITY},
    common::CPU_NUM,
    PhysAddr,
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
    // Keep the offset within the page.  A DTB is only required to be 8-byte
    // aligned, so converting its address to a page number would make an
    // address such as `...f480` incorrectly point at `...f000`.
    let ptr = fdt_data_ptr(dtb_ptr)?;
    let fdt = Fdt::from_ptr(ptr)?;
    let dtb_size = fdt.total_size();
    #[cfg(all(target_arch = "loongarch64", board = "2k1000"))]
    let dtb_size = dtb_size
        .checked_add(crate::arch::consts::PAGE_SIZE - 1)
        .ok_or(FdtError::BadCell)?
        & !(crate::arch::consts::PAGE_SIZE - 1);
    DTB_INFO.init_once((dtb_ptr, dtb_size));

    for mm in fdt.memory().flat_map(|x| x.regions()) {
        let base = normalize_fdt_address(mm.address as usize);
        let end = base.checked_add(mm.size).ok_or(FdtError::BadCell)?;

        #[cfg(not(target_arch = "riscv64"))]
        let start = base;
        #[cfg(target_arch = "riscv64")]
        let start = base.checked_add(0x200_000).ok_or(FdtError::BadCell)?;

        // A region smaller than the architecture-specific prefix contains no
        // memory that can be handed to the allocator.
        if start < end {
            unsafe { add_memory_region(start, end) };
        }
    }

    // The 2K1000 U-Boot control DTB describes a firmware-owned boot parameter
    // range.  Keep this board-specific so existing QEMU and other-architecture
    // memory initialization remains unchanged.
    #[cfg(all(target_arch = "loongarch64", board = "2k1000"))]
    reserve_fdt_memory(&fdt)?;
    Ok(())
}

/// Get Flattened Device Tree
pub fn get_fdt() -> Result<Fdt<'static>, FdtError<'static>> {
    let (dtb_ptr, _) = DTB_INFO.get().ok_or(FdtError::BadPtr)?;
    let ptr = fdt_data_ptr(*dtb_ptr)?;
    Fdt::from_ptr(ptr)
}

fn fdt_data_ptr(dtb_ptr: PhysAddr) -> Result<NonNull<u8>, FdtError<'static>> {
    let cached = NonNull::new(dtb_ptr.get_mut_ptr::<u8>()).ok_or(FdtError::BadPtr)?;

    #[cfg(all(target_arch = "loongarch64", board = "2k1000"))]
    {
        const FDT_MAGIC: u32 = 0xd00d_feed;
        const DMW_UNCACHED: usize = 0x8000_0000_0000_0000;

        if unsafe { read_be_u32(cached.as_ptr()) } == FDT_MAGIC {
            return Ok(cached);
        }

        // U-Boot may leave the control DTB visible only through the uncached
        // DMW alias when handing control to the kernel.  Try that alias before
        // rejecting the fixed physical address.
        let uncached_addr = DMW_UNCACHED | dtb_ptr.0;
        let uncached = NonNull::new(uncached_addr as *mut u8).ok_or(FdtError::BadPtr)?;
        if unsafe { read_be_u32(uncached.as_ptr()) } == FDT_MAGIC {
            return Ok(uncached);
        }

        Err(FdtError::BadMagic)
    }

    #[cfg(not(all(target_arch = "loongarch64", board = "2k1000")))]
    {
        Ok(cached)
    }
}

#[cfg(all(target_arch = "loongarch64", board = "2k1000"))]
unsafe fn read_be_u32(ptr: *const u8) -> u32 {
    let bytes = unsafe {
        [
            ptr.read_volatile(),
            ptr.add(1).read_volatile(),
            ptr.add(2).read_volatile(),
            ptr.add(3).read_volatile(),
        ]
    };
    u32::from_be_bytes(bytes)
}

/// Return a stable, allocation-free name for early-boot diagnostics.
#[cfg(all(target_arch = "loongarch64", board = "2k1000"))]
pub fn fdt_error_kind(error: &FdtError<'_>) -> &'static str {
    match error {
        FdtError::NotFound(_) => "NotFound",
        FdtError::BadMagic => "BadMagic",
        FdtError::BadPtr => "BadPtr",
        FdtError::BadCell => "BadCell",
        FdtError::BadCellSize(_) => "BadCellSize",
        FdtError::Eof => "Eof",
        FdtError::MissingProperty => "MissingProperty",
        FdtError::Utf8Parse { .. } => "Utf8Parse",
        FdtError::FromBytesUntilNull { .. } => "FromBytesUntilNull",
    }
}

/// Convert addresses embedded by firmware to physical addresses.
///
/// The 2K1000 U-Boot control DTB describes RAM through the cached or uncached
/// LoongArch DMW aliases.  The boot allocator, however, stores physical
/// addresses, so strip the DMW virtual-segment prefix before registering or
/// reserving a range.
#[inline]
fn normalize_fdt_address(address: usize) -> usize {
    #[cfg(all(target_arch = "loongarch64", board = "2k1000"))]
    {
        const DMW_VSEG_MASK: usize = 0xf000_0000_0000_0000;
        const DMW_UNCACHED: usize = 0x8000_0000_0000_0000;
        const DMW_CACHED: usize = 0x9000_0000_0000_0000;

        match address & DMW_VSEG_MASK {
            DMW_UNCACHED | DMW_CACHED => address & !DMW_VSEG_MASK,
            _ => address,
        }
    }

    #[cfg(not(all(target_arch = "loongarch64", board = "2k1000")))]
    {
        address
    }
}

#[cfg(all(target_arch = "loongarch64", board = "2k1000"))]
fn reserve_fdt_memory(fdt: &Fdt<'_>) -> Result<(), FdtError<'static>> {
    for region in fdt.memory_reservation_block() {
        reserve_fdt_region(region.address as usize, region.size)?;
    }

    let Some(reserved_root) = fdt.find_nodes("/reserved-memory").next() else {
        return Ok(());
    };
    let reserved_level = reserved_root.level;
    let reserved_name = reserved_root.name();
    let mut nodes = fdt.all_nodes();

    // Position the iterator immediately after the root /reserved-memory node.
    for node in nodes.by_ref() {
        if node.level == reserved_level && node.name() == reserved_name {
            break;
        }
    }

    // The specification places reserved ranges in direct child nodes.
    for node in nodes {
        if node.level <= reserved_level {
            break;
        }
        if node.level != reserved_level + 1 {
            continue;
        }
        if let Some(regions) = node.reg() {
            for region in regions {
                reserve_fdt_region(region.address as usize, region.size.unwrap_or_default())?;
            }
        }
    }

    Ok(())
}

#[cfg(all(target_arch = "loongarch64", board = "2k1000"))]
fn reserve_fdt_region(address: usize, size: usize) -> Result<(), FdtError<'static>> {
    let start = normalize_fdt_address(address);
    let end = start.checked_add(size).ok_or(FdtError::BadCell)?;
    if start < end {
        unsafe { remove_memory_region(start, end) };
    }
    Ok(())
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
            let start = normalize_fdt_address(mm.address as usize);
            display_info!(
                "Platform Memory Region",
                "{:#p} - {:#018x}",
                start as *mut u8,
                start + mm.size
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
    if start >= end {
        return;
    }
    extern "C" {
        fn _skernel();
        fn _end();
    }
    let (dtb_s, dtb_e) = DTB_INFO
        .get()
        .and_then(|(start, size)| start.0.checked_add(*size).map(|end| (start.0, end)))
        .unwrap_or((0, 0));
    let (self_s, self_e) = (
        _skernel as usize - VIRT_ADDR_START,
        _end as usize - VIRT_ADDR_START,
    );
    #[cfg(all(target_arch = "loongarch64", board = "2k1000"))]
    {
        // On 2K1000, 0x8000_0000..0x9000_0000 aliases the low 256 MiB
        // window.  The kernel is linked at 0x8000_0000 and can span from that
        // alias into the direct high-memory window, so exclude both physical
        // portions from the allocator.
        const LOW_ALIAS_START: usize = 0x8000_0000;
        const HIGH_MEMORY_START: usize = 0x9000_0000;

        let low_kernel_start = self_s.max(LOW_ALIAS_START);
        let low_kernel_end = self_e.min(HIGH_MEMORY_START);
        if low_kernel_start < low_kernel_end
            && unsafe {
                exclude_memory_region(
                    start,
                    end,
                    low_kernel_start - LOW_ALIAS_START,
                    low_kernel_end - LOW_ALIAS_START,
                )
            }
        {
            return;
        }

        let high_kernel_start = self_s.max(HIGH_MEMORY_START);
        if high_kernel_start < self_e
            && unsafe { exclude_memory_region(start, end, high_kernel_start, self_e) }
        {
            return;
        }
    }

    #[cfg(not(all(target_arch = "loongarch64", board = "2k1000")))]
    if unsafe { exclude_memory_region(start, end, self_s, self_e) } {
        return;
    }

    if unsafe { exclude_memory_region(start, end, dtb_s, dtb_e) } {
        return;
    }

    unsafe {
        MEM_AREA.push((start, end - start));
    }
}

/// Subtract one occupied interval and recursively register the remaining
/// pieces. Returns true when the interval overlaps the candidate region.
unsafe fn exclude_memory_region(
    start: usize,
    end: usize,
    occupied_start: usize,
    occupied_end: usize,
) -> bool {
    let overlap_start = start.max(occupied_start);
    let overlap_end = end.min(occupied_end);
    if overlap_start >= overlap_end {
        return false;
    }

    unsafe {
        if start < overlap_start {
            add_memory_region(start, overlap_start);
        }
        if overlap_end < end {
            add_memory_region(overlap_end, end);
        }
    }
    true
}

/// Remove a reserved physical range from the boot allocator's RAM regions.
///
/// # Safety
///
/// This has the same single-threaded boot-time requirement as
/// [`add_memory_region`].
#[cfg(all(target_arch = "loongarch64", board = "2k1000"))]
unsafe fn remove_memory_region(start: usize, end: usize) {
    if start >= end {
        return;
    }

    unsafe {
        // Iterate backwards because a fully covered area is removed in place.
        for index in (0..MEM_AREA.len()).rev() {
            let (area_start, area_size) = MEM_AREA[index];
            let Some(area_end) = area_start.checked_add(area_size) else {
                continue;
            };

            let overlap_start = area_start.max(start);
            let overlap_end = area_end.min(end);
            if overlap_start >= overlap_end {
                continue;
            }

            match (overlap_start == area_start, overlap_end == area_end) {
                (true, true) => {
                    MEM_AREA.remove(index);
                }
                (true, false) => {
                    MEM_AREA[index] = (overlap_end, area_end - overlap_end);
                }
                (false, true) => {
                    MEM_AREA[index] = (area_start, overlap_start - area_start);
                }
                (false, false) => {
                    MEM_AREA[index] = (area_start, overlap_start - area_start);
                    MEM_AREA.push((overlap_end, area_end - overlap_end));
                }
            }
        }
    }
}
