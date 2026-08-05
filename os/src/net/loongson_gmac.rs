//! Polling driver for the Loongson 2K1000 PCI DWMAC 3.x controllers.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::arch::asm;
use core::mem::size_of;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicBool, Ordering};

use polyhal::consts::PAGE_SIZE;
use spin::Mutex;

use crate::mm::{PhysPageNum, frame_alloc_hal, frame_dealloc};
use crate::net::device::{NetDevice, NetDeviceFlags, XmitError};
use crate::net::skb::Skb;

const LOONGARCH_UNCACHED_DMW_BASE: usize = 0x8000_0000_0000_0000;

// LS2K1000 CFG0 uses (devfn << 8) rather than the PCI ECAM layout.
/// Loongson 2K1000 PCI CFG0 配置空间物理基址。
const PCI_CFG0_PHYS: usize = 0x1a00_0000;
/// 片上 GMAC 控制器所在 PCI slot。
const PCI_SLOT_GMAC: u8 = 3;
/// Loongson PCI vendor ID。
const PCI_VENDOR_LOONGSON: u16 = 0x0014;
/// GMAC0 PCI device ID。
const PCI_DEVICE_GMAC0: u16 = 0x7a03;
/// GMAC1 PCI device ID。
const PCI_DEVICE_GMAC1: u16 = 0x7a23;
const PCI_COMMAND_MEMORY: u16 = 1 << 1;
const PCI_COMMAND_BUS_MASTER: u16 = 1 << 2;

// GMAC MAC/DMA register offsets.
const GMAC_CONTROL: usize = 0x0000;
const GMAC_FRAME_FILTER: usize = 0x0004;
const GMAC_MII_ADDR: usize = 0x0010;
const GMAC_MII_DATA: usize = 0x0014;
const GMAC_FLOW_CONTROL: usize = 0x0018;
const GMAC_VERSION: usize = 0x0020;
const GMAC_INT_MASK: usize = 0x003c;
const GMAC_ADDR_HIGH0: usize = 0x0040;
const GMAC_ADDR_LOW0: usize = 0x0044;

const DMA_BUS_MODE: usize = 0x1000;
const DMA_TX_POLL_DEMAND: usize = 0x1004;
const DMA_RX_POLL_DEMAND: usize = 0x1008;
const DMA_RX_DESC_BASE: usize = 0x100c;
const DMA_TX_DESC_BASE: usize = 0x1010;
const DMA_STATUS: usize = 0x1014;
const DMA_OPERATION_MODE: usize = 0x1018;
const DMA_INT_ENABLE: usize = 0x101c;
const DMA_MISSED_FRAMES: usize = 0x1020;
const DMA_HW_FEATURE: usize = 0x1058;

// MAC control bits.
const MAC_JABBER_DISABLE: u32 = 1 << 22;
const MAC_FRAME_BURST: u32 = 1 << 21;
const MAC_DISABLE_CARRIER_SENSE: u32 = 1 << 16;
const MAC_PORT_SELECT: u32 = 1 << 15;
const MAC_FAST_ETHERNET_SPEED: u32 = 1 << 14;
const MAC_DUPLEX: u32 = 1 << 11;
const MAC_TX_ENABLE: u32 = 1 << 3;
const MAC_RX_ENABLE: u32 = 1 << 2;
const MAC_SPEED_MASK: u32 = MAC_PORT_SELECT | MAC_FAST_ETHERNET_SPEED;

// DMA control/status bits.
const DMA_SOFT_RESET: u32 = 1 << 0;
const DMA_BUS_PBL_SHIFT: u32 = 8;
const DMA_BUS_RX_PBL_SHIFT: u32 = 17;
const DMA_BUS_USE_SEPARATE_PBL: u32 = 1 << 23;
const DMA_BUS_PBL_X8: u32 = 1 << 24;
const DMA_OP_START_RX: u32 = 1 << 1;
const DMA_OP_SECOND_FRAME: u32 = 1 << 2;
const DMA_OP_START_TX: u32 = 1 << 13;
const DMA_OP_FLUSH_TX_FIFO: u32 = 1 << 20;
const DMA_OP_TX_STORE_FORWARD: u32 = 1 << 21;
const DMA_OP_DISABLE_RX_FLUSH: u32 = 1 << 24;
const DMA_OP_RX_STORE_FORWARD: u32 = 1 << 25;
const DMA_STATUS_RX_UNAVAILABLE: u32 = 1 << 7;
const DMA_STATUS_RX_STOPPED: u32 = 1 << 8;
const DMA_STATUS_CLEAR: u32 = 0x0007_ffff;

// MDIO command fields.
const MDIO_BUSY: u32 = 1 << 0;
const MDIO_WRITE: u32 = 1 << 1;
// Linux's Loongson glue uses CSR clock range 2 (20-35 MHz input clock).
const MDIO_CLOCK_20_35_MHZ: u32 = 2 << 2;
const MDIO_REG_SHIFT: u32 = 6;
const MDIO_PHY_SHIFT: u32 = 11;

// Standard MII registers and link negotiation bits.
const MII_BMCR: u8 = 0;
const MII_BMSR: u8 = 1;
const MII_PHYSID1: u8 = 2;
const MII_PHYSID2: u8 = 3;
const MII_ADVERTISE: u8 = 4;
const MII_LPA: u8 = 5;
const MII_CTRL1000: u8 = 9;
const MII_STAT1000: u8 = 10;
const MII_YTPHY_STATUS: u8 = 0x11;
const BMCR_SPEED1000: u16 = 1 << 6;
const BMCR_FULL_DUPLEX: u16 = 1 << 8;
const BMCR_RESTART_AN: u16 = 1 << 9;
const BMCR_ISOLATE: u16 = 1 << 10;
const BMCR_POWER_DOWN: u16 = 1 << 11;
const BMCR_AN_ENABLE: u16 = 1 << 12;
const BMCR_SPEED100: u16 = 1 << 13;
const BMSR_LINK: u16 = 1 << 2;
const BMSR_AN_COMPLETE: u16 = 1 << 5;

