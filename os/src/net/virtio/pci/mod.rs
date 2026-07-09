use core::ptr::{self, read_volatile, write_volatile};
use core::sync::atomic::{AtomicU64, Ordering};

use log::info;
use polyhal::consts::VIRT_ADDR_START;
use virtio_drivers::transport::pci::bus::{
    BarInfo, Command, DeviceFunction, MemoryBarType, PciRoot,
};

use super::config::*;
// use crate::config::KERNEL_SPACE_OFFSET;
// RISC-V QEMU virt machine defaults
const DEFAULT_ECAM_BASE: u64 = 0x3000_0000;
const PCI_INVALID_VENDOR_ID: u16 = 0xFFFF;
const PCI_COMMAND_IO_SPACE: u16 = 1 << 0;
const PCI_COMMAND_MEMORY_SPACE: u16 = 1 << 1;
const PCI_COMMAND_BUS_MASTER: u16 = 1 << 2;

const PCI_HEADER_TYPE_OFFSET: u8 = 0x0C;
const PCI_BAR0_OFFSET: u8 = 0x10;
const PCI_BAR_MEM_64BIT: u32 = 0x2;

// MMIO window for runtime BAR assignment (inside board PCI MMIO aperture)
const PCI_MMIO_BAR_START: u64 = 0x4010_0000;
const PCI_MMIO_BAR_END: u64 = 0x8000_0000;

static ECAM_BASE: AtomicU64 = AtomicU64::new(DEFAULT_ECAM_BASE);
static NEXT_MMIO_BAR_BASE: AtomicU64 = AtomicU64::new(PCI_MMIO_BAR_START);

#[allow(unused)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum PciRangeType {
    ConfigurationSpace,
    IoSpace,
    Memory32,
    Memory64,
}

impl From<u32> for PciRangeType {
    fn from(value: u32) -> Self {
        match value {
            0 => Self::ConfigurationSpace,
            1 => Self::IoSpace,
            2 => Self::Memory32,
            3 => Self::Memory64,
            _ => panic!("invalid PCI range type {}", value),
        }
    }
}

/// 32-bit BAR address allocator (for helper APIs)
#[allow(unused)]
pub struct PciMemory32Allocator {
    start: u32,
    end: u32,
}

impl PciMemory32Allocator {
    #[allow(unused)]
    pub const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    #[allow(unused)]
    pub fn allocate_memory_32(&mut self, size: u32) -> u32 {
        assert!(size.is_power_of_two());
        let allocated = align_up_u32(self.start, size);
        assert!(allocated + size <= self.end);
        self.start = allocated + size;
        allocated
    }
}

#[allow(unused)]
const fn align_up_u32(value: u32, alignment: u32) -> u32 {
    ((value - 1) | (alignment - 1)) + 1
}

#[inline]
fn align_up_u64(value: u64, alignment: u64) -> u64 {
    (value + alignment - 1) & !(alignment - 1)
}

#[allow(unused)]
pub fn dump_bar_contents(root: &mut PciRoot, device_function: DeviceFunction, bar_index: u8) {
    let Ok(bar_info) = root.bar_info(device_function, bar_index) else {
        return;
    };
    info!("Dumping bar {}: {:#x?}", bar_index, bar_info);
    if let BarInfo::Memory { address, size, .. } = bar_info {
        let start = address as *const u8;
        unsafe {
            let mut buf = [0u8; 32];
            for i in 0..size / 32 {
                let p = start.add(i as usize * 32);
                ptr::copy(p, buf.as_mut_ptr(), 32);
                if buf.iter().any(|b| *b != 0xff) {
                    // debug hook
                }
            }
        }
    }
}

