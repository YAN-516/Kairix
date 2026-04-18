// net/virtio/device.rs
use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

use super::config::*;
use super::pci::{self, PciLocation};
use super::virtqueue::{VirtQueue, VirtQueueMemory, alloc_virtqueue_memory};
use crate::config::KERNEL_SPACE_OFFSET;

use crate::net::device::{NetDevice, NetDeviceFlags, XmitError};
use crate::net::skb::Skb;

const VIRTIO_NET_HDR_LEN: usize = 12;
const PCI_MMIO_START: usize = 0x4000_0000;
const PCI_MMIO_END: usize = 0x8000_0000;

#[inline]
fn virt_to_phys(addr: usize) -> u64 {
    if addr >= KERNEL_SPACE_OFFSET {
        (addr - KERNEL_SPACE_OFFSET) as u64
    } else {
        addr as u64
    }
}

#[inline]
fn is_valid_pci_mmio_vaddr(addr: usize) -> bool {
    let paddr = if addr >= KERNEL_SPACE_OFFSET {
        addr - KERNEL_SPACE_OFFSET
    } else {
        return false;
    };
    (PCI_MMIO_START..PCI_MMIO_END).contains(&paddr)
}

/// VirtIO-net 设备
#[allow(unused)]
pub struct VirtIONetDevice {
    name: String,
    mac: [u8; 6],
    ip: u32,
    running: AtomicBool,
    pci_loc: Option<PciLocation>,
    common_cfg: *mut VirtIOCommonCfg,
    notify_base: *mut u8,
    notify_off_multiplier: u32,
    queue_notify_off: [u16; 2],
    isr_status: *mut u8,
    device_cfg: *mut u8,
    rx_vq: Mutex<VirtQueue>,
    tx_vq: Mutex<VirtQueue>,
    rx_handler: Mutex<Option<Box<dyn Fn(Skb) + Send + Sync>>>,
    // 持有内存所有权
    rx_memory: Mutex<Option<VirtQueueMemory>>,
    tx_memory: Mutex<Option<VirtQueueMemory>>,
    rx_buffers: Mutex<Vec<Option<Vec<u8>>>>,
    tx_buffers: Mutex<Vec<Option<Vec<u8>>>>,
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