// Motorcomm YT8511 settings used by Linux for the LS2K1000 rgmii-id ports.
const PHY_ID_YT8511: u32 = 0x0000_010a;
const YT8511_PAGE_SELECT: u8 = 0x1e;
const YT8511_PAGE_DATA: u8 = 0x1f;
const YT8511_EXT_CLK_GATE: u16 = 0x000c;
const YT8511_EXT_DELAY_DRIVE: u16 = 0x000d;
const YT8511_EXT_SLEEP_CTRL: u16 = 0x0027;
const YT8511_DELAY_RX: u16 = 1 << 0;
const YT8511_DELAY_GE_TX_MASK: u16 = 0xf << 4;
const YT8511_DELAY_GE_TX_ENABLE: u16 = 0xf << 4;
const YT8511_DELAY_FE_TX_MASK: u16 = 0xf << 12;
const YT8511_DELAY_FE_TX_ENABLE: u16 = 0xf << 12;
const YT8511_CLK_125M: u16 = (1 << 2) | (1 << 1);
const YT8511_PLL_ON_SLEEP: u16 = 1 << 14;

// Enhanced DWMAC descriptor bits. LS2K1000 advertises ENHDESSEL and its DMA
// ignores the normal-descriptor TX control bits in des1.
const DESC_OWN: u32 = 1 << 31;
const RX_DESC_LAST: u32 = 1 << 8;
const RX_DESC_FIRST: u32 = 1 << 9;
const RX_DESC_ERROR: u32 = 1 << 15;
const RX_DESC_FRAME_LEN_MASK: u32 = 0x3fff << 16;
const RX_DESC_BUFFER_SIZE_MASK: u32 = 0x1fff;
const RX_DESC_END_RING: u32 = 1 << 15;
const TX_DESC_END_RING: u32 = 1 << 21;
const TX_DESC_FIRST: u32 = 1 << 28;
const TX_DESC_LAST: u32 = 1 << 29;
const TX_DESC_ERROR: u32 = 1 << 15;
const TX_DESC_BUFFER_SIZE_MASK: u32 = 0x1fff;

// Ring and buffer layout.
const RX_RING_SIZE: usize = 32;
const TX_RING_SIZE: usize = 4;
const DMA_BUFFER_SIZE: usize = 1536;
const TX_DESC_OFFSET: usize = 0x400;
const ETHERNET_MIN_FRAME: usize = 60;
const ETHERNET_MAX_FRAME: usize = 1518;
const LINK_POLL_INTERVAL_SECS: u64 = 1;

#[repr(C)]
#[derive(Clone, Copy)]
/// Loongson 2K1000 GMAC DMA descriptor。
struct DmaDesc {
    des0: u32,
    des1: u32,
    des2: u32,
    des3: u32,
}

const _: () = assert!(size_of::<DmaDesc>() == 16);

/// 单个 4 KiB DMA 页。
///
/// 2K1000 GMAC 只能可靠访问 32-bit DMA 地址，因此分配后会检查物理地址上限。
struct DmaPage {
    ppn: PhysPageNum,
}

impl DmaPage {
    /// 分配并清零一个 DMA 页。
    fn allocate() -> Result<Self, &'static str> {
        let ppn = frame_alloc_hal().ok_or("failed to allocate Loongson GMAC DMA page")?;
        let paddr = (ppn.0 as u64) << 12;
        if paddr + PAGE_SIZE as u64 > (u32::MAX as u64) + 1 {
            frame_dealloc(ppn);
            return Err("Loongson GMAC DMA page is above the 32-bit DMA limit");
        }
        let mut page = Self { ppn };
        page.bytes().fill(0);
        dma_barrier();
        Ok(page)
    }

    /// 返回 DMA 页的 32-bit 物理地址。
    fn paddr(&self) -> u32 {
        ((self.ppn.0 as u64) << 12) as u32
    }

    /// 返回 CPU 通过 uncached DMW 访问该 DMA 页的地址。
    fn cpu_addr(&self) -> usize {
        LOONGARCH_UNCACHED_DMW_BASE | self.paddr() as usize
    }

    /// 以可变字节切片形式访问 DMA 页。
    fn bytes(&mut self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.cpu_addr() as *mut u8, PAGE_SIZE) }
    }
}

impl Drop for DmaPage {
    fn drop(&mut self) {
        frame_dealloc(self.ppn);
    }
}

/// GMAC 使用的 DMA 内存集合。
struct DmaMemory {
    /// RX/TX descriptor 共享的 DMA 页。
    descriptors: DmaPage,
    /// RX packet buffers。
    rx_buffers: Vec<DmaPage>,
    /// TX packet buffers。
    tx_buffers: Vec<DmaPage>,
}