/// Allocate BARs using PciRoot helper API (not used in runtime path)
#[allow(unused)]
pub fn allocate_bars(
    root: &mut PciRoot,
    device_function: DeviceFunction,
    allocator: &mut PciMemory32Allocator,
) {
    let Ok(bars) = root.bars(device_function) else {
        return;
    };
    for (bar_index, info) in bars.into_iter().enumerate() {
        let Some(info) = info else {
            continue;
        };
        if let BarInfo::Memory {
            address_type, size, ..
        } = info
        {
            if size > u32::MAX.into() {
                continue;
            }
            let size = size as u32;
            if size == 0 {
                continue;
            }

            match address_type {
                MemoryBarType::Width32 => {
                    let addr = allocator.allocate_memory_32(size);
                    root.set_bar_32(device_function, bar_index as u8, addr);
                }
                MemoryBarType::Width64 => {
                    let addr = allocator.allocate_memory_32(size) as u64;
                    root.set_bar_64(device_function, bar_index as u8, addr);
                }
                _ => {}
            }
        }
    }

    root.set_command(
        device_function,
        Command::IO_SPACE | Command::MEMORY_SPACE | Command::BUS_MASTER,
    );
}

#[allow(unused)]
pub fn set_ecam_base(base: u64) {
    ECAM_BASE.store(base, Ordering::Relaxed);
}

fn get_ecam_base() -> u64 {
    ECAM_BASE.load(Ordering::Relaxed)
}

#[derive(Debug, Clone, Copy)]
pub struct PciLocation {
    pub bus: u8,
    pub slot: u8,
    pub func: u8,
}

impl PciLocation {
    pub fn new(bus: u8, slot: u8, func: u8) -> Self {
        Self { bus, slot, func }
    }

    fn ecam_addr(&self, offset: u8) -> u64 {
        let base = get_ecam_base();
        let bdf =
            ((self.bus as u64) << 20) | ((self.slot as u64) << 15) | ((self.func as u64) << 12);
        base + bdf + ((offset as u64) & 0xFFC)
    }

    #[inline]
    fn ecam_virt_addr(&self, offset: u8) -> usize {
        (self.ecam_addr(offset) as usize) + VIRT_ADDR_START
    }

    pub unsafe fn read_config(&self, offset: u8) -> u32 {
        let vaddr = self.ecam_virt_addr(offset);
        unsafe { read_volatile(vaddr as *const u32) }
    }

    pub unsafe fn write_config(&self, offset: u8, value: u32) {
        let vaddr = self.ecam_virt_addr(offset);
        unsafe { write_volatile(vaddr as *mut u32, value) }
    }

    fn header_type(&self) -> u8 {
        ((unsafe { self.read_config(PCI_HEADER_TYPE_OFFSET) } >> 16) & 0xFF) as u8
    }
}

fn is_present(loc: &PciLocation) -> bool {
    let vendor_id = (unsafe { loc.read_config(0) } & 0xFFFF) as u16;
    vendor_id != PCI_INVALID_VENDOR_ID
}

fn iter_functions(bus: u8, slot: u8) -> impl Iterator<Item = PciLocation> {
    info!("Scanning bus {}, slot {} for functions...", bus, slot);
    let loc0 = PciLocation::new(bus, slot, 0);
    let funcs = if is_present(&loc0) && (loc0.header_type() & 0x80) != 0 {
        8
    } else {
        1
    };
    (0..funcs).map(move |func| PciLocation::new(bus, slot, func))
}

fn bar_mem32_size(loc: &PciLocation, bar_offset: u8) -> Option<u32> {
    let original = unsafe { loc.read_config(bar_offset) };
    if original == 0 || original == 0xFFFF_FFFF || (original & 1) != 0 {
        return None;
    }

    unsafe { loc.write_config(bar_offset, 0xFFFF_FFFF) };
    let probe = unsafe { loc.read_config(bar_offset) };
    unsafe { loc.write_config(bar_offset, original) };

    let mask = probe & 0xFFFF_FFF0;
    if mask == 0 {
        return None;
    }

    let size = (!mask).wrapping_add(1);
    if size == 0 {
        return None;
    }
    Some(size)
}

