// net/virtio/device.rs
use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

use super::config::*;
use super::pci::{self, PciLocation};
use super::virtqueue::{VirtQueue, VirtQueueMemory, alloc_virtqueue_memory};

use crate::net::device::{NetDevice, NetDeviceFlags, XmitError};
use crate::net::skb::Skb;

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
    isr_status: *mut u8,
    device_cfg: *mut u8,
    rx_vq: Mutex<VirtQueue>,
    tx_vq: Mutex<VirtQueue>,
    rx_handler: Mutex<Option<Box<dyn Fn(Skb) + Send + Sync>>>,
    // 持有内存所有权
    rx_memory: Mutex<Option<VirtQueueMemory>>,
    tx_memory: Mutex<Option<VirtQueueMemory>>,
}
#[allow(unused)]
impl VirtIONetDevice {
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
            isr_status: core::ptr::null_mut(),
            device_cfg: core::ptr::null_mut(),
            rx_vq: Mutex::new(VirtQueue::empty()),
            tx_vq: Mutex::new(VirtQueue::empty()),
            rx_handler: Mutex::new(None),
            rx_memory: Mutex::new(None),
            tx_memory: Mutex::new(None),
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
        let cap_ptr = (unsafe { loc.read_config(0x34) } & 0xFF) as u16;

        if cap_ptr == 0 {
            return false;
        }

        let mut ptr = cap_ptr;
        while ptr != 0 {
            let cap_id = (unsafe { loc.read_config(ptr as u8) } & 0xFF) as u8;
            let next_ptr = ((unsafe { loc.read_config(ptr as u8) } >> 8) & 0xFF) as u16;

            if cap_id == 0x09 {
                let cfg_type = (unsafe { loc.read_config((ptr + 3) as u8) } & 0xFF) as u8;
                let bar = (unsafe { loc.read_config((ptr + 4) as u8) } & 0xFF) as u8;
                let offset = unsafe { loc.read_config((ptr + 8) as u8) };

                if let Some(bar_base) = pci::get_bar_base(loc, bar) {
                    let addr = (bar_base + offset as u64) as *mut u8;

                    match cfg_type {
                        VIRTIO_PCI_CAP_COMMON_CFG => {
                            self.common_cfg = addr as *mut VirtIOCommonCfg;
                        }
                        VIRTIO_PCI_CAP_NOTIFY_CFG => {
                            self.notify_base = addr;
                            self.notify_off_multiplier =
                                unsafe { loc.read_config((ptr + 12) as u8) };
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

            let mem = alloc_virtqueue_memory(size)?;
            let desc_pa = mem.desc_pa;
            let avail_pa = mem.avail_pa;
            let used_pa = mem.used_pa;

            (*self.common_cfg).queue_desc_lo = (desc_pa & 0xFFFFFFFF) as u32;
            (*self.common_cfg).queue_desc_hi = (desc_pa >> 32) as u32;
            (*self.common_cfg).queue_avail_lo = (avail_pa & 0xFFFFFFFF) as u32;
            (*self.common_cfg).queue_avail_hi = (avail_pa >> 32) as u32;
            (*self.common_cfg).queue_used_lo = (used_pa & 0xFFFFFFFF) as u32;
            (*self.common_cfg).queue_used_hi = (used_pa >> 32) as u32;
            (*self.common_cfg).queue_enable = 1;

            let vq = mem.into_virtqueue();

            match queue_idx {
                0 => {
                    *self.rx_vq.lock() = vq;
                }
                1 => {
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
        let mut added = 0;

        for _ in 0..(QUEUE_SIZE / 2) {
            if let Ok(desc_idx) = vq.alloc_desc() {
                let mut skb = Skb::new(2048);
                if skb.put(2048).is_none() {
                    vq.free_desc(desc_idx);
                    break;
                }

                let desc = unsafe { &mut *vq.desc.add(desc_idx as usize) };
                desc.addr = skb.data().as_ptr() as u64;
                desc.len = 2048;
                desc.flags = VIRTQ_DESC_F_WRITE;
                desc.next = 0;

                let avail = unsafe { &mut *vq.avail };
                let avail_idx = avail.idx;
                unsafe {
                    (avail.ring.as_mut_ptr().add(avail_idx as usize)).write(desc_idx);
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
            let offset = self.notify_off_multiplier * queue_idx as u32;
            unsafe {
                self.notify_base
                    .add(offset as usize)
                    .write_volatile(queue_idx as u8);
            }
        }
    }

    fn xmit_frame(&self, skb: Skb) -> Result<(Skb, u32, u16), &'static str> {
        if !self.running.load(Ordering::Acquire) {
            return Err(XmitError::Invalid.into());
        }

        let mut vq = self.tx_vq.lock();
        let data = skb.data();

        if data.len() > 1514 {
            return Err(XmitError::Invalid.into());
        }

        let desc_idx = vq.alloc_desc().map_err(|_| XmitError::Busy)?;

        let desc = unsafe { &mut *vq.desc.add(desc_idx as usize) };
        desc.addr = data.as_ptr() as u64;
        desc.len = data.len() as u32;
        desc.flags = 0;
        desc.next = 0;

        let avail = unsafe { &mut *vq.avail };
        let avail_idx = avail.idx;
        unsafe {
            (avail.ring.as_mut_ptr().add(avail_idx as usize)).write(desc_idx);
        }
        avail.idx = avail_idx.wrapping_add(1);

        core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
        drop(vq);

        self.notify(1);

        log::debug!("VirtIO-net: sent {} bytes", data.len());
        Ok((skb, 0, 0))
    }

    #[allow(unused)]
    pub fn poll_rx(&self) {
        let mut vq = self.rx_vq.lock();
        let used = unsafe { &*vq.used };

        let mut processed = 0;
        while used.idx != vq.last_used_idx {
            let elem = unsafe { &*used.ring.as_ptr().add(vq.last_used_idx as usize) };
            let desc_idx = elem.id as u16;
            let len = elem.len as usize;

            if len > 0 {
                let desc = unsafe { &*vq.desc.add(desc_idx as usize) };

                let mut skb = Skb::new(len);
                if let Some(data) = skb.put(len) {
                    unsafe {
                        data.copy_from_slice(core::slice::from_raw_parts(
                            desc.addr as *const u8,
                            len,
                        ));
                    }

                    if let Some(handler) = self.rx_handler.lock().as_ref() {
                        handler(skb);
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
        //         dev.poll_rx();
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
            isr_status: self.isr_status,
            device_cfg: self.device_cfg,
            rx_vq: Mutex::new(VirtQueue::empty()),
            tx_vq: Mutex::new(VirtQueue::empty()),
            rx_handler: Mutex::new(None),
            rx_memory: Mutex::new(None),
            tx_memory: Mutex::new(None),
        }
    }
}

unsafe impl Send for VirtIONetDevice {}
unsafe impl Sync for VirtIONetDevice {}