impl DmaMemory {
    /// 分配 descriptor 页和所有 RX/TX buffer 页。
    fn allocate() -> Result<Self, &'static str> {
        let descriptors = DmaPage::allocate()?;
        let mut rx_buffers = Vec::with_capacity(RX_RING_SIZE);
        let mut tx_buffers = Vec::with_capacity(TX_RING_SIZE);
        for _ in 0..RX_RING_SIZE {
            rx_buffers.push(DmaPage::allocate()?);
        }
        for _ in 0..TX_RING_SIZE {
            tx_buffers.push(DmaPage::allocate()?);
        }
        Ok(Self {
            descriptors,
            rx_buffers,
            tx_buffers,
        })
    }

    /// RX descriptor ring 物理地址。
    fn rx_ring_paddr(&self) -> u32 {
        self.descriptors.paddr()
    }

    /// TX descriptor ring 物理地址。
    fn tx_ring_paddr(&self) -> u32 {
        self.descriptors.paddr() + TX_DESC_OFFSET as u32
    }

    /// 获取指定 RX descriptor 指针。
    fn rx_desc(&self, index: usize) -> *mut DmaDesc {
        (self.descriptors.cpu_addr() + index * size_of::<DmaDesc>()) as *mut DmaDesc
    }

    /// 获取指定 TX descriptor 指针。
    fn tx_desc(&self, index: usize) -> *mut DmaDesc {
        (self.descriptors.cpu_addr() + TX_DESC_OFFSET + index * size_of::<DmaDesc>())
            as *mut DmaDesc
    }
}

/// 2K1000 固定 CFG0 空间中的 PCI function。
#[derive(Clone, Copy)]
struct PciFunction {
    /// PCI function number。
    function: u8,
    /// PCI device id。
    device_id: u16,
}

impl PciFunction {
    /// 计算配置空间寄存器地址。
    fn cfg_addr(&self, offset: usize) -> usize {
        let devfn = ((PCI_SLOT_GMAC as usize) << 3) | self.function as usize;
        LOONGARCH_UNCACHED_DMW_BASE | (PCI_CFG0_PHYS + (devfn << 8) + (offset & !3))
    }

    /// 读取 PCI 配置空间 32 位值。
    fn read(&self, offset: usize) -> u32 {
        let value = unsafe { read_volatile(self.cfg_addr(offset) as *const u32) };
        dma_barrier();
        value
    }

    /// 写入 PCI 配置空间 32 位值。
    fn write(&self, offset: usize, value: u32) {
        unsafe { write_volatile(self.cfg_addr(offset) as *mut u32, value) };
        dma_barrier();
    }

    /// 允许 memory space 和 bus mastering。
    fn enable(&self) {
        let command_status = self.read(0x04);
        let command = (command_status as u16) | PCI_COMMAND_MEMORY | PCI_COMMAND_BUS_MASTER;
        self.write(0x04, (command_status & 0xffff_0000) | command as u32);
    }

    /// 读取 BAR0 MMIO 物理基址。
    fn bar0(&self) -> Result<u32, &'static str> {
        let bar = self.read(0x10);
        if bar == 0 || bar == u32::MAX {
            return Err("Loongson GMAC BAR0 is not assigned by firmware");
        }
        if bar & 1 != 0 {
            return Err("Loongson GMAC BAR0 is an unsupported I/O BAR");
        }
        if (bar >> 1) & 0x3 == 0x2 && self.read(0x14) != 0 {
            return Err("Loongson GMAC BAR0 is above the 32-bit MMIO range");
        }
        Ok(bar & 0xffff_fff0)
    }
}

/// PHY 报告的链路模式。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LinkMode {
    /// 链路速率，单位 Mbps。
    speed: u16,
    /// 是否全双工。
    full_duplex: bool,
}

#[derive(Clone, Copy)]
/// 一个可用 GMAC function 候选。
struct Candidate {
    /// PCI function。
    pci: PciFunction,
    /// uncached MMIO 虚拟基址。
    base: usize,
    /// PHY 地址。
    phy: u8,
    /// PHY ID。
    phy_id: u32,
    /// GMAC 版本寄存器值。
    version: u32,
}

/// 驱动运行态。
struct GmacState {
    /// descriptor 和 packet buffer DMA 内存。
    dma: DmaMemory,
    /// 下一次轮询的 RX descriptor 下标。
    rx_index: usize,
    /// 下一次使用的 TX descriptor 下标。
    tx_index: usize,
    /// RX 丢包计数。
    rx_dropped: u64,
    /// 上一次轮询 PHY 链路状态的 tick。
    last_link_poll: u64,
    /// 当前链路是否 up。
    link_up: bool,
    /// 当前链路模式。
    link_mode: Option<LinkMode>,
}

/// 防止 RX 轮询重入的 RAII guard。
struct RxPollGuard<'a> {
    active: &'a AtomicBool,
}

impl Drop for RxPollGuard<'_> {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
    }
}

/// Loongson 2K1000 physical GMAC network device.
pub struct LoongsonGmacDevice {
    /// 设备名。
    name: String,
    /// MAC 地址。
    mac: [u8; 6],
    /// IPv4 地址。
    ip: u32,
    /// GMAC uncached MMIO 虚拟基址。
    base: usize,
    /// 当前使用的 PHY 地址。
    phy: u8,
    /// DMA ring、索引和链路状态。
    state: Mutex<GmacState>,
    /// RX 轮询重入保护。
    rx_polling: AtomicBool,
    /// 上层协议栈注册的 RX handler。
    rx_handler: Mutex<Option<Arc<dyn Fn(Skb) + Send + Sync>>>,
}

impl LoongsonGmacDevice {
    /// Probe both on-chip GMAC functions and prefer the port whose PHY link is up.
    pub fn probe(name: &str, ip: u32) -> Result<Self, &'static str> {
        let candidate = select_candidate()?;
        candidate.pci.enable();