fn bar_mem64_size(loc: &PciLocation, bar_offset: u8) -> Option<u64> {
    if bar_offset >= PCI_BAR0_OFFSET + 5 * 4 {
        return None;
    }

    let original_low = unsafe { loc.read_config(bar_offset) };
    let original_high = unsafe { loc.read_config(bar_offset + 4) };
    if original_low == 0xFFFF_FFFF || original_high == 0xFFFF_FFFF || (original_low & 1) != 0 {
        return None;
    }

    unsafe {
        loc.write_config(bar_offset, 0xFFFF_FFFF);
        loc.write_config(bar_offset + 4, 0xFFFF_FFFF);
    }
    let probe_low = unsafe { loc.read_config(bar_offset) };
    let probe_high = unsafe { loc.read_config(bar_offset + 4) };
    unsafe {
        loc.write_config(bar_offset, original_low);
        loc.write_config(bar_offset + 4, original_high);
    }

    let mask = ((probe_high as u64) << 32) | ((probe_low as u64) & 0xFFFF_FFF0);
    if mask == 0 {
        return None;
    }

    let size = (!mask).wrapping_add(1);
    if size == 0 {
        return None;
    }
    Some(size)
}

/// Assign 32-bit MMIO BARs when firmware did not do it.
pub fn ensure_mmio_bars_assigned(loc: &PciLocation) {
    let mut bar = 0u8;
    while bar < 6 {
        let bar_offset = PCI_BAR0_OFFSET + bar * 4;
        let bar_val = unsafe { loc.read_config(bar_offset) };

        if bar_val == 0xFFFF_FFFF {
            bar += 1;
            continue;
        }

        // IO BAR not supported in this path.
        if (bar_val & 1) != 0 {
            bar += 1;
            continue;
        }

        let mem_type = (bar_val >> 1) & 0x3;
        if mem_type == PCI_BAR_MEM_64BIT {
            let bar_high = unsafe { loc.read_config(bar_offset + 4) };

            // already assigned
            if (bar_val & 0xFFFF_FFF0) != 0 || bar_high != 0 {
                bar += 2;
                continue;
            }

            let Some(size) = bar_mem64_size(loc, bar_offset) else {
                bar += 2;
                continue;
            };

            let mut next = NEXT_MMIO_BAR_BASE.load(Ordering::Relaxed);
            next = align_up_u64(next, size.max(0x1000));
            if next + size > PCI_MMIO_BAR_END {
                info!(
                    "Skip BAR{} assignment for bus={},slot={},func={} (out of MMIO window)",
                    bar, loc.bus, loc.slot, loc.func
                );
                bar += 2;
                continue;
            }

            NEXT_MMIO_BAR_BASE.store(next + size, Ordering::Relaxed);
            let low = ((next as u32) & 0xFFFF_FFF0) | (bar_val & 0xF);
            let high = (next >> 32) as u32;
            unsafe {
                loc.write_config(bar_offset, low);
                loc.write_config(bar_offset + 4, high);
            }

            info!(
                "Assigned PCI BAR{}-{} for bus={},slot={},func={} => {:#x} (size={:#x})",
                bar,
                bar + 1,
                loc.bus,
                loc.slot,
                loc.func,
                next,
                size
            );

            bar += 2;
            continue;
        }

        // already assigned
        if (bar_val & 0xFFFF_FFF0) != 0 {
            bar += 1;
            continue;
        }

        let Some(size32) = bar_mem32_size(loc, bar_offset) else {
            bar += 1;
            continue;
        };

        let size = size32 as u64;
        let mut next = NEXT_MMIO_BAR_BASE.load(Ordering::Relaxed);
        next = align_up_u64(next, size.max(0x1000));
        if next + size > PCI_MMIO_BAR_END {
            info!(
                "Skip BAR{} assignment for bus={},slot={},func={} (out of MMIO window)",
                bar, loc.bus, loc.slot, loc.func
            );
            bar += 1;
            continue;
        }

        NEXT_MMIO_BAR_BASE.store(next + size, Ordering::Relaxed);
        let low = ((next as u32) & 0xFFFF_FFF0) | (bar_val & 0xF);
        unsafe { loc.write_config(bar_offset, low) };

        info!(
            "Assigned PCI BAR{} for bus={},slot={},func={} => {:#x} (size={:#x})",
            bar, loc.bus, loc.slot, loc.func, next, size
        );

        bar += 1;
    }
}

