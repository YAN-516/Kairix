//! VirtIO-net 设备实现。
//!
//! 同一套 `VirtIONetDevice` 支持 PCI modern transport 和 MMIO transport。
//! 发送/接收都通过 virtqueue descriptor 把 DMA buffer 交给设备。

use super::config::*;
use super::mmio::MmioNetTransport;
use super::pci::PciLocation;
use super::virtqueue::{VirtQueue, VirtQueueMemory, alloc_virtqueue_memory};
use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicBool, Ordering};
use polyhal::consts::VIRT_ADDR_START;
use spin::Mutex;
// use crate::config::VIRT_ADDR_START;

use crate::net::device::{NetDevice, NetDeviceFlags, XmitError};
use crate::net::skb::Skb;

#[cfg(target_arch = "loongarch64")]
/// LoongArch VirtIO-net header 长度。
const VIRTIO_NET_HDR_LEN: usize = 12;

#[cfg(not(target_arch = "loongarch64"))]
/// RISC-V 等平台使用的 VirtIO-net header 长度。
const VIRTIO_NET_HDR_LEN: usize = 10;

/// 以太网最短帧长度，不包含 FCS。
const ETHERNET_MIN_FRAME_LEN: usize = 60;

#[cfg(target_arch = "loongarch64")]
const LOONGARCH_UNCACHED_DMW_BASE: usize = 0x8000_0000_0000_0000;

/// 将 CPU 虚拟地址转换为设备 DMA 可见的物理地址。
#[inline]
fn virt_to_phys(addr: usize) -> u64 {
    #[cfg(target_arch = "loongarch64")]
    {
        if addr >= VIRT_ADDR_START {
            return (addr - VIRT_ADDR_START) as u64;
        }
        if addr >= LOONGARCH_UNCACHED_DMW_BASE {
            return (addr - LOONGARCH_UNCACHED_DMW_BASE) as u64;
        }
    }

    #[cfg(not(target_arch = "loongarch64"))]
    {
        if addr >= VIRT_ADDR_START {
            return (addr - VIRT_ADDR_START) as u64;
        }
    }

    addr as u64
}

/// 将 DMA buffer 地址转换为 CPU 应访问的地址。
///
/// LoongArch 上使用 uncached DMW 访问 DMA 内存，避免缓存一致性问题。
#[inline]
fn dma_cpu_addr(addr: usize) -> usize {
    #[cfg(target_arch = "loongarch64")]
    {
        return LOONGARCH_UNCACHED_DMW_BASE + virt_to_phys(addr) as usize;
    }

    #[cfg(not(target_arch = "loongarch64"))]
    {
        addr
    }
}

#[inline]
#[allow(unused)]
/// 清零 DMA buffer。
unsafe fn dma_zero(ptr: *mut u8, len: usize) {
    unsafe {
        core::ptr::write_bytes(dma_cpu_addr(ptr as usize) as *mut u8, 0, len);
    }
}

#[inline]
/// 把普通内存数据复制到设备可见的 DMA buffer。
unsafe fn dma_copy_to_device(dst: *mut u8, offset: usize, src: &[u8]) {
    unsafe {
        core::ptr::copy_nonoverlapping(
            src.as_ptr(),
            (dma_cpu_addr(dst as usize) as *mut u8).add(offset),
            src.len(),
        );
    }
}

#[inline]
/// 从设备写过的 DMA buffer 复制到普通内存切片。
unsafe fn dma_copy_from_device(src: *const u8, offset: usize, dst: &mut [u8]) {
    unsafe {
        core::ptr::copy_nonoverlapping(
            (dma_cpu_addr(src as usize) as *const u8).add(offset),
            dst.as_mut_ptr(),
            dst.len(),
        );
    }
}

#[inline]
/// 分配适合作为 DMA buffer 的零初始化内存。
fn dma_alloc_buffer(len: usize) -> Vec<u8> {
    #[cfg(target_arch = "loongarch64")]
    {
        let mut buf = Vec::with_capacity(len);
        unsafe {
            buf.set_len(len);
            dma_zero(buf.as_mut_ptr(), len);
        }
        buf
    }

    #[cfg(not(target_arch = "loongarch64"))]
    {
        vec![0u8; len]
    }
}