        dma_reset(candidate.base)?;
        configure_yt8511(candidate.base, candidate.phy, candidate.phy_id)?;
        initialize_phy(candidate.base, candidate.phy)?;

        let mut dma = DmaMemory::allocate()?;
        initialize_descriptors(&mut dma);

        let mac = read_mac(candidate.base).unwrap_or([
            0x02,
            0x4b,
            0x32,
            0x4b,
            0x10,
            candidate.pci.function,
        ]);
        configure_hardware(candidate.base, &dma, mac);

        let mode = read_link_mode(candidate.base, candidate.phy).ok().flatten();
        if let Some(mode) = mode {
            configure_mac_link(candidate.base, mode);
        }

        log::info!(
            "Loongson GMAC: PCI 00:03.{}, device={:#06x}, BAR0={:#010x}, version={:#010x}, PHY@{}={:#010x}, MAC={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            candidate.pci.function,
            candidate.pci.device_id,
            candidate.base & 0xffff_ffff,
            candidate.version,
            candidate.phy,
            candidate.phy_id,
            mac[0],
            mac[1],
            mac[2],
            mac[3],
            mac[4],
            mac[5],
        );

        Ok(Self {
            name: String::from(name),
            mac,
            ip,
            base: candidate.base,
            phy: candidate.phy,
            state: Mutex::new(GmacState {
                dma,
                rx_index: 0,
                tx_index: 0,
                rx_dropped: 0,
                last_link_poll: 0,
                link_up: mode.is_some(),
                link_mode: mode,
            }),
            rx_polling: AtomicBool::new(false),
            rx_handler: Mutex::new(None),
        })
    }

    /// 发送一个完整以太网帧。
    fn transmit(&self, skb: Skb) -> Result<(Skb, u32, u16), &'static str> {
        let payload = skb.data();
        if payload.len() > ETHERNET_MAX_FRAME {
            return Err(XmitError::Invalid.into());
        }

        let mut state = self.state.lock();
        self.poll_link(&mut state);
        let index = state.tx_index;
        let desc = state.dma.tx_desc(index);
        dma_barrier();
        if unsafe { read_volatile(core::ptr::addr_of!((*desc).des0)) } & DESC_OWN != 0 {
            return Err(XmitError::Busy.into());
        }

        let frame_len = payload.len().max(ETHERNET_MIN_FRAME);
        let buffer = state.dma.tx_buffers[index].bytes();
        buffer[..payload.len()].copy_from_slice(payload);
        buffer[payload.len()..frame_len].fill(0);
        let buffer_paddr = state.dma.tx_buffers[index].paddr();
        let end = if index + 1 == TX_RING_SIZE {
            TX_DESC_END_RING
        } else {
            0
        };

        unsafe {
            write_volatile(core::ptr::addr_of_mut!((*desc).des0), end);
            write_volatile(
                core::ptr::addr_of_mut!((*desc).des1),
                frame_len as u32 & TX_DESC_BUFFER_SIZE_MASK,
            );
            write_volatile(core::ptr::addr_of_mut!((*desc).des2), buffer_paddr);
            write_volatile(core::ptr::addr_of_mut!((*desc).des3), 0);
        }
        dma_barrier();
        unsafe {
            write_volatile(
                core::ptr::addr_of_mut!((*desc).des0),
                DESC_OWN | TX_DESC_FIRST | TX_DESC_LAST | end,
            )
        };
        dma_barrier();
        mmio_write(self.base, DMA_TX_POLL_DEMAND, 1);

        let start = polyhal::timer::get_ticks();
        let timeout = polyhal::timer::get_freq().max(1);
        loop {
            dma_barrier();
            let status = unsafe { read_volatile(core::ptr::addr_of!((*desc).des0)) };
            if status & DESC_OWN == 0 {
                if status & TX_DESC_ERROR != 0 {
                    log::error!(
                        "Loongson GMAC TX error: index={} desc0={:#010x} dma_status={:#010x}",
                        index,
                        status,
                        mmio_read(self.base, DMA_STATUS),
                    );
                    return Err("Loongson GMAC transmit error");
                }
                break;
            }
            if polyhal::timer::get_ticks().wrapping_sub(start) >= timeout {
                log::error!(
                    "Loongson GMAC TX timeout: index={} dma_status={:#010x} missed={:#010x}",
                    index,
                    mmio_read(self.base, DMA_STATUS),
                    mmio_read(self.base, DMA_MISSED_FRAMES),
                );
                return Err("Loongson GMAC transmit completion timeout");
            }
            core::hint::spin_loop();
        }

        state.tx_index = (index + 1) % TX_RING_SIZE;
        Ok((skb, 0, 0))
    }

    /// 定期读取 PHY 链路状态，并根据协商结果更新 MAC。
    fn poll_link(&self, state: &mut GmacState) {
        let now = polyhal::timer::get_ticks();
        let interval = polyhal::timer::get_freq().saturating_mul(LINK_POLL_INTERVAL_SECS);
        if state.last_link_poll != 0 && now.wrapping_sub(state.last_link_poll) < interval {
            return;
        }
        state.last_link_poll = now;

        let mode = match read_link_mode(self.base, self.phy) {
            Ok(mode) => mode,
            Err(error) => {
                log::warn!("Loongson GMAC link read failed: {}", error);
                return;
            }
        };
        match mode {
            Some(mode) => {
                if state.link_mode != Some(mode) {
                    configure_mac_link(self.base, mode);
                    log::info!(
                        "Loongson GMAC link up: {} Mbps/{} duplex",
                        mode.speed,
                        if mode.full_duplex { "full" } else { "half" },
                    );
                }
                state.link_up = true;
                state.link_mode = Some(mode);
            }
            None => {
                if state.link_up {
                    log::info!("Loongson GMAC link down");
                }
                state.link_up = false;
                state.link_mode = None;
            }
        }
    }

    /// 轮询 RX descriptor ring，把收到的帧交给上层 handler。
    fn poll_receive(&self) {
        if self
            .rx_polling
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        let _guard = RxPollGuard {
            active: &self.rx_polling,
        };

        loop {
            let skb = {
                let Some(mut state) = self.state.try_lock() else {
                    return;
                };
                self.poll_link(&mut state);

                let index = state.rx_index;
                let desc = state.dma.rx_desc(index);
                dma_barrier();
                let status = unsafe { read_volatile(core::ptr::addr_of!((*desc).des0)) };
                if status & DESC_OWN != 0 {
                    let dma_status = mmio_read(self.base, DMA_STATUS);
                    if dma_status & (DMA_STATUS_RX_UNAVAILABLE | DMA_STATUS_RX_STOPPED) != 0 {
                        mmio_write(self.base, DMA_STATUS, DMA_STATUS_CLEAR);
                        mmio_write(self.base, DMA_RX_POLL_DEMAND, 1);
                    }
                    return;
                }

                let raw_len = ((status & RX_DESC_FRAME_LEN_MASK) >> 16) as usize;
                let valid = status & (RX_DESC_FIRST | RX_DESC_LAST)
                    == (RX_DESC_FIRST | RX_DESC_LAST)
                    && status & RX_DESC_ERROR == 0
                    && (4..=DMA_BUFFER_SIZE).contains(&raw_len);
                let packet_len = raw_len.saturating_sub(4);
                let mut packet = None;
                if valid && packet_len <= ETHERNET_MAX_FRAME {
                    let buffer = state.dma.rx_buffers[index].bytes();
                    let mut skb = Skb::new(packet_len);
                    if let Some(dst) = skb.put(packet_len) {
                        dst.copy_from_slice(&buffer[..packet_len]);
                        packet = Some(skb);
                    }
                } else {
                    state.rx_dropped += 1;
                    if state.rx_dropped <= 4 || state.rx_dropped.is_power_of_two() {
                        log::warn!(
                            "Loongson GMAC RX drop: count={} index={} len={} desc0={:#010x}",
                            state.rx_dropped,
                            index,
                            raw_len,
                            status,
                        );
                    }
                }

                rearm_rx_descriptor(&mut state.dma, index);
                state.rx_index = (index + 1) % RX_RING_SIZE;
                mmio_write(self.base, DMA_STATUS, DMA_STATUS_CLEAR);
                packet
            };

            if let Some(skb) = skb {
                if let Some(handler) = self.rx_handler.lock().clone() {
                    handler(skb);
                }
            }
        }
    }
}