    pub fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
            mac: [0; 6],
            ip: 0,
            running: AtomicBool::new(false),
            pci_loc: None,
            common_cfg: core::ptr::null_mut(),
            notify_base: core::ptr::null_mut(),
            notify_off_multiplier: 0,
            queue_notify_off: [0; 2],
            isr_status: core::ptr::null_mut(),
            device_cfg: core::ptr::null_mut(),
            rx_vq: Mutex::new(VirtQueue::empty()),
            tx_vq: Mutex::new(VirtQueue::empty()),
            rx_handler: Mutex::new(None),
            rx_memory: Mutex::new(None),
            tx_memory: Mutex::new(None),
            rx_buffers: Mutex::new(vec![None; QUEUE_SIZE as usize]),
            tx_buffers: Mutex::new(vec![None; QUEUE_SIZE as usize]),
        }
    }

    /// 探测 VirtIO 设备
    pub fn probe(&mut self) -> bool {
        // 设置 PCIe ECAM 基址（QEMU virt 平台默认 0x30000000）
        pci::set_ecam_base(0x30000000);
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
                    let vaddr = (bar_base as usize) + (offset as usize) + KERNEL_SPACE_OFFSET;
                    if !is_valid_pci_mmio_vaddr(vaddr) {
                        log::warn!(
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
            log::warn!(
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
        if self.common_cfg.is_null() {
            return Err("Common config not found");
        }

        // 复位设备
        unsafe {
            (*self.common_cfg).device_status = VIRTIO_STATUS_RESET;
        }
        unsafe {
            (*self.common_cfg).device_status = VIRTIO_STATUS_ACK;
        }
        unsafe {
            (*self.common_cfg).device_status |= VIRTIO_STATUS_DRIVER;
        }

        // 协商特性
        unsafe {
            let driver_features = VIRTIO_F_VERSION_1 | VIRTIO_NET_F_MAC;
            (*self.common_cfg).driver_feature_select = 0;
            (*self.common_cfg).driver_feature = (driver_features & 0xFFFFFFFF) as u32;
            (*self.common_cfg).driver_feature_select = 1;
            (*self.common_cfg).driver_feature = (driver_features >> 32) as u32;
        }

        unsafe {
            (*self.common_cfg).device_status |= VIRTIO_STATUS_FEATURES_OK;
        }
        if (unsafe { (*self.common_cfg).device_status } & VIRTIO_STATUS_FEATURES_OK) == 0 {
            return Err("Feature negotiation failed");
        }

        // 初始化队列
        self.init_virtqueue(0)?;
        self.init_virtqueue(1)?;

        // 读取 MAC 地址
        self.read_mac();

        unsafe {
            (*self.common_cfg).device_status |= VIRTIO_STATUS_DRIVER_OK;
        }
        self.running.store(true, Ordering::Release);

        log::info!("VirtIO-net device initialized");
        Ok(())
    }

    fn init_virtqueue(&mut self, queue_idx: u16) -> Result<(), &'static str> {
        unsafe {
            (*self.common_cfg).queue_select = queue_idx;
            (*self.common_cfg).queue_size = QUEUE_SIZE;

            let size = (*self.common_cfg).queue_size;
            if size == 0 {
                return Err("Queue size 0");
            }

            if (queue_idx as usize) < self.queue_notify_off.len() {
                self.queue_notify_off[queue_idx as usize] = (*self.common_cfg).queue_notify_off;
            }

            let mem = alloc_virtqueue_memory(size)?;
            let desc_pa = mem.desc_pa;
            let avail_pa = mem.avail_pa;
            let used_pa = mem.used_pa;

            let desc_pa = virt_to_phys(desc_pa as usize);
            let avail_pa = virt_to_phys(avail_pa as usize);
            let used_pa = virt_to_phys(used_pa as usize);

            (*self.common_cfg).queue_desc_lo = (desc_pa & 0xFFFFFFFF) as u32;
            (*self.common_cfg).queue_desc_hi = (desc_pa >> 32) as u32;
            (*self.common_cfg).queue_avail_lo = (avail_pa & 0xFFFFFFFF) as u32;
            (*self.common_cfg).queue_avail_hi = (avail_pa >> 32) as u32;
            (*self.common_cfg).queue_used_lo = (used_pa & 0xFFFFFFFF) as u32;
            (*self.common_cfg).queue_used_hi = (used_pa >> 32) as u32;
            (*self.common_cfg).queue_enable = 1;

            match queue_idx {
                0 => {
                    *self.rx_memory.lock() = Some(mem);
                    let vq = {
                        let guard = self.rx_memory.lock();
                        guard
                            .as_ref()
                            .ok_or("RX queue memory missing")?
                            .as_virtqueue()
                    };
                    *self.rx_vq.lock() = vq;
                }
                1 => {
                    *self.tx_memory.lock() = Some(mem);
                    let vq = {
                        let guard = self.tx_memory.lock();
                        guard
                            .as_ref()
                            .ok_or("TX queue memory missing")?
                            .as_virtqueue()
                    };
                    *self.tx_vq.lock() = vq;
                }
                _ => {}
            }

            if queue_idx == 0 {
                self.prepare_rx_buffers();
            }
        }

        Ok(())
    }

    fn prepare_rx_buffers(&self) {
        let mut vq = self.rx_vq.lock();
        let mut rx_buffers = self.rx_buffers.lock();
        let mut added = 0;

        for _ in 0..(QUEUE_SIZE / 2) {
            if let Ok(desc_idx) = vq.alloc_desc() {
                let buf = vec![0u8; 2048];

                let desc = unsafe { &mut *vq.desc.add(desc_idx as usize) };
                desc.addr = virt_to_phys(buf.as_ptr() as usize);
                desc.len = 2048;
                desc.flags = VIRTQ_DESC_F_WRITE;
                desc.next = 0;

                rx_buffers[desc_idx as usize] = Some(buf);

                let avail = unsafe { &mut *vq.avail };
                let avail_idx = avail.idx;
                let ring_idx = (avail_idx % vq.queue_size) as usize;
                unsafe {
                    (avail.ring.as_mut_ptr().add(ring_idx)).write(desc_idx);
                }
                avail.idx = avail_idx.wrapping_add(1);
                added += 1;
            } else {
                break;
            }
        }
        drop(vq);

        if added > 0 {
            self.notify(0);
        }
    }

    fn read_mac(&mut self) {
        if !self.device_cfg.is_null() {
            unsafe {
                for i in 0..6 {
                    self.mac[i] = *self.device_cfg.add(i);
                }
            }
            log::info!(
                "VirtIO-net MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                self.mac[0],
                self.mac[1],
                self.mac[2],
                self.mac[3],
                self.mac[4],
                self.mac[5]
            );
        }
    }

    fn notify(&self, queue_idx: u16) {
        if !self.notify_base.is_null() {
            let notify_off = if (queue_idx as usize) < self.queue_notify_off.len() {
                self.queue_notify_off[queue_idx as usize] as u32
            } else {
                queue_idx as u32
            };
            let offset = self.notify_off_multiplier * notify_off;
            unsafe {
                self.notify_base
                    .add(offset as usize)
                    .cast::<u16>()
                    .write_volatile(queue_idx);
            }
        }
    }

    fn xmit_frame(&self, skb: Skb) -> Result<(Skb, u32, u16), &'static str> {
        if !self.running.load(Ordering::Acquire) {
            return Err(XmitError::Invalid.into());
        }

        self.reclaim_tx_used();

        let mut vq = self.tx_vq.lock();
        let data = skb.data();

        // VirtIO-net 报文前必须携带 10 字节 virtio_net_hdr。
        let mut frame = vec![0u8; VIRTIO_NET_HDR_LEN + data.len()];
        frame[VIRTIO_NET_HDR_LEN..].copy_from_slice(data);

        if frame.len() > 1514 + VIRTIO_NET_HDR_LEN {
            return Err(XmitError::Invalid.into());
        }

        let desc_idx = vq.alloc_desc().map_err(|_| XmitError::Busy)?;
        self.tx_buffers.lock()[desc_idx as usize] = Some(frame);
        let tx_buffers = self.tx_buffers.lock();
        let tx_frame = tx_buffers[desc_idx as usize]
            .as_ref()
            .ok_or("tx buffer missing")?;

        let desc = unsafe { &mut *vq.desc.add(desc_idx as usize) };
        desc.addr = virt_to_phys(tx_frame.as_ptr() as usize);
        desc.len = tx_frame.len() as u32;
        desc.flags = 0;
        desc.next = 0;

        let avail = unsafe { &mut *vq.avail };
        let avail_idx = avail.idx;
        let ring_idx = (avail_idx % vq.queue_size) as usize;
        unsafe {
            (avail.ring.as_mut_ptr().add(ring_idx)).write(desc_idx);
        }
        avail.idx = avail_idx.wrapping_add(1);

        core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
        drop(vq);
        drop(tx_buffers);

        self.notify(1);

        log::debug!("VirtIO-net: sent {} bytes", data.len());
        Ok((skb, 0, 0))
    }

    fn reclaim_tx_used(&self) {
        let mut vq = self.tx_vq.lock();
        if vq.used.is_null() {
            return;
        }

        let used = unsafe { &*vq.used };
        let mut tx_buffers = self.tx_buffers.lock();

        while used.idx != vq.last_used_idx {
            let ring_idx = (vq.last_used_idx % vq.queue_size) as usize;
            let elem = unsafe { &*used.ring.as_ptr().add(ring_idx) };
            let desc_idx = elem.id as u16;
            if (desc_idx as usize) < tx_buffers.len() {
                tx_buffers[desc_idx as usize] = None;
            }
            vq.free_desc(desc_idx);
            vq.last_used_idx = vq.last_used_idx.wrapping_add(1);
        }
    }

    #[allow(unused)]
    pub fn poll_rx_once(&self) {
        // 设备未完成初始化时，RX 队列指针可能为空，避免空指针解引用导致内核页故障。
        if !self.running.load(Ordering::Acquire) {
            return;
        }

        let mut vq = self.rx_vq.lock();
        if vq.used.is_null() || vq.desc.is_null() || vq.avail.is_null() {
            return;
        }
        let used = unsafe { &*vq.used };

        let mut processed = 0;
        while used.idx != vq.last_used_idx {
            let ring_idx = (vq.last_used_idx % vq.queue_size) as usize;
            let elem = unsafe { &*used.ring.as_ptr().add(ring_idx) };
            let desc_idx = elem.id as u16;
            let len = elem.len as usize;

            if len > 0 {
                let mut rx_buffers = self.rx_buffers.lock();
                if let Some(buf) = rx_buffers[desc_idx as usize].take() {
                    if len > VIRTIO_NET_HDR_LEN && len <= buf.len() {
                        let pkt_len = len - VIRTIO_NET_HDR_LEN;
                        let mut skb = Skb::new(pkt_len);
                        if let Some(data) = skb.put(pkt_len) {
                            data.copy_from_slice(
                                &buf[VIRTIO_NET_HDR_LEN..VIRTIO_NET_HDR_LEN + pkt_len],
                            );

                            if let Some(handler) = self.rx_handler.lock().as_ref() {
                                handler(skb);
                            }
                        }
                    }
                }
            }

            vq.free_desc(desc_idx);
            vq.last_used_idx = vq.last_used_idx.wrapping_add(1);
            processed += 1;
        }
        drop(vq);

        if processed > 0 {
            self.prepare_rx_buffers();
        }
    }

    pub fn start_rx_thread(&self) {
        let _dev = Arc::new(self.clone());
        // TODO: 使用你的任务系统
        // crate::task::spawn(async move {
        //     loop {
        //         dev.poll_rx_once();
        //         crate::task::yield_now().await;
        //     }
        // });
        log::info!("RX thread started (polling mode)");
    }

    pub fn set_ip(&mut self, ip: u32) {
        self.ip = ip;
    }
}