#[inline]
/// 为 TX 构造包含 virtio_net_hdr 的 DMA frame。
fn dma_alloc_tx_frame(payload: &[u8], eth_len: usize) -> Vec<u8> {
    let frame_len = VIRTIO_NET_HDR_LEN + eth_len;
    let mut frame = dma_alloc_buffer(frame_len);
    unsafe {
        dma_copy_to_device(frame.as_mut_ptr(), VIRTIO_NET_HDR_LEN, payload);
    }
    frame
}

#[inline]
/// 读取设备写入的数据前使用的 DMA 读屏障。
fn dma_read_barrier() {
    #[cfg(target_arch = "loongarch64")]
    unsafe {
        core::arch::asm!("dbar 0", options(nostack, preserves_flags));
    }

    #[cfg(not(target_arch = "loongarch64"))]
    core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
}

#[inline]
/// 通知设备前使用的 DMA 写屏障。
fn dma_write_barrier() {
    #[cfg(target_arch = "loongarch64")]
    unsafe {
        core::arch::asm!("dbar 0", options(nostack, preserves_flags));
    }

    #[cfg(not(target_arch = "loongarch64"))]
    core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
}

/// VirtIO-net 设备。
#[allow(unused)]
pub struct VirtIONetDevice {
    /// 网络设备名称。
    name: String,
    /// 设备 MAC 地址。
    mac: [u8; 6],
    /// 设备 IPv4 地址。
    ip: u32,
    /// 设备是否完成初始化并可收发。
    pub(crate) running: AtomicBool,
    /// PCI transport 位置；MMIO transport 时为 `None`。
    pub(crate) pci_loc: Option<PciLocation>,
    /// MMIO transport；PCI transport 时为 `None`。
    pub(crate) mmio: Option<MmioNetTransport>,
    /// PCI common config MMIO 指针。
    pub(crate) common_cfg: *mut VirtIOCommonCfg,
    /// PCI notify 区域基址。
    pub(crate) notify_base: *mut u8,
    /// PCI notify 偏移乘数。
    pub(crate) notify_off_multiplier: u32,
    /// 每个队列的 notify offset。
    pub(crate) queue_notify_off: [u16; 2],
    /// PCI ISR 状态寄存器。
    pub(crate) isr_status: *mut u8,
    /// VirtIO-net device-specific config 指针。
    pub(crate) device_cfg: *mut u8,
    /// 接收队列。
    rx_vq: Mutex<VirtQueue>,
    /// 发送队列。
    tx_vq: Mutex<VirtQueue>,
    /// 上层协议栈注册的 RX handler。
    rx_handler: Mutex<Option<Box<dyn Fn(Skb) + Send + Sync>>>,
    /// RX virtqueue backing memory，必须和队列生命周期一致。
    rx_memory: Mutex<Option<VirtQueueMemory>>,
    /// TX virtqueue backing memory，必须和队列生命周期一致。
    tx_memory: Mutex<Option<VirtQueueMemory>>,
    /// RX descriptor 对应的 DMA buffer。
    rx_buffers: Mutex<Vec<Option<Vec<u8>>>>,
    /// TX descriptor 对应的 DMA buffer，used 后才能释放。
    tx_buffers: Mutex<Vec<Option<Vec<u8>>>>,
}

#[allow(unused)]
impl VirtIONetDevice {
    /// 创建尚未绑定 transport 的 VirtIO-net 设备对象。
    pub fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
            mac: [0; 6],
            ip: 0,
            running: AtomicBool::new(false),
            pci_loc: None,
            mmio: None,
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

    /// 绑定 MMIO transport。
    ///
    /// PCI 相关指针会被清空，后续初始化走 MMIO 寄存器访问路径。
    pub(crate) fn attach_mmio(&mut self, transport: MmioNetTransport) {
        self.pci_loc = None;
        self.common_cfg = core::ptr::null_mut();
        self.notify_base = core::ptr::null_mut();
        self.isr_status = core::ptr::null_mut();
        self.notify_off_multiplier = 0;
        self.queue_notify_off = [0; 2];
        self.mmio = Some(transport);
        self.device_cfg = transport.device_config();
    }