#[allow(unused)]
fn scan_for_virtio_device(device_id_target: u16) -> Option<PciLocation> {
    for slot in 0..32 {
        for loc in iter_functions(0, slot) {
            if !is_present(&loc) {
                continue;
            }
            let vendor_device = unsafe { loc.read_config(0) };
            let vendor_id = (vendor_device & 0xFFFF) as u16;
            let device_id = ((vendor_device >> 16) & 0xFFFF) as u16;
            if vendor_id == VIRTIO_PCI_VENDOR_ID && device_id == device_id_target {
                return Some(loc);
            }
        }
    }
    None
}

fn scan_for_virtio_devices(device_ids: &[u16]) -> Option<PciLocation> {
    for slot in 0..32 {
        for loc in iter_functions(0, slot) {
            if !is_present(&loc) {
                continue;
            }
            let vendor_device = unsafe { loc.read_config(0) };
            let vendor_id = (vendor_device & 0xFFFF) as u16;
            let device_id = ((vendor_device >> 16) & 0xFFFF) as u16;
            if vendor_id == VIRTIO_PCI_VENDOR_ID && device_ids.iter().any(|id| *id == device_id) {
                return Some(loc);
            }
        }
    }
    None
}

#[allow(unused)]
pub fn scan_for_virtio_net() -> Option<PciLocation> {
    info!("Scanning PCI bus for VirtIO-net device...");
    if let Some(loc) =
        scan_for_virtio_devices(&[VIRTIO_PCI_DEVICE_ID_NET, VIRTIO_PCI_DEVICE_ID_NET_MODERN])
    {
        ensure_mmio_bars_assigned(&loc);
        info!(
            "Found VirtIO-net at bus={}, slot={}, func={}",
            loc.bus, loc.slot, loc.func
        );
        Some(loc)
    } else {
        None
    }
}

#[allow(unused)]
pub fn get_bar_base(loc: &PciLocation, bar: u8) -> Option<u64> {
    if bar >= 6 {
        return None;
    }

    let bar_offset = PCI_BAR0_OFFSET + bar * 4;
    let bar_val = unsafe { loc.read_config(bar_offset) };
    if bar_val == 0xFFFF_FFFF || bar_val == 0 {
        return None;
    }

    // IO BAR is not handled by this MMIO-based driver.
    if (bar_val & 1) != 0 {
        return None;
    }

    let mem_type = (bar_val >> 1) & 0x3;
    if mem_type == PCI_BAR_MEM_64BIT {
        if bar >= 5 {
            return None;
        }
        let high = unsafe { loc.read_config(bar_offset + 4) } as u64;
        let low = (bar_val & 0xFFFF_FFF0) as u64;
        Some((high << 32) | low)
    } else {
        Some((bar_val & 0xFFFF_FFF0) as u64)
    }
}

#[allow(unused)]
pub fn enable_bus_master(loc: &PciLocation) {
    let command_status = unsafe { loc.read_config(0x04) };
    let command = (command_status & 0xFFFF) as u16;
    let command =
        command | PCI_COMMAND_IO_SPACE | PCI_COMMAND_MEMORY_SPACE | PCI_COMMAND_BUS_MASTER;
    let new_value = (command_status & 0xFFFF_0000) | (command as u32);
    unsafe { loc.write_config(0x04, new_value) };
}