impl NetDevice for VirtIONetDevice {
    fn name(&self) -> &str {
        &self.name
    }

    fn mtu(&self) -> u16 {
        1500
    }

    fn flags(&self) -> NetDeviceFlags {
        let mut flags = NetDeviceFlags::UP | NetDeviceFlags::RUNNING;
        flags |= NetDeviceFlags::BROADCAST;
        flags
    }

    fn hard_start_xmit(&self, skb: Skb) -> Result<(Skb, u32, u16), &'static str> {
        self.xmit_frame(skb)
    }

    fn set_rx_handler(&self, handler: Box<dyn Fn(Skb) + Send + Sync>) {
        *self.rx_handler.lock() = Some(handler);
        self.start_rx_thread();
    }

    fn poll_rx(&self) {
        self.poll_rx_once();
    }

    fn mac_addr(&self) -> [u8; 6] {
        self.mac
    }

    fn ip_addr(&self) -> u32 {
        self.ip
    }
}

impl Clone for VirtIONetDevice {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            mac: self.mac,
            ip: self.ip,
            running: AtomicBool::new(self.running.load(Ordering::Acquire)),
            pci_loc: self.pci_loc,
            common_cfg: self.common_cfg,
            notify_base: self.notify_base,
            notify_off_multiplier: self.notify_off_multiplier,
            queue_notify_off: self.queue_notify_off,
            isr_status: self.isr_status,
            device_cfg: self.device_cfg,
            rx_vq: Mutex::new(VirtQueue::empty()),
            tx_vq: Mutex::new(VirtQueue::empty()),
            rx_handler: Mutex::new(None),
            rx_memory: Mutex::new(None),
            tx_memory: Mutex::new(None),
            rx_buffers: Mutex::new(vec![None; QUEUE_SIZE as usize]),
            tx_buffers: Mutex::new(vec![None; QUEUE_SIZE as usize]),
        }
    }
}

unsafe impl Send for VirtIONetDevice {}
unsafe impl Sync for VirtIONetDevice {}
