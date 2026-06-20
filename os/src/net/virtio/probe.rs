use super::config::*;
use super::device::VirtIONetDevice;
use core::sync::atomic::Ordering;

use super::mmio;
use super::pci::{self, PciLocation};

#[cfg(target_arch = "loongarch64")]
fn init_ecam_base_from_fdt() -> bool {
    use flat_device_tree::Fdt;

    // 与 block 驱动保持一致：bootloader 将 DTB 放在该虚拟地址。
    const LOONGARCH_FDT_VADDR: u64 = 0x9000_0000_0010_0000;

    let Ok(fdt) = (unsafe { Fdt::from_ptr(LOONGARCH_FDT_VADDR as *const u8) }) else {
        log::info!("virtio-net: failed to parse FDT, fallback to default ECAM base");
        return false;
    };

    let Some(pci_node) = fdt.find_compatible(&["pci-host-ecam-generic"]) else {
        log::info!("virtio-net: cannot find pci-host-ecam-generic in FDT");
        return false;
    };

    let mut regs = pci_node.reg();
    let Some(region) = regs.next() else {
        log::info!("virtio-net: PCI node has no reg region in FDT");
        return false;
    };

    let ecam_base = region.starting_address as u64;
    pci::set_ecam_base(ecam_base);
    log::info!("virtio-net: use ECAM base from FDT: {:#x}", ecam_base);
    true
}

#[cfg(not(target_arch = "loongarch64"))]
fn init_ecam_base_from_fdt() -> bool {
    false
}

#[allow(unused)]
impl VirtIONetDevice {
    #[inline]
    fn pci_read_u8(loc: &PciLocation, offset: u16) -> u8 {
        let aligned = (offset & !0x3) as u8;
        let shift = ((offset & 0x3) * 8) as u32;
        ((unsafe { loc.read_config(aligned) } >> shift) & 0xFF) as u8
    }

    #[inline]
    fn pci_read_u32(loc: &PciLocation, offset: u16) -> u32 {
        unsafe { loc.read_config(offset as u8) }
    }

    /// 探测 VirtIO 设备
    pub fn probe(&mut self) -> bool {
        if !init_ecam_base_from_fdt() {
            // 兜底：QEMU virt 平台默认 ECAM 为 0x30000000。
            pci::set_ecam_base(0x30000000);
        }
        let loc = match pci::scan_for_virtio_net() {
            Some(loc) => loc,
            None => return false,
        };
        self.pci_loc = Some(loc);

        let vendor_id = unsafe { loc.read_config(0) } & 0xFFFF;
        let device_id = (unsafe { loc.read_config(0) } >> 16) & 0xFFFF;

        log::info!(
            "Found VirtIO-net device: vendor=0x{:x}, device=0x{:x}",
            vendor_id,
            device_id
        );
        // 启用总线主控和内存空间
        pci::enable_bus_master(&loc);
        // 解析能力列表
        self.parse_capabilities(&loc)
    }

    pub fn probe_mmio(&mut self) -> bool {
        let Some(transport) = mmio::probe_virtio_net() else {
            return false;
        };
        self.attach_mmio(transport);
        true
    }

    fn parse_capabilities(&mut self, loc: &PciLocation) -> bool {
        let cap_ptr = Self::pci_read_u8(loc, 0x34) as u16;

        if cap_ptr == 0 {
            return false;
        }

        let mut ptr = cap_ptr;
        while ptr != 0 {
            let cap_id = Self::pci_read_u8(loc, ptr);
            let next_ptr = Self::pci_read_u8(loc, ptr + 1) as u16;

            if cap_id == 0x09 {
                let cfg_type = Self::pci_read_u8(loc, ptr + 3);
                let bar = Self::pci_read_u8(loc, ptr + 4);
                let offset = Self::pci_read_u32(loc, ptr + 8);

                if let Some(bar_base) = pci::get_bar_base(loc, bar) {
                    let vaddr =
                        (bar_base as usize) + (offset as usize) + polyhal::consts::VIRT_ADDR_START;
                    if !(0x4000_0000..0x8000_0000)
                        .contains(&(vaddr - polyhal::consts::VIRT_ADDR_START))
                    {
                        log::info!(
                            "virtio-pci cap cfg_type={} has invalid MMIO vaddr {:#x} (bar_base={:#x}, offset={:#x})",
                            cfg_type,
                            vaddr,
                            bar_base,
                            offset
                        );
                        ptr = next_ptr;
                        continue;
                    }
                    let addr = vaddr as *mut u8;

                    match cfg_type {
                        VIRTIO_PCI_CAP_COMMON_CFG => {
                            self.common_cfg = addr as *mut VirtIOCommonCfg;
                        }
                        VIRTIO_PCI_CAP_NOTIFY_CFG => {
                            self.notify_base = addr;
                            self.notify_off_multiplier = Self::pci_read_u32(loc, ptr + 16);
                        }
                        VIRTIO_PCI_CAP_ISR_CFG => {
                            self.isr_status = addr;
                        }
                        VIRTIO_PCI_CAP_DEVICE_CFG => {
                            self.device_cfg = addr;
                        }
                        _ => {}
                    }
                }
            }

            ptr = next_ptr;
        }

        if self.common_cfg.is_null()
            || self.notify_base.is_null()
            || self.isr_status.is_null()
            || self.device_cfg.is_null()
        {
            log::info!(
                "virtio-pci capability parse incomplete: common={:p} notify={:p} isr={:p} device={:p}",
                self.common_cfg,
                self.notify_base,
                self.isr_status,
                self.device_cfg
            );
            return false;
        }

        !self.common_cfg.is_null()
            && !self.notify_base.is_null()
            && !self.isr_status.is_null()
            && !self.device_cfg.is_null()
    }

    /// 初始化设备
    pub fn init_device(&mut self) -> Result<(), &'static str> {
        if self.common_cfg.is_null() && self.mmio.is_none() {
            return Err("Common config not found");
        }

        // 复位设备
        self.reset_device();
        self.add_status(VIRTIO_STATUS_ACK);
        self.add_status(VIRTIO_STATUS_DRIVER);

        // 协商特性
        let driver_features = VIRTIO_F_VERSION_1 | VIRTIO_NET_F_MAC;
        self.write_driver_features(driver_features);

        self.add_status(VIRTIO_STATUS_FEATURES_OK);
        if (self.device_status() & VIRTIO_STATUS_FEATURES_OK) == 0 {
            return Err("Feature negotiation failed");
        }

        // 初始化队列
        self.init_virtqueue(0)?;
        self.init_virtqueue(1)?;

        // 读取 MAC 地址
        self.read_mac();

        self.add_status(VIRTIO_STATUS_DRIVER_OK);
        self.running.store(true, Ordering::Release);

        log::info!("VirtIO-net device initialized");
        Ok(())
    }
}

/// 探测并初始化 VirtIO-net 设备。
///
/// 调用方只需要拿到一个已经完成硬件发现和初始化的网络设备。
pub fn probe_virtio_net(name: &str) -> Option<VirtIONetDevice> {
    let mut device = VirtIONetDevice::new(name);
    if !device.probe() && !device.probe_mmio() {
        return None;
    }
    if device.init_device().is_err() {
        return None;
    }
    Some(device)
}