    /// 初始化指定 virtqueue。
    ///
    /// queue 0 为 RX，queue 1 为 TX；函数负责分配 queue 内存并写入 transport。
    pub(crate) fn init_virtqueue(&mut self, queue_idx: u16) -> Result<(), &'static str> {
        unsafe {
            let size = if let Some(mmio) = self.mmio.as_ref() {
                let max_size = mmio.max_queue_size(queue_idx);
                if max_size < QUEUE_SIZE {
                    max_size
                } else {
                    QUEUE_SIZE
                }
            } else {
                write_volatile(&mut (*self.common_cfg).queue_select, queue_idx);
                write_volatile(&mut (*self.common_cfg).queue_size, QUEUE_SIZE);
                read_volatile(&(*self.common_cfg).queue_size)
            };
            if size == 0 {
                return Err("Queue size 0");
            }

            if self.mmio.is_none() && (queue_idx as usize) < self.queue_notify_off.len() {
                self.queue_notify_off[queue_idx as usize] =
                    read_volatile(&(*self.common_cfg).queue_notify_off);
            }

            let mem = alloc_virtqueue_memory(size)?;
            let desc_pa = mem.desc_pa;
            let avail_pa = mem.avail_pa;
            let used_pa = mem.used_pa;

            let desc_pa = virt_to_phys(desc_pa as usize);
            let avail_pa = virt_to_phys(avail_pa as usize);
            let used_pa = virt_to_phys(used_pa as usize);

            if let Some(mmio) = self.mmio.as_ref() {
                mmio.setup_queue(queue_idx, size, desc_pa, avail_pa, used_pa);
            } else {
                write_volatile(&mut (*self.common_cfg).queue_select, queue_idx);
                write_volatile(
                    &mut (*self.common_cfg).queue_msix_vector,
                    VIRTIO_MSI_NO_VECTOR,
                );
                write_volatile(&mut (*self.common_cfg).queue_desc, desc_pa);
                write_volatile(&mut (*self.common_cfg).queue_driver, avail_pa);
                write_volatile(&mut (*self.common_cfg).queue_device, used_pa);
                write_volatile(&mut (*self.common_cfg).queue_enable, 1);
            }

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
        }

