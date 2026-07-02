use core::ptr::NonNull;

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

/// Device Tree Infomation
///
/// [DTB_INFO] is a lazy init value
static DTB_INFO: LazyInit<(PhysAddr, usize)> = LazyInit::new();

/// Init Device Tree Binary Pointer
///
/// # Arguments
///
/// - `dtb_ptr` is the pointer to the device tree binary.
///
pub fn init_dtb_once(dtb_ptr: PhysAddr) -> Result<(), FdtError<'static>> {
    // Validate Device Tree
    let ptr = NonNull::new(dtb_ptr.get_mut_ptr::<u8>()).ok_or(FdtError::BadPtr)?;
    let total_size = early_validate_fdt(ptr.as_ptr())?;
    DTB_INFO.init_once((dtb_ptr, total_size));
    early_init_memory_from_fdt(ptr.as_ptr(), total_size)?;
    Ok(())
}

/// Get Flattened Device Tree
pub fn get_fdt() -> Result<Fdt<'static>, FdtError<'static>> {
    if !DTB_INFO.is_inited() {
        return Err(FdtError::BadPtr);
    }
    unsafe { Fdt::from_ptr(NonNull::new_unchecked(DTB_INFO.0.get_mut_ptr::<u8>())) }
}

const FDT_MAGIC: u32 = 0xd00d_feed;
const FDT_BEGIN_NODE: u32 = 1;
const FDT_END_NODE: u32 = 2;
const FDT_PROP: u32 = 3;
const FDT_NOP: u32 = 4;
const FDT_END: u32 = 9;

#[inline]
fn align4(value: usize) -> usize {
    (value + 3) & !3
}

#[inline]
fn early_read_u8(base: *const u8, offset: usize) -> u8 {
    unsafe { base.add(offset).read_volatile() }
}

fn early_read_be_u32(
    base: *const u8,
    total_size: usize,
    offset: usize,
) -> Result<u32, FdtError<'static>> {
    if offset.checked_add(4).map_or(true, |end| end > total_size) {
        return Err(FdtError::Eof);
    }
    Ok(u32::from_be_bytes([
        early_read_u8(base, offset),
        early_read_u8(base, offset + 1),
        early_read_u8(base, offset + 2),
        early_read_u8(base, offset + 3),
    ]))
}

fn early_read_cells(
    base: *const u8,
    total_size: usize,
    offset: usize,
    cells: usize,
) -> Result<usize, FdtError<'static>> {
    match cells {
        0 => Ok(0),
        1 => Ok(early_read_be_u32(base, total_size, offset)? as usize),
        2 => {
            let hi = early_read_be_u32(base, total_size, offset)? as usize;
            let lo = early_read_be_u32(base, total_size, offset + 4)? as usize;
            Ok((hi << 32) | lo)
        }
        n => Err(FdtError::BadCellSize(n)),
    }
}

fn early_validate_fdt(base: *const u8) -> Result<usize, FdtError<'static>> {
    let magic = early_read_be_u32(base, 40, 0)?;
    if magic != FDT_MAGIC {
        return Err(FdtError::BadMagic);
    }
    let total_size = early_read_be_u32(base, 40, 4)? as usize;
    if total_size < 40 {
        return Err(FdtError::Eof);
    }
    Ok(total_size)
}

fn early_cstr_eq(base: *const u8, start: usize, limit: usize, expected: &[u8]) -> bool {
    if start + expected.len() >= limit {
        return false;
    }
    for (index, byte) in expected.iter().enumerate() {
        if early_read_u8(base, start + index) != *byte {
            return false;
        }
    }
    early_read_u8(base, start + expected.len()) == 0
}

fn early_cstr_starts_with(base: *const u8, start: usize, limit: usize, prefix: &[u8]) -> bool {
    if start + prefix.len() > limit {
        return false;
    }
    for (index, byte) in prefix.iter().enumerate() {
        if early_read_u8(base, start + index) != *byte {
            return false;
        }
    }
    true
}

fn early_prop_name_eq(
    base: *const u8,
    strings_start: usize,
    strings_size: usize,
    name_offset: usize,
    expected: &[u8],
) -> bool {
    if name_offset >= strings_size {
        return false;
    }
    early_cstr_eq(
        base,
        strings_start + name_offset,
        strings_start + strings_size,
        expected,
    )
}