impl NetDevice for LoongsonGmacDevice {
    fn name(&self) -> &str {
        &self.name
    }

    fn mtu(&self) -> u16 {
        1500
    }

    fn flags(&self) -> NetDeviceFlags {
        NetDeviceFlags::UP | NetDeviceFlags::RUNNING | NetDeviceFlags::BROADCAST
    }

    fn hard_start_xmit(&self, skb: Skb) -> Result<(Skb, u32, u16), &'static str> {
        self.transmit(skb)
    }

    fn set_rx_handler(&self, handler: Box<dyn Fn(Skb) + Send + Sync>) {
        *self.rx_handler.lock() = Some(Arc::from(handler));
    }

    fn poll_rx(&self) {
        self.poll_receive();
    }

    fn mac_addr(&self) -> [u8; 6] {
        self.mac
    }

    fn ip_addr(&self) -> u32 {
        self.ip
    }
}

/// 在两个片上 GMAC function 中选择可用端口。
///
/// 优先选择 PHY link 已经 up 的端口；都未 up 时返回第一个可用候选。
fn select_candidate() -> Result<Candidate, &'static str> {
    let mut first = None;
    for function in 0..=1 {
        let probe = PciFunction {
            function,
            device_id: 0,
        };
        let id = probe.read(0x00);
        let vendor = id as u16;
        let device = (id >> 16) as u16;
        log::info!(
            "Loongson GMAC PCI probe: 00:03.{} vendor={:#06x} device={:#06x}",
            function,
            vendor,
            device,
        );
        if vendor != PCI_VENDOR_LOONGSON
            || (device != PCI_DEVICE_GMAC0 && device != PCI_DEVICE_GMAC1)
        {
            continue;
        }

        let pci = PciFunction {
            function,
            device_id: device,
        };
        pci.enable();
        let bar = pci.bar0()?;
        let base = LOONGARCH_UNCACHED_DMW_BASE | bar as usize;
        let version = mmio_read(base, GMAC_VERSION);
        if version == 0 || version == u32::MAX {
            log::warn!(
                "Loongson GMAC 00:03.{} BAR0={:#010x} does not respond",
                function,
                bar,
            );
            continue;
        }

        // Both on-board PHYs observed on the 2K1000 board use address 0 on
        // their respective per-GMAC MDIO buses. Keep the full scan fallback
        // for board revisions wired to a different address.
        let expected_phy = 0;
        let (phy, phy_id) = match find_phy(base, expected_phy) {
            Ok(found) => found,
            Err(error) => {
                log::warn!(
                    "Loongson GMAC 00:03.{} PHY probe failed: {}",
                    function,
                    error,
                );
                continue;
            }
        };
        let link_up = phy_link_up(base, phy).unwrap_or(false);
        let candidate = Candidate {
            pci,
            base,
            phy,
            phy_id,
            version,
        };
        if link_up {
            return Ok(candidate);
        }
        if first.is_none() {
            first = Some(candidate);
        }
    }
    first.ok_or("no usable Loongson 2K1000 GMAC found at PCI 00:03.0/1")
}

