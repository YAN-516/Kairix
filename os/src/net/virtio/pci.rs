// net/virtio/pci.rs
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU64, Ordering};

use super::config::*;

// RISC-V QEMU virt machine 默认 ECAM 基址
const DEFAULT_ECAM_BASE: u64 = 0x30000000;
static ECAM_BASE: AtomicU64 = AtomicU64::new(DEFAULT_ECAM_BASE);
#[allow(unused)]
/// 设置 PCIe ECAM 基址（应在扫描设备前调用）
pub fn set_ecam_base(base: u64) {
    ECAM_BASE.store(base, Ordering::Relaxed);
}

/// 获取当前 ECAM 基址
fn get_ecam_base() -> u64 {
    ECAM_BASE.load(Ordering::Relaxed)
}

/// PCI 设备位置 (BDF)
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

    /// 计算 PCIe ECAM 配置空间地址（4 字节对齐）
    fn ecam_addr(&self, offset: u8) -> u64 {
        let base = get_ecam_base();
        let bdf =
            ((self.bus as u64) << 20) | ((self.slot as u64) << 15) | ((self.func as u64) << 12);
        base + bdf + ((offset as u64) & 0xFFC)
    }

    /// 读取 PCI 配置空间（32 位，对齐到 4 字节）
    pub unsafe fn read_config(&self, offset: u8) -> u32 {
        let addr = self.ecam_addr(offset);
        // 必须使用 unsafe 块包裹 read_volatile
        unsafe { read_volatile(addr as *const u32) }
    }

    /// 写入 PCI 配置空间（32 位，对齐到 4 字节）
    pub unsafe fn write_config(&self, offset: u8, value: u32) {
        let addr = self.ecam_addr(offset);
        // 必须使用 unsafe 块包裹 write_volatile
        unsafe { write_volatile(addr as *mut u32, value) }
    }
}
#[allow(unused)]
/// 扫描 PCI 总线找到 VirtIO-net 设备
pub fn scan_for_virtio_net() -> Option<PciLocation> {
    // 扫描 bus 0，slot 0-31，func 0（通常 virtio-net 在 func 0）
    for slot in 0..32 {
        let loc = PciLocation::new(0, slot, 0);
        // 调用 unsafe 函数 read_config 需要 unsafe 块
        let vendor_device = unsafe { loc.read_config(0) };

        let vendor_id = (vendor_device & 0xFFFF) as u16;
        let device_id = ((vendor_device >> 16) & 0xFFFF) as u16;

        if vendor_id == VIRTIO_PCI_VENDOR_ID && device_id == VIRTIO_PCI_DEVICE_ID_NET {
            log::info!("Found VirtIO-net at bus=0, slot={}, func=0", slot);
            return Some(loc);
        }
    }
    None
}
#[allow(unused)]
/// 获取 BAR 基址（物理地址）
pub fn get_bar_base(loc: &PciLocation, bar: u8) -> Option<u64> {
    let bar_offset = 0x10 + (bar as u16) * 4;
    let bar_val = unsafe { loc.read_config(bar_offset as u8) };

    if bar_val == 0xFFFFFFFF || bar_val == 0 {
        return None;
    }

    let is_io = (bar_val & 1) != 0;
    if is_io {
        Some((bar_val & 0xFFFFFFFC) as u64)
    } else {
        Some((bar_val & 0xFFFFFFF0) as u64)
    }
}
#[allow(unused)]
/// 启用总线主控和内存空间
pub fn enable_bus_master(loc: &PciLocation) {
    let command = unsafe { loc.read_config(0x04) };
    unsafe { loc.write_config(0x04, command | 0x4 | 0x2) };
}