fn early_init_memory_from_fdt(base: *const u8, total_size: usize) -> Result<(), FdtError<'static>> {
    let struct_start = early_read_be_u32(base, total_size, 8)? as usize;
    let strings_start = early_read_be_u32(base, total_size, 12)? as usize;
    let strings_size = early_read_be_u32(base, total_size, 32)? as usize;
    let struct_size = early_read_be_u32(base, total_size, 36)? as usize;
    let struct_end = struct_start
        .checked_add(struct_size)
        .filter(|end| *end <= total_size)
        .ok_or(FdtError::Eof)?;
    strings_start
        .checked_add(strings_size)
        .filter(|end| *end <= total_size)
        .ok_or(FdtError::Eof)?;

    let mut pos = struct_start;
    let mut depth = 0usize;
    let mut root_addr_cells = 2usize;
    let mut root_size_cells = 1usize;
    let mut memory_depth = None;

    while pos < struct_end {
        let token = early_read_be_u32(base, total_size, pos)?;
        pos += 4;
        match token {
            FDT_BEGIN_NODE => {
                let name_start = pos;
                while pos < struct_end && early_read_u8(base, pos) != 0 {
                    pos += 1;
                }
                if pos >= struct_end {
                    return Err(FdtError::Eof);
                }
                let is_memory_node =
                    depth == 1 && early_cstr_starts_with(base, name_start, pos, b"memory");
                pos = align4(pos + 1);
                if is_memory_node {
                    memory_depth = Some(depth);
                }
                depth += 1;
            }
            FDT_END_NODE => {
                if depth == 0 {
                    return Err(FdtError::BadCell);
                }
                depth -= 1;
                if memory_depth == Some(depth) {
                    memory_depth = None;
                }
            }
            FDT_PROP => {
                let len = early_read_be_u32(base, total_size, pos)? as usize;
                let name_offset = early_read_be_u32(base, total_size, pos + 4)? as usize;
                pos += 8;
                let data_start = pos;
                let data_end = data_start.checked_add(len).ok_or(FdtError::Eof)?;
                if data_end > struct_end {
                    return Err(FdtError::Eof);
                }

                if depth == 1
                    && len >= 4
                    && early_prop_name_eq(
                        base,
                        strings_start,
                        strings_size,
                        name_offset,
                        b"#address-cells",
                    )
                {
                    root_addr_cells = early_read_be_u32(base, total_size, data_start)? as usize;
                } else if depth == 1
                    && len >= 4
                    && early_prop_name_eq(
                        base,
                        strings_start,
                        strings_size,
                        name_offset,
                        b"#size-cells",
                    )
                {
                    root_size_cells = early_read_be_u32(base, total_size, data_start)? as usize;
                } else if memory_depth == depth.checked_sub(1)
                    && early_prop_name_eq(base, strings_start, strings_size, name_offset, b"reg")
                {
                    let entry_cells = root_addr_cells + root_size_cells;
                    if entry_cells == 0 {
                        return Err(FdtError::BadCell);
                    }
                    let entry_size = entry_cells * 4;
                    let mut entry = data_start;
                    while entry + entry_size <= data_end {
                        let start = early_read_cells(base, total_size, entry, root_addr_cells)?;
                        let size = early_read_cells(
                            base,
                            total_size,
                            entry + root_addr_cells * 4,
                            root_size_cells,
                        )?;
                        if size != 0 {
                            let end = start.checked_add(size).ok_or(FdtError::BadCell)?;
                            unsafe {
                                #[cfg(not(target_arch = "riscv64"))]
                                add_memory_region(start, end);
                                #[cfg(target_arch = "riscv64")]
                                add_memory_region(start + 0x200_000, end);
                            }
                        }
                        entry += entry_size;
                    }
                }
                pos = align4(data_end);
            }
            FDT_NOP => {}
            FDT_END => break,
            _ => return Err(FdtError::BadCell),
        }
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
    unsafe {
        for (start, size) in MEM_AREA.iter_mut() {
            if *size > alloc_size {
                let ptr = *start;
                *start += alloc_size;
                *size -= alloc_size;
                return ptr as _;
            }
        }
        unreachable!()
    }
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
        .map(|x| (x.0 .0, x.0 .0 + x.1))
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