/// 在 MDIO 总线上寻找 PHY。
fn find_phy(base: usize, expected: u8) -> Result<(u8, u32), &'static str> {
    if let Some(id) = read_phy_id(base, expected)? {
        return Ok((expected, id));
    }
    for phy in 0..32 {
        if phy == expected {
            continue;
        }
        if let Some(id) = read_phy_id(base, phy)? {
            log::warn!(
                "Loongson GMAC PHY found at {}, expected {}; using detected address",
                phy,
                expected,
            );
            return Ok((phy, id));
        }
    }
    Err("no PHY responded on the Loongson GMAC MDIO bus")
}

/// 读取 PHY ID，未响应时返回 `Ok(None)`。
fn read_phy_id(base: usize, phy: u8) -> Result<Option<u32>, &'static str> {
    let id1 = mdio_read(base, phy, MII_PHYSID1)?;
    let id2 = mdio_read(base, phy, MII_PHYSID2)?;
    if (id1 == 0 && id2 == 0) || (id1 == u16::MAX && id2 == u16::MAX) {
        Ok(None)
    } else {
        Ok(Some(((id1 as u32) << 16) | id2 as u32))
    }
}

/// Apply the LS2K1000 `rgmii-id` setup from Linux's YT8511 PHY driver.
fn configure_yt8511(base: usize, phy: u8, phy_id: u32) -> Result<(), &'static str> {
    if phy_id != PHY_ID_YT8511 {
        return Ok(());
    }

    let old_page = mdio_read(base, phy, YT8511_PAGE_SELECT)?;
    let result: Result<(), &'static str> = (|| {
        mdio_write(base, phy, YT8511_PAGE_SELECT, YT8511_EXT_CLK_GATE)?;
        let clock_delay_before = mdio_read(base, phy, YT8511_PAGE_DATA)?;
        let clock_delay_after = (clock_delay_before & !(YT8511_DELAY_RX | YT8511_DELAY_GE_TX_MASK))
            | YT8511_DELAY_RX
            | YT8511_DELAY_GE_TX_ENABLE
            | YT8511_CLK_125M;
        mdio_write(base, phy, YT8511_PAGE_DATA, clock_delay_after)?;

        mdio_write(base, phy, YT8511_PAGE_SELECT, YT8511_EXT_DELAY_DRIVE)?;
        let fast_delay_before = mdio_read(base, phy, YT8511_PAGE_DATA)?;
        let fast_delay_after =
            (fast_delay_before & !YT8511_DELAY_FE_TX_MASK) | YT8511_DELAY_FE_TX_ENABLE;
        mdio_write(base, phy, YT8511_PAGE_DATA, fast_delay_after)?;

        mdio_write(base, phy, YT8511_PAGE_SELECT, YT8511_EXT_SLEEP_CTRL)?;
        let sleep_before = mdio_read(base, phy, YT8511_PAGE_DATA)?;
        let sleep_after = sleep_before | YT8511_PLL_ON_SLEEP;
        mdio_write(base, phy, YT8511_PAGE_DATA, sleep_after)?;

        log::info!(
            "Loongson GMAC YT8511 rgmii-id: clock/delay={:#06x}->{:#06x}, fast-delay={:#06x}->{:#06x}, sleep={:#06x}->{:#06x}",
            clock_delay_before,
            clock_delay_after,
            fast_delay_before,
            fast_delay_after,
            sleep_before,
            sleep_after,
        );
        Ok(())
    })();
    let restore = mdio_write(base, phy, YT8511_PAGE_SELECT, old_page);
    result?;
    restore
}

/// 解除 PHY isolate/power-down 并重启自协商。
fn initialize_phy(base: usize, phy: u8) -> Result<(), &'static str> {
    let bmcr = mdio_read(base, phy, MII_BMCR)?;
    let bmcr = (bmcr & !(BMCR_ISOLATE | BMCR_POWER_DOWN)) | BMCR_AN_ENABLE | BMCR_RESTART_AN;
    mdio_write(base, phy, MII_BMCR, bmcr)
}

/// 读取 PHY link bit。
fn phy_link_up(base: usize, phy: u8) -> Result<bool, &'static str> {
    let _ = mdio_read(base, phy, MII_BMSR)?;
    Ok(mdio_read(base, phy, MII_BMSR)? & BMSR_LINK != 0)
}