        Ok(())
    }

    /// 给 RX 队列补充设备可写 buffer。
    ///
    /// VirtIO-net 收包要求驱动先把空 buffer 放入 available ring。
    pub(crate) fn prepare_rx_buffers(&self) {
        let mut vq = self.rx_vq.lock();
        let mut rx_buffers = self.rx_buffers.lock();
        let mut added = 0;

        for _ in 0..(vq.queue_size / 2) {
            if let Ok(desc_idx) = vq.alloc_desc() {
                let buf = dma_alloc_buffer(2048);

                let desc = unsafe { &mut *vq.desc.add(desc_idx as usize) };
                unsafe {
                    write_volatile(&mut desc.addr, virt_to_phys(buf.as_ptr() as usize));
                    write_volatile(&mut desc.len, 2048);
                    write_volatile(&mut desc.flags, VIRTQ_DESC_F_WRITE);
                    write_volatile(&mut desc.next, 0);
                }

                rx_buffers[desc_idx as usize] = Some(buf);

                let avail = unsafe { &mut *vq.avail };
                let avail_idx = unsafe { read_volatile(&avail.idx) };
                let ring_idx = (avail_idx % vq.queue_size) as usize;
                unsafe {
                    write_volatile(avail.ring.as_mut_ptr().add(ring_idx), desc_idx);
                }
                unsafe {
                    write_volatile(&mut avail.idx, avail_idx.wrapping_add(1));
                }
                added += 1;
            } else {
                break;
            }
        }
        dma_write_barrier();
        drop(vq);

        if added > 0 {
            self.notify(0);
        }
    }

    /// 从设备配置空间读取 MAC 地址。
    pub(crate) fn read_mac(&mut self) {
        if !self.device_cfg.is_null() {
            unsafe {
                for i in 0..6 {
                    self.mac[i] = read_volatile(self.device_cfg.add(i));
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

    /// 通知设备某个队列有新 descriptor 可处理。
    fn notify(&self, queue_idx: u16) {
        if let Some(mmio) = self.mmio.as_ref() {
            mmio.notify(queue_idx);
            return;
        }

        if !self.notify_base.is_null() {
            let notify_off = unsafe {
                write_volatile(&mut (*self.common_cfg).queue_select, queue_idx);
                read_volatile(&(*self.common_cfg).queue_notify_off) as u32
            };
            let offset = self.notify_off_multiplier * notify_off;
            unsafe {
                let notify_addr = self.notify_base.add(offset as usize).cast::<u16>();
                notify_addr.write_volatile(queue_idx);
            }
        }
    }

    /// 发送一个完整以太网帧。
    fn xmit_frame(&self, skb: Skb) -> Result<(Skb, u32, u16), &'static str> {
        if !self.running.load(Ordering::Acquire) {
            return Err(XmitError::Invalid.into());
        }

        self.reclaim_tx_used();

        let mut vq = self.tx_vq.lock();
        let data = skb.data();

        // VirtIO-net 报文前必须携带 virtio_net_hdr。
        let eth_len = core::cmp::max(data.len(), ETHERNET_MIN_FRAME_LEN);
        let frame = dma_alloc_tx_frame(data, eth_len);

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
        unsafe {
            write_volatile(&mut desc.addr, virt_to_phys(tx_frame.as_ptr() as usize));
            write_volatile(&mut desc.len, tx_frame.len() as u32);
            write_volatile(&mut desc.flags, 0);
            write_volatile(&mut desc.next, 0);
        }

        let avail = unsafe { &mut *vq.avail };
        let avail_idx = unsafe { read_volatile(&avail.idx) };
        let ring_idx = (avail_idx % vq.queue_size) as usize;
        unsafe {
            write_volatile(avail.ring.as_mut_ptr().add(ring_idx), desc_idx);
        }
        unsafe {
            write_volatile(&mut avail.idx, avail_idx.wrapping_add(1));
        }

        dma_write_barrier();
        drop(vq);
        drop(tx_buffers);

        self.notify(1);

        log::info!("VirtIO-net: sent {} bytes", data.len());
        Ok((skb, 0, 0))
    }

    /// 回收设备已经消费完的 TX descriptor 和 DMA buffer。
    fn reclaim_tx_used(&self) {
        let mut vq = self.tx_vq.lock();
        if vq.used.is_null() {
            return;
        }

        let mut tx_buffers = self.tx_buffers.lock();

        while unsafe { read_volatile(&(*vq.used).idx) } != vq.last_used_idx {
            let ring_idx = (vq.last_used_idx % vq.queue_size) as usize;
            let elem = unsafe { (*vq.used).ring.as_ptr().add(ring_idx) };
            let desc_idx = unsafe { read_volatile(&(*elem).id) } as u16;
            if (desc_idx as usize) < tx_buffers.len() {
                tx_buffers[desc_idx as usize] = None;
            }
            vq.free_desc(desc_idx);
            vq.last_used_idx = vq.last_used_idx.wrapping_add(1);
        }
    }

    #[allow(unused)]
    /// 轮询一次 RX used ring。
    ///
    /// 对每个已完成的 RX descriptor，跳过 virtio_net_hdr 后生成 `Skb` 并交给 handler。
    pub fn poll_rx_once(&self) {
        // 设备未完成初始化时，RX 队列指针可能为空，避免空指针解引用导致内核页故障。
        if !self.running.load(Ordering::Acquire) {
            return;
        }

        let Some(mut vq) = self.rx_vq.try_lock() else {
            return;
        };
        if vq.used.is_null() || vq.desc.is_null() || vq.avail.is_null() {
            return;
        }
        dma_read_barrier();

        let mut processed = 0;
        while unsafe { read_volatile(&(*vq.used).idx) } != vq.last_used_idx {
            let ring_idx = (vq.last_used_idx % vq.queue_size) as usize;
            let elem = unsafe { (*vq.used).ring.as_ptr().add(ring_idx) };
            let desc_idx = unsafe { read_volatile(&(*elem).id) } as u16;
            let len = unsafe { read_volatile(&(*elem).len) } as usize;

            let mut rx_skb = None;
            if len > 0 {
                let buf = {
                    let mut rx_buffers = self.rx_buffers.lock();
                    rx_buffers[desc_idx as usize].take()
                };
                if let Some(buf) = buf {
                    if len > VIRTIO_NET_HDR_LEN && len <= buf.len() {
                        let pkt_len = len - VIRTIO_NET_HDR_LEN;
                        let mut skb = Skb::new(pkt_len);
                        if let Some(data) = skb.put(pkt_len) {
                            unsafe {
                                dma_copy_from_device(buf.as_ptr(), VIRTIO_NET_HDR_LEN, data);
                            }
                            rx_skb = Some(skb);
                        }
                    }
                }
            }

            if let Some(skb) = rx_skb {
                if let Some(handler) = self.rx_handler.lock().as_ref() {
                    handler(skb);
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

    /// 启动接收线程的占位接口。
    ///
    /// 当前系统使用显式轮询，因此这里只记录日志。
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

    /// 设置设备 IPv4 地址。
    pub fn set_ip(&mut self, ip: u32) {
        self.ip = ip;
    }

    /// 复位底层 VirtIO 设备。
    pub(crate) fn reset_device(&self) {
        if let Some(mmio) = self.mmio.as_ref() {
            mmio.reset();
        } else {
            unsafe {
                write_volatile(&mut (*self.common_cfg).device_status, VIRTIO_STATUS_RESET);
            }
        }
    }

    /// OR 写入 VirtIO device status 位。
    pub(crate) fn add_status(&self, status: u8) {
        if let Some(mmio) = self.mmio.as_ref() {
            mmio.add_status(status);
        } else {
            unsafe {
                let current = read_volatile(&(*self.common_cfg).device_status);
                write_volatile(&mut (*self.common_cfg).device_status, current | status);
            }
        }
    }

    /// 读取 VirtIO device status。
    pub(crate) fn device_status(&self) -> u8 {
        if let Some(mmio) = self.mmio.as_ref() {
            mmio.status()
        } else {
            unsafe { read_volatile(&(*self.common_cfg).device_status) }
        }
    }

    /// 读取设备支持的 feature bits。
    pub(crate) fn read_device_features(&self) -> u64 {
        if let Some(mmio) = self.mmio.as_ref() {
            mmio.read_device_features()
        } else {
            unsafe {
                write_volatile(&mut (*self.common_cfg).device_feature_select, 0);
                let low = read_volatile(&(*self.common_cfg).device_feature) as u64;
                write_volatile(&mut (*self.common_cfg).device_feature_select, 1);
                low | ((read_volatile(&(*self.common_cfg).device_feature) as u64) << 32)
            }
        }
    }

    /// 写入驱动选择启用的 feature bits。
    pub(crate) fn write_driver_features(&self, driver_features: u64) {
        if let Some(mmio) = self.mmio.as_ref() {
            let features = if mmio.is_legacy() {
                driver_features & !VIRTIO_F_VERSION_1
            } else {
                driver_features
            };
            mmio.write_driver_features(features);
        } else {
            unsafe {
                write_volatile(&mut (*self.common_cfg).driver_feature_select, 0);
                write_volatile(
                    &mut (*self.common_cfg).driver_feature,
                    (driver_features & 0xFFFFFFFF) as u32,
                );
                write_volatile(&mut (*self.common_cfg).driver_feature_select, 1);
                write_volatile(
                    &mut (*self.common_cfg).driver_feature,
                    (driver_features >> 32) as u32,
                );
            }
        }
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
            mmio: self.mmio,
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