/// 读取当前链路速率和双工模式。
fn read_link_mode(base: usize, phy: u8) -> Result<Option<LinkMode>, &'static str> {
    let _ = mdio_read(base, phy, MII_BMSR)?;
    let bmsr = mdio_read(base, phy, MII_BMSR)?;
    if bmsr & BMSR_LINK == 0 {
        return Ok(None);
    }

    // YT8511 reports the resolved RGMII mode directly in register 0x11.
    let phy_id = read_phy_id(base, phy)?.unwrap_or(0);
    if phy_id == PHY_ID_YT8511 {
        let status = mdio_read(base, phy, MII_YTPHY_STATUS)?;
        if status & (1 << 10) == 0 || status & (1 << 11) == 0 {
            return Ok(None);
        }
        let speed = match (status >> 14) & 0x3 {
            0 => 10,
            1 => 100,
            2 => 1000,
            _ => return Err("YT8511 reported an invalid link speed"),
        };
        return Ok(Some(LinkMode {
            speed,
            full_duplex: status & (1 << 13) != 0,
        }));
    }

    let bmcr = mdio_read(base, phy, MII_BMCR)?;
    if bmcr & BMCR_AN_ENABLE == 0 || bmsr & BMSR_AN_COMPLETE == 0 {
        let speed = if bmcr & BMCR_SPEED1000 != 0 {
            1000
        } else if bmcr & BMCR_SPEED100 != 0 {
            100
        } else {
            10
        };
        return Ok(Some(LinkMode {
            speed,
            full_duplex: bmcr & BMCR_FULL_DUPLEX != 0,
        }));
    }

    let ctrl1000 = mdio_read(base, phy, MII_CTRL1000)?;
    let stat1000 = mdio_read(base, phy, MII_STAT1000)?;
    if ctrl1000 & (1 << 9) != 0 && stat1000 & (1 << 11) != 0 {
        return Ok(Some(LinkMode {
            speed: 1000,
            full_duplex: true,
        }));
    }
    if ctrl1000 & (1 << 8) != 0 && stat1000 & (1 << 10) != 0 {
        return Ok(Some(LinkMode {
            speed: 1000,
            full_duplex: false,
        }));
    }

    let common = mdio_read(base, phy, MII_ADVERTISE)? & mdio_read(base, phy, MII_LPA)?;
    for (bit, speed, full_duplex) in [
        (1 << 8, 100, true),
        (1 << 7, 100, false),
        (1 << 6, 10, true),
        (1 << 5, 10, false),
    ] {
        if common & bit != 0 {
            return Ok(Some(LinkMode { speed, full_duplex }));
        }
    }
    Err("PHY link is up but no common speed was negotiated")
}

/// 复位 GMAC DMA engine。
fn dma_reset(base: usize) -> Result<(), &'static str> {
    let current = mmio_read(base, DMA_BUS_MODE);
    if current & DMA_SOFT_RESET != 0 {
        return Err("Loongson GMAC DMA reset is already asserted; PHY clock may be missing");
    }
    mmio_write(base, DMA_BUS_MODE, current | DMA_SOFT_RESET);
    let start = polyhal::timer::get_ticks();
    let timeout = polyhal::timer::get_freq().saturating_mul(2).max(1);
    loop {
        if mmio_read(base, DMA_BUS_MODE) & DMA_SOFT_RESET == 0 {
            return Ok(());
        }
        if polyhal::timer::get_ticks().wrapping_sub(start) >= timeout {
            return Err("Loongson GMAC DMA reset timed out after two seconds");
        }
        core::hint::spin_loop();
    }
}

/// 初始化 RX/TX descriptor ring。
fn initialize_descriptors(dma: &mut DmaMemory) {
    for index in 0..RX_RING_SIZE {
        rearm_rx_descriptor(dma, index);
    }
    for index in 0..TX_RING_SIZE {
        let desc = dma.tx_desc(index);
        let end = if index + 1 == TX_RING_SIZE {
            TX_DESC_END_RING
        } else {
            0
        };
        unsafe {
            write_volatile(core::ptr::addr_of_mut!((*desc).des0), end);
            write_volatile(core::ptr::addr_of_mut!((*desc).des1), 0);
            write_volatile(core::ptr::addr_of_mut!((*desc).des2), 0);
            write_volatile(core::ptr::addr_of_mut!((*desc).des3), 0);
        }
    }
    dma_barrier();
}

/// 重新把一个 RX descriptor 交给 DMA。
fn rearm_rx_descriptor(dma: &mut DmaMemory, index: usize) {
    let desc = dma.rx_desc(index);
    let buffer_paddr = dma.rx_buffers[index].paddr();
    let end = if index + 1 == RX_RING_SIZE {
        RX_DESC_END_RING
    } else {
        0
    };
    unsafe {
        write_volatile(core::ptr::addr_of_mut!((*desc).des0), 0);
        write_volatile(
            core::ptr::addr_of_mut!((*desc).des1),
            end | (DMA_BUFFER_SIZE as u32 & RX_DESC_BUFFER_SIZE_MASK),
        );
        write_volatile(core::ptr::addr_of_mut!((*desc).des2), buffer_paddr);
        write_volatile(core::ptr::addr_of_mut!((*desc).des3), 0);
    }
    dma_barrier();
    unsafe { write_volatile(core::ptr::addr_of_mut!((*desc).des0), DESC_OWN) };
    dma_barrier();
}

/// 配置 GMAC MAC、DMA 和 ring 基址。
fn configure_hardware(base: usize, dma: &DmaMemory, mac: [u8; 6]) {
    mmio_write(base, DMA_INT_ENABLE, 0);
    mmio_write(base, DMA_STATUS, DMA_STATUS_CLEAR);

    let bus_mode = DMA_BUS_USE_SEPARATE_PBL
        | DMA_BUS_PBL_X8
        | (32 << DMA_BUS_PBL_SHIFT)
        | (32 << DMA_BUS_RX_PBL_SHIFT);
    mmio_write(base, DMA_BUS_MODE, bus_mode);
    mmio_write(base, DMA_RX_DESC_BASE, dma.rx_ring_paddr());
    mmio_write(base, DMA_TX_DESC_BASE, dma.tx_ring_paddr());

    write_mac(base, mac);
    mmio_write(base, GMAC_FRAME_FILTER, 0);
    mmio_write(base, GMAC_FLOW_CONTROL, 0);
    mmio_write(base, GMAC_INT_MASK, 0x0000_020f);

    let mac_control = MAC_JABBER_DISABLE
        | MAC_FRAME_BURST
        | MAC_DISABLE_CARRIER_SENSE
        | MAC_DUPLEX
        | MAC_TX_ENABLE
        | MAC_RX_ENABLE;
    mmio_write(base, GMAC_CONTROL, mac_control);

    let operation = DMA_OP_SECOND_FRAME
        | DMA_OP_TX_STORE_FORWARD
        | DMA_OP_DISABLE_RX_FLUSH
        | DMA_OP_RX_STORE_FORWARD
        | DMA_OP_START_TX
        | DMA_OP_START_RX;
    mmio_write(base, DMA_OPERATION_MODE, operation | DMA_OP_FLUSH_TX_FIFO);
    for _ in 0..100_000 {
        if mmio_read(base, DMA_OPERATION_MODE) & DMA_OP_FLUSH_TX_FIFO == 0 {
            break;
        }
        core::hint::spin_loop();
    }
    mmio_write(base, DMA_OPERATION_MODE, operation);
    mmio_write(base, DMA_RX_POLL_DEMAND, 1);

    log::info!(
        "Loongson GMAC DMA: RX ring={:#010x}, TX ring={:#010x}, bus={:#010x}, op={:#010x}, feature={:#010x}",
        dma.rx_ring_paddr(),
        dma.tx_ring_paddr(),
        mmio_read(base, DMA_BUS_MODE),
        mmio_read(base, DMA_OPERATION_MODE),
        mmio_read(base, DMA_HW_FEATURE),
    );
}

/// 根据 PHY 协商结果更新 MAC 速率和双工位。
fn configure_mac_link(base: usize, mode: LinkMode) {
    let speed = match mode.speed {
        1000 => 0,
        100 => MAC_PORT_SELECT | MAC_FAST_ETHERNET_SPEED,
        10 => MAC_PORT_SELECT,
        _ => return,
    };
    let duplex = if mode.full_duplex { MAC_DUPLEX } else { 0 };
    let control = mmio_read(base, GMAC_CONTROL);
    mmio_write(
        base,
        GMAC_CONTROL,
        (control & !(MAC_SPEED_MASK | MAC_DUPLEX)) | speed | duplex,
    );
}

/// 从 MAC 寄存器读取固件预置地址。
fn read_mac(base: usize) -> Option<[u8; 6]> {
    let high = mmio_read(base, GMAC_ADDR_HIGH0);
    let low = mmio_read(base, GMAC_ADDR_LOW0);
    let mac = [
        low as u8,
        (low >> 8) as u8,
        (low >> 16) as u8,
        (low >> 24) as u8,
        high as u8,
        (high >> 8) as u8,
    ];
    if mac.iter().all(|byte| *byte == 0) || mac.iter().all(|byte| *byte == 0xff) || mac[0] & 1 != 0
    {
        None
    } else {
        Some(mac)
    }
}

/// 写入 MAC 地址寄存器。
fn write_mac(base: usize, mac: [u8; 6]) {
    let high = (mac[4] as u32) | ((mac[5] as u32) << 8);
    let low = (mac[0] as u32)
        | ((mac[1] as u32) << 8)
        | ((mac[2] as u32) << 16)
        | ((mac[3] as u32) << 24);
    mmio_write(base, GMAC_ADDR_HIGH0, high);
    mmio_write(base, GMAC_ADDR_LOW0, low);
}

/// 通过 MDIO 读取 PHY 寄存器。
fn mdio_read(base: usize, phy: u8, reg: u8) -> Result<u16, &'static str> {
    mdio_wait_idle(base)?;
    let command = ((phy as u32) << MDIO_PHY_SHIFT)
        | ((reg as u32) << MDIO_REG_SHIFT)
        | MDIO_CLOCK_20_35_MHZ
        | MDIO_BUSY;
    mmio_write(base, GMAC_MII_ADDR, command);
    mdio_wait_idle(base)?;
    Ok(mmio_read(base, GMAC_MII_DATA) as u16)
}

/// 通过 MDIO 写入 PHY 寄存器。
fn mdio_write(base: usize, phy: u8, reg: u8, value: u16) -> Result<(), &'static str> {
    mdio_wait_idle(base)?;
    mmio_write(base, GMAC_MII_DATA, value as u32);
    let command = ((phy as u32) << MDIO_PHY_SHIFT)
        | ((reg as u32) << MDIO_REG_SHIFT)
        | MDIO_CLOCK_20_35_MHZ
        | MDIO_WRITE
        | MDIO_BUSY;
    mmio_write(base, GMAC_MII_ADDR, command);
    mdio_wait_idle(base)
}

/// 等待 MDIO 控制器空闲。
fn mdio_wait_idle(base: usize) -> Result<(), &'static str> {
    for _ in 0..100_000 {
        if mmio_read(base, GMAC_MII_ADDR) & MDIO_BUSY == 0 {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err("Loongson GMAC MDIO transaction timed out")
}

/// 读取 32 位 MMIO 寄存器并执行 DMA barrier。
#[inline]
fn mmio_read(base: usize, offset: usize) -> u32 {
    let value = unsafe { read_volatile((base + offset) as *const u32) };
    dma_barrier();
    value
}

/// 写入 32 位 MMIO 寄存器并执行 DMA barrier。
#[inline]
fn mmio_write(base: usize, offset: usize, value: u32) {
    unsafe { write_volatile((base + offset) as *mut u32, value) };
    dma_barrier();
}

/// LoongArch DMA/MMIO 访问屏障。
#[inline]
fn dma_barrier() {
    unsafe { asm!("dbar 0", options(nostack, preserves_flags)) };
}
