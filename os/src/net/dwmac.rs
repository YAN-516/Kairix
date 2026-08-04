//! Polling driver for the JH7110 GMAC0 (Synopsys DWMAC 5.10a).

use alloc::alloc::{alloc_zeroed, Layout};
use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use core::mem::{align_of, size_of};
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{fence, AtomicBool, Ordering};

use polyhal::consts::VIRT_ADDR_START;
use spin::Mutex;

use crate::net::device::{NetDevice, NetDeviceFlags, XmitError};
use crate::net::skb::Skb;

const GMAC_PHYS: usize = 0x1603_0000;
/// 系统时钟/复位控制器基址。
const SYS_CRG_PHYS: usize = 0x1302_0000;
/// always-on 时钟/复位控制器基址。
const AON_CRG_PHYS: usize = 0x1700_0000;
/// always-on syscon 基址。
const AON_SYSCON_PHYS: usize = 0x1701_0000;
/// StarFive ccache 控制器基址，用于手动 flush cache line。
const CCACHE_PHYS: usize = 0x0201_0000;

// GMAC core/MTL/DMA register offsets.
const GMAC_CONFIG: usize = 0x0000;
const GMAC_PACKET_FILTER: usize = 0x0008;
const GMAC_VERSION: usize = 0x0110;
const GMAC_RXQ_CTRL0: usize = 0x00a0;
const GMAC_INT_EN: usize = 0x00b4;
const GMAC_Q0_TX_FLOW_CTRL: usize = 0x0070;
const GMAC_RX_FLOW_CTRL: usize = 0x0090;
const GMAC_LPI_CTRL_STATUS: usize = 0x00d0;
const GMAC_MDIO_ADDR: usize = 0x0200;
const GMAC_MDIO_DATA: usize = 0x0204;
const GMAC_ADDR_HIGH0: usize = 0x0300;
const GMAC_ADDR_LOW0: usize = 0x0304;
const MTL_TXQ0_OP_MODE: usize = 0x0d00;
const MTL_RXQ0_OP_MODE: usize = 0x0d30;

const DMA_MODE: usize = 0x1000;
const DMA_SYS_BUS_MODE: usize = 0x1004;
const DMA_DEBUG_STATUS0: usize = 0x100c;
const DMA_CHAN_CONTROL: usize = 0x1100;
const DMA_CHAN_TX_CONTROL: usize = 0x1104;
const DMA_CHAN_RX_CONTROL: usize = 0x1108;
const DMA_CHAN_TX_BASE_HI: usize = 0x1110;
const DMA_CHAN_TX_BASE: usize = 0x1114;
const DMA_CHAN_RX_BASE_HI: usize = 0x1118;
const DMA_CHAN_RX_BASE: usize = 0x111c;
const DMA_CHAN_TX_TAIL: usize = 0x1120;
const DMA_CHAN_RX_TAIL: usize = 0x1128;
const DMA_CHAN_TX_RING_LEN: usize = 0x112c;
const DMA_CHAN_RX_RING_LEN: usize = 0x1130;
const DMA_CHAN_INTR_ENA: usize = 0x1134;
const DMA_CHAN_CUR_TX_DESC: usize = 0x1144;
const DMA_CHAN_CUR_RX_DESC: usize = 0x114c;
const DMA_CHAN_STATUS: usize = 0x1160;
const MTL_TXQ0_DEBUG: usize = 0x0d08;

// GMAC_CONFIG bit definitions.
const GMAC_CONFIG_PS: u32 = 1 << 15;
const GMAC_CONFIG_FES: u32 = 1 << 14;
const GMAC_CONFIG_DM: u32 = 1 << 13;
const GMAC_CONFIG_BE: u32 = 1 << 18;
const GMAC_CONFIG_JD: u32 = 1 << 17;
const GMAC_CONFIG_JE: u32 = 1 << 16;
const GMAC_CONFIG_DCRS: u32 = 1 << 9;
const GMAC_CONFIG_TE: u32 = 1 << 1;
const GMAC_CONFIG_RE: u32 = 1 << 0;
const GMAC_CORE_INIT: u32 =
    GMAC_CONFIG_JD | GMAC_CONFIG_PS | GMAC_CONFIG_BE | GMAC_CONFIG_DCRS | GMAC_CONFIG_JE;
// DMA control bit definitions.
const DMA_SOFT_RESET: u32 = 1 << 0;
const DMA_CONTROL_OSP: u32 = 1 << 4;
const DMA_CONTROL_START: u32 = 1 << 0;
const DMA_CONTROL_DSL_SHIFT: u32 = 18;
const DMA_RX_BUF_SIZE_SHIFT: u32 = 1;
const DMA_PBL_SHIFT: u32 = 16;
const DMA_SYS_BUS_WR_OSR_SHIFT: u32 = 24;
const DMA_SYS_BUS_EAME: u32 = 1 << 11;
const DMA_SYS_BUS_RD_OSR_SHIFT: u32 = 16;
const DMA_SYS_BUS_BLEN32: u32 = 1 << 4;
const DMA_SYS_BUS_BLEN64: u32 = 1 << 5;
const DMA_SYS_BUS_BLEN128: u32 = 1 << 6;
const DMA_SYS_BUS_BLEN256: u32 = 1 << 7;
const DMA_SYS_BUS_FIXED_BURST: u32 = 1 << 0;

// MTL queue configuration bits.
const MTL_TX_QUEUE_ENABLE: u32 = 1 << 3;
const MTL_TX_THRESHOLD_64: u32 = 1 << 4;
const MTL_TX_QUEUE_SIZE_SHIFT: u32 = 16;
const MTL_RX_QUEUE_SIZE_SHIFT: u32 = 20;

// MDIO command fields.
const MDIO_BUSY: u32 = 1 << 0;
const MDIO_WRITE: u32 = 1 << 2;
const MDIO_READ: u32 = 3 << 2;
const MDIO_CLOCK_DIV_102: u32 = 4 << 8;
const MDIO_REG_SHIFT: u32 = 16;
const MDIO_PHY_SHIFT: u32 = 21;

// Standard MII/YT8531 PHY registers and bit definitions.
const PHY_ADDR: u8 = 0;
const MII_BMCR: u8 = 0;
const MII_BMSR: u8 = 1;
const MII_PHYSID1: u8 = 2;
const MII_PHYSID2: u8 = 3;
const MII_ADVERTISE: u8 = 4;
const MII_CTRL1000: u8 = 9;
const MII_SPEC_STATUS: u8 = 0x11;
const MII_EXT_ADDR: u8 = 0x1e;
const MII_EXT_DATA: u8 = 0x1f;
const BMCR_ANENABLE: u16 = 1 << 12;
const BMCR_ISOLATE: u16 = 1 << 10;
const BMCR_ANRESTART: u16 = 1 << 9;
const BMSR_LINK: u16 = 1 << 2;
const YT8531_ID: u32 = 0x4f51_e91b;
const YT8531_CHIP_CONFIG: u16 = 0xa001;
const YT8531_RGMII_CONFIG1: u16 = 0xa003;
const YT8531_PAD_DRIVE_CONFIG: u16 = 0xa010;
const YT8531_RXC_DELAY_ENABLE: u16 = 1 << 8;
const YT8531_TX_CLOCK_INVERTED: u16 = 1 << 14;
const YT8531_RX_DELAY_MASK: u16 = 0xf << 10;
const YT8531_FE_TX_DELAY_MASK: u16 = 0xf << 4;
const YT8531_GE_TX_DELAY_MASK: u16 = 0xf;
const YT8531_RGMII_SW_DR_2_MASK: u16 = 1 << 12;
const YT8531_RGMII_SW_DR_MASK: u16 = 0x3 << 4;
const YT8531_RGMII_RXC_DR_MASK: u16 = 0x7 << 13;
// Values from the GMAC0 PHY node in StarFive's VisionFive 2 device tree.
const VF2_GMAC0_RX_DELAY: u16 = 0xa << 10;
const VF2_GMAC0_FE_TX_DELAY: u16 = 5 << 4;
const VF2_GMAC0_GE_TX_DELAY: u16 = 0xa;
const VF2_GMAC0_RGMII_SW_DR: u16 = 0x3 << 4;
const VF2_GMAC0_RGMII_RXC_DR: u16 = 0x6 << 13;

// DMA descriptor status/control bits.
const DESC_OWN: u32 = 1 << 31;
const DESC_TX_FIRST: u32 = 1 << 29;
const DESC_TX_LAST: u32 = 1 << 28;
const DESC_TX_ERROR_SUMMARY: u32 = 1 << 15;
const DESC_TX_IP_HEADER_ERROR: u32 = 1 << 0;
const DESC_TX_DEFERRED: u32 = 1 << 1;
const DESC_TX_UNDERFLOW_ERROR: u32 = 1 << 2;
const DESC_TX_EXCESSIVE_DEFERRAL: u32 = 1 << 3;
const DESC_TX_COLLISION_COUNT_MASK: u32 = 0xf << 4;
const DESC_TX_EXCESSIVE_COLLISION: u32 = 1 << 8;
const DESC_TX_LATE_COLLISION: u32 = 1 << 9;
const DESC_TX_NO_CARRIER: u32 = 1 << 10;
const DESC_TX_LOSS_CARRIER: u32 = 1 << 11;
const DESC_TX_PAYLOAD_ERROR: u32 = 1 << 12;
const DESC_TX_PACKET_FLUSHED: u32 = 1 << 13;
const DESC_TX_JABBER_TIMEOUT: u32 = 1 << 14;
const DESC_RX_FIRST: u32 = 1 << 29;
const DESC_RX_LAST: u32 = 1 << 28;
const DESC_RX_BUF1_VALID: u32 = 1 << 24;
const DESC_RX_ERROR: u32 = 1 << 15;
const DESC_LEN_MASK: u32 = 0x7fff;

// DMA ring and buffer sizing.
const RX_RING_SIZE: usize = 64;
const TX_RING_SIZE: usize = 4;
const DMA_BUF_SIZE: usize = 2048;
const CACHE_LINE_SIZE: usize = 64;
const CCACHE_FLUSH64: usize = 0x200;
const ETHERNET_MIN_FRAME: usize = 60;
const ETHERNET_MAX_FRAME: usize = 1518;
const LINK_POLL_INTERVAL_SECS: u64 = 1;
const TX_COMPLETION_POLLS: usize = 100_000;
const DMA_STATUS_RX_BUF_UNAVAILABLE: u32 = 1 << 7;
const DMA_STATUS_RX_PROCESS_STOPPED: u32 = 1 << 8;
const AXI_BUS_WIDTH: usize = 8;
const DMA_DESCRIPTOR_STRIDE: usize = CACHE_LINE_SIZE;
const DMA_DESCRIPTOR_SKIP_WORDS: u32 =
    ((DMA_DESCRIPTOR_STRIDE - size_of::<DmaDesc>()) / AXI_BUS_WIDTH) as u32;

/// TX status bits that are sticky and should be cleared before waking TX DMA。
const DMA_STATUS_TX_MASK: u32 = (1 << 14) // abnormal interrupt summary
    | (1 << 12) // fatal bus error
    | (1 << 10) // early transmit interrupt
    | (1 << 2) // transmit buffer unavailable
    | (1 << 1) // transmit process stopped
    | (1 << 0); // transmit interrupt

#[repr(C)]
#[derive(Clone, Copy)]
/// Synopsys enhanced DMA descriptor。
struct DmaDesc {
    des0: u32,
    des1: u32,
    des2: u32,
    des3: u32,
}

#[repr(C, align(64))]
/// 一个 cache-line 对齐的 descriptor slot。
///
/// DWMAC 支持 descriptor skip length，这里把每个 descriptor 扩展到 64 字节，
/// 降低 cache line 共享带来的同步风险。
struct DmaDescSlot {
    desc: DmaDesc,
    padding: [u8; DMA_DESCRIPTOR_STRIDE - size_of::<DmaDesc>()],
}

#[repr(C, align(64))]
/// 固定大小的 descriptor ring。
struct DescriptorRing<const N: usize> {
    slots: [DmaDescSlot; N],
}

#[repr(C, align(64))]
/// 固定大小的 DMA packet buffers。
struct DmaBuffers<const N: usize> {
    bytes: [[u8; DMA_BUF_SIZE]; N],
}

#[repr(C, align(64))]
/// DWMAC 使用的全部 DMA 内存。
struct DmaMemory {
    rx_ring: DescriptorRing<RX_RING_SIZE>,
    tx_ring: DescriptorRing<TX_RING_SIZE>,
    rx_buffers: DmaBuffers<RX_RING_SIZE>,
    tx_buffers: DmaBuffers<TX_RING_SIZE>,
}

/// 驱动运行态。
struct DwmacState {
    /// descriptor rings 和 packet buffers。
    dma: Box<DmaMemory>,
    /// 下一次轮询的 RX descriptor 下标。
    rx_index: usize,
    /// 下一次使用的 TX descriptor 下标。
    tx_index: usize,
    /// 已提交的 TX 包计数，仅用于日志和诊断。
    tx_submitted: u64,
    /// RX 丢包计数。
    rx_dropped: u64,
    /// RX buffer unavailable 恢复次数。
    rx_rbu_recoveries: u64,
    /// 上一次轮询 PHY 链路状态的 tick。
    last_link_poll: u64,
    /// 当前链路是否 up。
    link_up: bool,
    /// 当前链路速率/双工模式。
    link_mode: Option<LinkMode>,
}

/// PHY 报告的链路模式。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LinkMode {
    /// 链路速率，单位 Mbps。
    speed: u16,
    /// 是否全双工。
    full_duplex: bool,
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

/// JH7110 GMAC0 network device.
pub struct DwmacDevice {
    /// 设备名。
    name: String,
    /// MAC 地址。
    mac: [u8; 6],
    /// IPv4 地址。
    ip: u32,
    /// DMA ring、索引和链路状态。
    state: Mutex<DwmacState>,
    /// RX 轮询重入保护。
    rx_polling: AtomicBool,
    /// 上层协议栈注册的 RX handler。
    rx_handler: Mutex<Option<Arc<dyn Fn(Skb) + Send + Sync>>>,
}

impl DwmacDevice {
    /// Initialize clocks, PHY, MAC and polling DMA rings for GMAC0.
    pub fn probe(name: &str, ip: u32) -> Result<Self, &'static str> {
        platform_enable()?;

        let version = mmio_read(GMAC_PHYS, GMAC_VERSION);
        if version == 0 || version == u32::MAX {
            return Err("DWMAC registers are not responding");
        }

        let inherited_mac = read_mac_address();
        dma_reset()?;
        let mac = inherited_mac.unwrap_or([0x02, 0x4b, 0x41, 0x49, 0x52, 0x58]);

        let phy_id = phy_initialize()?;
        let mut dma = alloc_dma_memory()?;
        initialize_descriptors(&mut dma)?;
        configure_mac_dma(&mut dma, mac)?;

        log::info!(
            "DWMAC: version={:#010x}, PHY={:#010x}, MAC={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            version,
            phy_id,
            mac[0],
            mac[1],
            mac[2],
            mac[3],
            mac[4],
            mac[5]
        );
        Ok(Self {
            name: String::from(name),
            mac,
            ip,
            state: Mutex::new(DwmacState {
                dma,
                rx_index: 0,
                tx_index: 0,
                tx_submitted: 0,
                rx_dropped: 0,
                rx_rbu_recoveries: 0,
                last_link_poll: 0,
                link_up: false,
                link_mode: None,
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
        let index = state.tx_index;
        let desc_ptr = &mut state.dma.tx_ring.slots[index].desc as *mut DmaDesc;
        dma_sync_range(desc_ptr.cast(), size_of::<DmaDesc>());
        let des3 = unsafe { read_volatile(core::ptr::addr_of!((*desc_ptr).des3)) };
        if des3 & DESC_OWN != 0 {
            log::error!(
                "DWMAC TX busy: index={} desc3={:#010x} tail={:#010x} cur={:#010x} status={:#010x} dma_debug={:#010x} mtl_debug={:#010x}",
                index,
                des3,
                mmio_read(GMAC_PHYS, DMA_CHAN_TX_TAIL),
                mmio_read(GMAC_PHYS, DMA_CHAN_CUR_TX_DESC),
                mmio_read(GMAC_PHYS, DMA_CHAN_STATUS),
                mmio_read(GMAC_PHYS, DMA_DEBUG_STATUS0),
                mmio_read(GMAC_PHYS, MTL_TXQ0_DEBUG),
            );
            return Err(XmitError::Busy.into());
        }

        let frame_len = payload.len().max(ETHERNET_MIN_FRAME);
        let buffer = &mut state.dma.tx_buffers.bytes[index];
        buffer[..payload.len()].copy_from_slice(payload);
        buffer[payload.len()..frame_len].fill(0);
        let buffer_pa = virt_to_phys(buffer.as_ptr() as usize);
        dma_sync_range(buffer.as_ptr(), frame_len);

        unsafe {
            write_volatile(core::ptr::addr_of_mut!((*desc_ptr).des0), buffer_pa as u32);
            write_volatile(
                core::ptr::addr_of_mut!((*desc_ptr).des1),
                (buffer_pa >> 32) as u32,
            );
            write_volatile(core::ptr::addr_of_mut!((*desc_ptr).des2), frame_len as u32);
            write_volatile(
                core::ptr::addr_of_mut!((*desc_ptr).des3),
                DESC_OWN | DESC_TX_FIRST | DESC_TX_LAST | frame_len as u32,
            );
        }
        dma_sync_range(desc_ptr.cast(), size_of::<DmaDesc>());

        state.tx_index = (index + 1) % TX_RING_SIZE;
        let next_pa = virt_to_phys(state.dma.tx_ring.slots.as_ptr() as usize)
            + (state.tx_index * DMA_DESCRIPTOR_STRIDE) as u64;
        // Clear sticky TX status before waking a channel that may be suspended
        // at the current tail pointer.
        mmio_write(GMAC_PHYS, DMA_CHAN_STATUS, DMA_STATUS_TX_MASK);
        mmio_write(GMAC_PHYS, DMA_CHAN_TX_TAIL, next_pa as u32);
        state.tx_submitted += 1;
        let sequence = state.tx_submitted;

        let mut writeback = None;
        for _ in 0..TX_COMPLETION_POLLS {
            dma_sync_range(desc_ptr.cast(), size_of::<DmaDesc>());
            let des3 = unsafe { read_volatile(core::ptr::addr_of!((*desc_ptr).des3)) };
            if des3 & DESC_OWN == 0 {
                writeback = Some(des3);
                break;
            }
            core::hint::spin_loop();
        }

        match writeback {
            Some(des3) => {
                if des3 & DESC_TX_ERROR_SUMMARY != 0 {
                    log::error!(
                        "DWMAC TX error: seq={} desc3={:#010x} status={:#010x} mtl_debug={:#010x} ip_header={} deferred={} underflow={} excessive_deferral={} collisions={} excessive_collision={} late_collision={} no_carrier={} loss_carrier={} payload={} flushed={} jabber={}",
                        sequence,
                        des3,
                        mmio_read(GMAC_PHYS, DMA_CHAN_STATUS),
                        mmio_read(GMAC_PHYS, MTL_TXQ0_DEBUG),
                        des3 & DESC_TX_IP_HEADER_ERROR != 0,
                        des3 & DESC_TX_DEFERRED != 0,
                        des3 & DESC_TX_UNDERFLOW_ERROR != 0,
                        des3 & DESC_TX_EXCESSIVE_DEFERRAL != 0,
                        (des3 & DESC_TX_COLLISION_COUNT_MASK) >> 4,
                        des3 & DESC_TX_EXCESSIVE_COLLISION != 0,
                        des3 & DESC_TX_LATE_COLLISION != 0,
                        des3 & DESC_TX_NO_CARRIER != 0,
                        des3 & DESC_TX_LOSS_CARRIER != 0,
                        des3 & DESC_TX_PAYLOAD_ERROR != 0,
                        des3 & DESC_TX_PACKET_FLUSHED != 0,
                        des3 & DESC_TX_JABBER_TIMEOUT != 0,
                    );
                    return Err("DWMAC transmit error");
                }
            }
            None => {
                log::error!(
                    "DWMAC TX timeout: seq={} index={} cur={:#010x} status={:#010x} dma_debug={:#010x} mtl_debug={:#010x}",
                    sequence,
                    index,
                    mmio_read(GMAC_PHYS, DMA_CHAN_CUR_TX_DESC),
                    mmio_read(GMAC_PHYS, DMA_CHAN_STATUS),
                    mmio_read(GMAC_PHYS, DMA_DEBUG_STATUS0),
                    mmio_read(GMAC_PHYS, MTL_TXQ0_DEBUG),
                );
                return Err("DWMAC transmit completion timeout");
            }
        }
        Ok((skb, 0, 0))
    }

    /// 定期读取 PHY 状态并按链路模式更新 MAC 配置。
    fn poll_link(&self, state: &mut DwmacState) {
        let now = polyhal::timer::get_ticks();
        let interval = polyhal::timer::get_freq().saturating_mul(LINK_POLL_INTERVAL_SECS);
        if state.last_link_poll != 0 && now.wrapping_sub(state.last_link_poll) < interval {
            return;
        }
        state.last_link_poll = now;

        let status = mdio_read(PHY_ADDR, MII_BMSR)
            .and_then(|_| mdio_read(PHY_ADDR, MII_BMSR))
            .unwrap_or(0);
        if status & BMSR_LINK == 0 {
            if state.link_up {
                log::info!("DWMAC: end0 link down");
            }
            state.link_up = false;
            state.link_mode = None;
            return;
        }

        let specific = match mdio_read(PHY_ADDR, MII_SPEC_STATUS) {
            Ok(value) => value,
            Err(error) => {
                log::warn!("DWMAC: failed to read YT8531 link mode: {}", error);
                return;
            }
        };
        let speed = match (specific >> 14) & 0x3 {
            0 => 10,
            1 => 100,
            2 => 1000,
            _ => {
                log::warn!("DWMAC: unsupported YT8531 status {:#06x}", specific);
                return;
            }
        };
        let mode = LinkMode {
            speed,
            full_duplex: specific & (1 << 13) != 0,
        };

        if state.link_mode != Some(mode) {
            if let Err(error) = configure_mac_link(mode) {
                log::error!("DWMAC: unsupported link mode: {}", error);
                return;
            }
            log::info!(
                "DWMAC: end0 link up (YT8531 status={:#06x}, {}Mbps/{})",
                specific,
                mode.speed,
                if mode.full_duplex { "full" } else { "half" },
            );
            state.link_mode = Some(mode);
        }
        state.link_up = true;
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
        let _poll_guard = RxPollGuard {
            active: &self.rx_polling,
        };

        loop {
            let skb = {
                let Some(mut state) = self.state.try_lock() else {
                    return;
                };
                self.poll_link(&mut state);

                let index = state.rx_index;
                let desc_ptr = &mut state.dma.rx_ring.slots[index].desc as *mut DmaDesc;
                dma_sync_range(desc_ptr.cast(), size_of::<DmaDesc>());
                let des3 = unsafe { read_volatile(core::ptr::addr_of!((*desc_ptr).des3)) };
                if des3 & DESC_OWN != 0 {
                    let status = mmio_read(GMAC_PHYS, DMA_CHAN_STATUS);
                    if status & DMA_STATUS_RX_BUF_UNAVAILABLE != 0 {
                        let ring_pa = virt_to_phys(state.dma.rx_ring.slots.as_ptr() as usize);
                        let tail_pa = ring_pa + (state.rx_index * DMA_DESCRIPTOR_STRIDE) as u64;
                        let current_before = mmio_read(GMAC_PHYS, DMA_CHAN_CUR_RX_DESC);
                        let tail_before = mmio_read(GMAC_PHYS, DMA_CHAN_RX_TAIL);
                        let control_before = mmio_read(GMAC_PHYS, DMA_CHAN_RX_CONTROL);

                        // A tail write is the documented DWMAC4/5 RBU resume
                        // operation. Reassert SR as a defensive recovery if the
                        // channel also reported that its receive process stopped.
                        mmio_write(
                            GMAC_PHYS,
                            DMA_CHAN_STATUS,
                            DMA_STATUS_RX_BUF_UNAVAILABLE | DMA_STATUS_RX_PROCESS_STOPPED,
                        );
                        mmio_write(GMAC_PHYS, DMA_CHAN_RX_TAIL, tail_pa as u32);
                        mmio_rmw(GMAC_PHYS, DMA_CHAN_RX_CONTROL, 0, DMA_CONTROL_START);
                        state.rx_rbu_recoveries += 1;
                        if state.rx_rbu_recoveries <= 4 || state.rx_rbu_recoveries.is_power_of_two()
                        {
                            log::warn!(
                                "DWMAC RX resumed after RBU: count={} index={} status={:#010x} cur={:#010x} tail={:#010x}->{:#010x} control={:#010x}->{:#010x} debug={:#010x}",
                                state.rx_rbu_recoveries,
                                index,
                                status,
                                current_before,
                                tail_before,
                                tail_pa as u32,
                                control_before,
                                mmio_read(GMAC_PHYS, DMA_CHAN_RX_CONTROL),
                                mmio_read(GMAC_PHYS, DMA_DEBUG_STATUS0),
                            );
                        }
                    }
                    return;
                }

                let raw_len = (des3 & DESC_LEN_MASK) as usize;
                let valid = des3 & (DESC_RX_FIRST | DESC_RX_LAST) == (DESC_RX_FIRST | DESC_RX_LAST)
                    && des3 & DESC_RX_ERROR == 0
                    && raw_len >= 4
                    && raw_len <= DMA_BUF_SIZE;
                let packet_len = raw_len.saturating_sub(4);
                let mut skb = None;
                if valid && packet_len <= ETHERNET_MAX_FRAME {
                    let buffer = &state.dma.rx_buffers.bytes[index];
                    dma_sync_range(buffer.as_ptr(), raw_len);
                    let mut packet = Skb::new(packet_len);
                    if let Some(dst) = packet.put(packet_len) {
                        dst.copy_from_slice(&buffer[..packet_len]);
                        skb = Some(packet);
                    }
                } else {
                    state.rx_dropped += 1;
                    if state.rx_dropped <= 4 || state.rx_dropped.is_power_of_two() {
                        log::warn!(
                            "DWMAC RX drop: count={} index={} raw_len={} status={:#010x}",
                            state.rx_dropped,
                            index,
                            raw_len,
                            des3,
                        );
                    }
                }

                rearm_rx_descriptor(&mut state.dma, index);
                state.rx_index = (index + 1) % RX_RING_SIZE;
                let tail_pa = virt_to_phys(state.dma.rx_ring.slots.as_ptr() as usize)
                    + (state.rx_index * DMA_DESCRIPTOR_STRIDE) as u64;
                mmio_write(GMAC_PHYS, DMA_CHAN_RX_TAIL, tail_pa as u32);
                skb
            };

            if let Some(skb) = skb {
                let handler = self.rx_handler.lock().clone();
                if let Some(handler) = handler {
                    handler(skb);
                }
            }
        }
    }
}

impl NetDevice for DwmacDevice {
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

/// 打开 JH7110 GMAC0 所需时钟、复位和 RGMII pin/clock 配置。
fn platform_enable() -> Result<(), &'static str> {
    const CLK_ENABLE: u32 = 1 << 31;
    const SYS_GMAC0_GTX: usize = 0x1b0;
    const SYS_GMAC0_GTXC: usize = 0x1bc;
    const AON_GMAC0_AHB: usize = 0x08;
    const AON_GMAC0_AXI: usize = 0x0c;
    const AON_GMAC0_RMII_RTX: usize = 0x10;
    const AON_GMAC0_TX: usize = 0x14;
    const AON_GMAC0_TX_INV: usize = 0x18;
    const AON_GMAC0_RX: usize = 0x1c;
    const AON_GMAC0_RX_INV: usize = 0x20;
    const AON_RESET_ASSERT: usize = 0x38;
    const AON_RESET_STATUS: usize = 0x3c;
    const AON_GMAC0_PHY_INTF: usize = 0x0c;
    const GMAC0_RESET_MASK: u32 = (1 << 0) | (1 << 1);
    const CLOCK_MUX_MASK: u32 = 1 << 24;
    const CLOCK_DIV_MASK: u32 = 0x0f;
    const PHY_INTF_SHIFT: u32 = 18;
    const PHY_INTF_MASK: u32 = 0x7 << PHY_INTF_SHIFT;
    const PHY_INTF_RGMII: u32 = 1 << PHY_INTF_SHIFT;
    let gtx_before = mmio_read(SYS_CRG_PHYS, SYS_GMAC0_GTX);
    let aon_before = mmio_read(AON_CRG_PHYS, AON_GMAC0_AHB);
    let rmii_rtx_before = mmio_read(AON_CRG_PHYS, AON_GMAC0_RMII_RTX);
    let tx_before = mmio_read(AON_CRG_PHYS, AON_GMAC0_TX);
    let tx_inv_before = mmio_read(AON_CRG_PHYS, AON_GMAC0_TX_INV);
    let rx_before = mmio_read(AON_CRG_PHYS, AON_GMAC0_RX);
    let rx_inv_before = mmio_read(AON_CRG_PHYS, AON_GMAC0_RX_INV);
    let intf_before = mmio_read(AON_SYSCON_PHYS, AON_GMAC0_PHY_INTF);

    // OpenSBI/U-Boot already selected the board-specific PLL parent and GTX
    // divider. Preserve that known-good rate; guessing a divider here can
    // silently produce an invalid RGMII clock.
    let gtx_div = gtx_before & CLOCK_DIV_MASK;
    if gtx_div == 0 {
        return Err("GMAC0 GTX clock divider was not configured by firmware");
    }
    mmio_rmw(
        SYS_CRG_PHYS,
        SYS_GMAC0_GTX,
        CLOCK_DIV_MASK,
        CLK_ENABLE | gtx_div,
    );
    mmio_rmw(SYS_CRG_PHYS, SYS_GMAC0_GTXC, 0, CLK_ENABLE);
    mmio_rmw(AON_CRG_PHYS, AON_GMAC0_AHB, 0, CLK_ENABLE);
    mmio_rmw(AON_CRG_PHYS, AON_GMAC0_AXI, 0, CLK_ENABLE);
    // VisionFive 2 v1.3B supplies the RGMII TX clock externally. Its board
    // DTS assigns gmac0_tx to the RMII_RTX parent and sets
    // starfive,tx-use-rgmii-clk, so enable parent 1 instead of replacing the
    // firmware selection with the internal GTX clock.
    mmio_rmw(
        AON_CRG_PHYS,
        AON_GMAC0_TX,
        CLOCK_MUX_MASK,
        CLK_ENABLE | CLOCK_MUX_MASK,
    );
    // The RX mux uses the same encoding: parent 0 is the external RGMII RX
    // clock from the PHY. Mainline's StarFive glue also programs the AON
    // syscon PHY interface field to RGMII (value 1 at shift 18).
    mmio_rmw(AON_CRG_PHYS, AON_GMAC0_RX, CLOCK_MUX_MASK, 0);
    // Mainline exposes TX_INV as GMAC0's "tx" clock while RX_INV remains a
    // separate phase-control clock. Preserve both phases established by the
    // board firmware; the parent selection itself lives in gmac0_tx above.
    mmio_rmw(
        AON_SYSCON_PHYS,
        AON_GMAC0_PHY_INTF,
        PHY_INTF_MASK,
        PHY_INTF_RGMII,
    );

    mmio_rmw(AON_CRG_PHYS, AON_RESET_ASSERT, GMAC0_RESET_MASK, 0);
    for _ in 0..100_000 {
        // The JH7110 reset status register reports a set bit when the
        // corresponding reset line has been deasserted.
        let reset_status = mmio_read(AON_CRG_PHYS, AON_RESET_STATUS);
        if reset_status & GMAC0_RESET_MASK == GMAC0_RESET_MASK {
            log::info!(
                "DWMAC clocks: GTX {:#010x}->{:#010x}, AHB {:#010x}->{:#010x}, RMII_RTX={:#010x}->{:#010x}, TX {:#010x}->{:#010x}, TX_INV={:#010x}->{:#010x}, RX {:#010x}->{:#010x}, RX_INV={:#010x}->{:#010x}, INTF {:#010x}->{:#010x}, reset={:#010x}",
                gtx_before,
                mmio_read(SYS_CRG_PHYS, SYS_GMAC0_GTX),
                aon_before,
                mmio_read(AON_CRG_PHYS, AON_GMAC0_AHB),
                rmii_rtx_before,
                mmio_read(AON_CRG_PHYS, AON_GMAC0_RMII_RTX),
                tx_before,
                mmio_read(AON_CRG_PHYS, AON_GMAC0_TX),
                tx_inv_before,
                mmio_read(AON_CRG_PHYS, AON_GMAC0_TX_INV),
                rx_before,
                mmio_read(AON_CRG_PHYS, AON_GMAC0_RX),
                rx_inv_before,
                mmio_read(AON_CRG_PHYS, AON_GMAC0_RX_INV),
                intf_before,
                mmio_read(AON_SYSCON_PHYS, AON_GMAC0_PHY_INTF),
                reset_status
            );
            return Ok(());
        }
        core::hint::spin_loop();
    }
    log::error!(
        "DWMAC reset timeout: assert={:#010x}, status={:#010x}",
        mmio_read(AON_CRG_PHYS, AON_RESET_ASSERT),
        mmio_read(AON_CRG_PHYS, AON_RESET_STATUS)
    );
    Err("GMAC0 reset did not deassert")
}

/// 复位 DWMAC DMA engine。
fn dma_reset() -> Result<(), &'static str> {
    mmio_rmw(GMAC_PHYS, DMA_MODE, 0, DMA_SOFT_RESET);
    for _ in 0..1_000_000 {
        if mmio_read(GMAC_PHYS, DMA_MODE) & DMA_SOFT_RESET == 0 {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err("DWMAC DMA reset timed out")
}

/// 初始化 YT8531 PHY，并套用 VisionFive 2 设备树中的 RGMII delay/drive 配置。
fn phy_initialize() -> Result<u32, &'static str> {
    let phy_id = ((mdio_read(PHY_ADDR, MII_PHYSID1)? as u32) << 16)
        | mdio_read(PHY_ADDR, MII_PHYSID2)? as u32;
    if phy_id == 0 || phy_id == u32::MAX {
        return Err("no PHY responded at MDIO address 0");
    }
    if phy_id & 0xffff_fff0 != YT8531_ID & 0xffff_fff0 {
        log::warn!("DWMAC: unexpected PHY ID {:#010x}", phy_id);
    }

    let bmcr_before = mdio_read(PHY_ADDR, MII_BMCR)?;
    let chip_before = phy_ext_read(YT8531_CHIP_CONFIG)?;
    let rgmii_before = phy_ext_read(YT8531_RGMII_CONFIG1)?;
    let drive_before = phy_ext_read(YT8531_PAD_DRIVE_CONFIG)?;
    let advertise = mdio_read(PHY_ADDR, MII_ADVERTISE)?;
    let ctrl1000 = mdio_read(PHY_ADDR, MII_CTRL1000)?;

    // Match StarFive's YT8531 config_init path and only replace fields that
    // are explicitly present in the VisionFive 2 GMAC0 PHY node.
    let chip = chip_before & !YT8531_RXC_DELAY_ENABLE;
    phy_ext_write(YT8531_CHIP_CONFIG, chip)?;

    let rgmii_mask = YT8531_TX_CLOCK_INVERTED
        | YT8531_RX_DELAY_MASK
        | YT8531_FE_TX_DELAY_MASK
        | YT8531_GE_TX_DELAY_MASK;
    let rgmii = (rgmii_before & !rgmii_mask)
        | YT8531_TX_CLOCK_INVERTED
        | VF2_GMAC0_RX_DELAY
        | VF2_GMAC0_FE_TX_DELAY
        | VF2_GMAC0_GE_TX_DELAY;
    phy_ext_write(YT8531_RGMII_CONFIG1, rgmii)?;

    let drive_mask = YT8531_RGMII_SW_DR_2_MASK | YT8531_RGMII_SW_DR_MASK | YT8531_RGMII_RXC_DR_MASK;
    let drive = (drive_before & !drive_mask) | VF2_GMAC0_RGMII_SW_DR | VF2_GMAC0_RGMII_RXC_DR;
    phy_ext_write(YT8531_PAD_DRIVE_CONFIG, drive)?;

    log::info!(
        "DWMAC PHY config (VF2 external TX clock): bmcr={:#06x}, chip={:#06x}->{:#06x}, rgmii={:#06x}->{:#06x}, drive={:#06x}->{:#06x}, advertise={:#06x} preserved, ctrl1000={:#06x} preserved",
        bmcr_before,
        chip_before,
        phy_ext_read(YT8531_CHIP_CONFIG)?,
        rgmii_before,
        phy_ext_read(YT8531_RGMII_CONFIG1)?,
        drive_before,
        phy_ext_read(YT8531_PAD_DRIVE_CONFIG)?,
        advertise,
        ctrl1000,
    );

    mdio_write(
        PHY_ADDR,
        MII_BMCR,
        (bmcr_before & !BMCR_ISOLATE) | BMCR_ANENABLE | BMCR_ANRESTART,
    )?;
    Ok(phy_id)
}

/// 配置 MAC、MTL 队列和 DMA channel。
fn configure_mac_dma(dma: &mut DmaMemory, mac: [u8; 6]) -> Result<(), &'static str> {
    let dma_va = dma as *mut DmaMemory as usize;
    if dma_va < VIRT_ADDR_START {
        return Err("DWMAC DMA memory is outside the kernel direct map");
    }
    let dma_pa = virt_to_phys(dma_va);
    if dma_pa
        .checked_add(VIRT_ADDR_START as u64)
        .map(|round_trip| round_trip as usize)
        != Some(dma_va)
    {
        return Err("DWMAC DMA virtual-to-physical mapping is not linear");
    }

    let rx_ring_pa = virt_to_phys(dma.rx_ring.slots.as_ptr() as usize);
    let tx_ring_pa = virt_to_phys(dma.tx_ring.slots.as_ptr() as usize);
    if rx_ring_pa & 0x3f != 0 || tx_ring_pa & 0x3f != 0 {
        return Err("DWMAC rings are not cache-line aligned");
    }
    let mac_low = (mac[0] as u32)
        | ((mac[1] as u32) << 8)
        | ((mac[2] as u32) << 16)
        | ((mac[3] as u32) << 24);
    let mac_high = (mac[4] as u32) | ((mac[5] as u32) << 8) | (1 << 31);
    mmio_write(GMAC_PHYS, GMAC_ADDR_LOW0, mac_low);
    mmio_write(GMAC_PHYS, GMAC_ADDR_HIGH0, mac_high);
    mmio_write(GMAC_PHYS, GMAC_PACKET_FILTER, 0);
    mmio_write(GMAC_PHYS, GMAC_INT_EN, 0);
    mmio_write(GMAC_PHYS, GMAC_Q0_TX_FLOW_CTRL, 0);
    mmio_write(GMAC_PHYS, GMAC_RX_FLOW_CTRL, 0);
    mmio_write(GMAC_PHYS, GMAC_LPI_CTRL_STATUS, 0);
    mmio_rmw(GMAC_PHYS, GMAC_RXQ_CTRL0, 0x3, 0x2);

    // Match the board DT's forced threshold mode with a 64-byte TX threshold.
    // The FIFO size fields still describe the complete 2 KiB queues.
    mmio_write(
        GMAC_PHYS,
        MTL_TXQ0_OP_MODE,
        MTL_TX_QUEUE_ENABLE | MTL_TX_THRESHOLD_64 | (7 << MTL_TX_QUEUE_SIZE_SHIFT),
    );
    mmio_write(GMAC_PHYS, MTL_RXQ0_OP_MODE, 7 << MTL_RX_QUEUE_SIZE_SHIFT);

    // Reproduce the coherent DMA profile in jh7110-visionfive-v2.dtb:
    // fixed bursts, read/write OSR 15, BLEN 32/64/128/256, no PBLx8 and
    // independent TX/RX PBL values of 16.
    let sys_bus = (15 << DMA_SYS_BUS_WR_OSR_SHIFT)
        | (15 << DMA_SYS_BUS_RD_OSR_SHIFT)
        | DMA_SYS_BUS_EAME
        | DMA_SYS_BUS_BLEN256
        | DMA_SYS_BUS_BLEN128
        | DMA_SYS_BUS_BLEN64
        | DMA_SYS_BUS_BLEN32
        | DMA_SYS_BUS_FIXED_BURST;
    mmio_write(GMAC_PHYS, DMA_SYS_BUS_MODE, sys_bus);
    mmio_write(
        GMAC_PHYS,
        DMA_CHAN_CONTROL,
        DMA_DESCRIPTOR_SKIP_WORDS << DMA_CONTROL_DSL_SHIFT,
    );
    mmio_write(
        GMAC_PHYS,
        DMA_CHAN_TX_CONTROL,
        DMA_CONTROL_OSP | (16 << DMA_PBL_SHIFT),
    );
    mmio_write(
        GMAC_PHYS,
        DMA_CHAN_RX_CONTROL,
        (16 << DMA_PBL_SHIFT) | ((DMA_BUF_SIZE as u32) << DMA_RX_BUF_SIZE_SHIFT),
    );
    mmio_write(GMAC_PHYS, DMA_CHAN_TX_BASE_HI, (tx_ring_pa >> 32) as u32);
    mmio_write(GMAC_PHYS, DMA_CHAN_TX_BASE, tx_ring_pa as u32);
    mmio_write(GMAC_PHYS, DMA_CHAN_RX_BASE_HI, (rx_ring_pa >> 32) as u32);
    mmio_write(GMAC_PHYS, DMA_CHAN_RX_BASE, rx_ring_pa as u32);
    mmio_write(GMAC_PHYS, DMA_CHAN_TX_RING_LEN, (TX_RING_SIZE - 1) as u32);
    mmio_write(GMAC_PHYS, DMA_CHAN_RX_RING_LEN, (RX_RING_SIZE - 1) as u32);
    mmio_write(GMAC_PHYS, DMA_CHAN_INTR_ENA, 0);
    mmio_write(GMAC_PHYS, DMA_CHAN_STATUS, u32::MAX);
    mmio_write(GMAC_PHYS, DMA_CHAN_TX_TAIL, tx_ring_pa as u32);
    mmio_write(
        GMAC_PHYS,
        DMA_CHAN_RX_TAIL,
        (rx_ring_pa + (RX_RING_SIZE * DMA_DESCRIPTOR_STRIDE) as u64) as u32,
    );

    // The inherited GTX clock is 125 MHz, so initialize the MAC in its
    // matching 1000/full GMII mode. Starting with PS set would select the
    // 10/100 MII datapath until the first link poll.
    // A non-gigabit result is reported by poll_link() until clock-rate control
    // is implemented.
    // This is GMAC_CORE_INIT from Linux dwmac4.h, plus the negotiated full
    // duplex default and TX/RX enables. poll_link() updates PS/FES/DM once the
    // PHY reports its final link mode.
    let config =
        (GMAC_CORE_INIT & !GMAC_CONFIG_PS) | GMAC_CONFIG_DM | GMAC_CONFIG_TE | GMAC_CONFIG_RE;
    mmio_write(GMAC_PHYS, GMAC_CONFIG, config);
    mmio_rmw(GMAC_PHYS, DMA_CHAN_TX_CONTROL, 0, DMA_CONTROL_START);
    mmio_rmw(GMAC_PHYS, DMA_CHAN_RX_CONTROL, 0, DMA_CONTROL_START);
    log::info!(
        "DWMAC DMA: RX ring={:#x}, TX ring={:#x}, stride={}, channel={:#010x}, sys_bus={:#010x}, tx={:#010x}, rx={:#010x}, txq={:#010x}, rxq={:#010x}, mac={:#010x}, status={:#010x}",
        rx_ring_pa,
        tx_ring_pa,
        DMA_DESCRIPTOR_STRIDE,
        mmio_read(GMAC_PHYS, DMA_CHAN_CONTROL),
        mmio_read(GMAC_PHYS, DMA_SYS_BUS_MODE),
        mmio_read(GMAC_PHYS, DMA_CHAN_TX_CONTROL),
        mmio_read(GMAC_PHYS, DMA_CHAN_RX_CONTROL),
        mmio_read(GMAC_PHYS, MTL_TXQ0_OP_MODE),
        mmio_read(GMAC_PHYS, MTL_RXQ0_OP_MODE),
        mmio_read(GMAC_PHYS, GMAC_CONFIG),
        mmio_read(GMAC_PHYS, DMA_CHAN_STATUS)
    );
    Ok(())
}

/// 初始化 RX/TX descriptor ring。
fn initialize_descriptors(dma: &mut DmaMemory) -> Result<(), &'static str> {
    if size_of::<DmaDescSlot>() != DMA_DESCRIPTOR_STRIDE || DMA_DESCRIPTOR_SKIP_WORDS > 7 {
        return Err("DWMAC descriptor stride is not representable");
    }
    for index in 0..RX_RING_SIZE {
        rearm_rx_descriptor(dma, index);
    }
    dma_sync_range(
        dma.rx_ring.slots.as_ptr().cast(),
        size_of::<DescriptorRing<RX_RING_SIZE>>(),
    );
    dma_sync_range(
        dma.tx_ring.slots.as_ptr().cast(),
        size_of::<DescriptorRing<TX_RING_SIZE>>(),
    );
    Ok(())
}

/// 根据 PHY 链路模式更新 MAC 速率和双工位。
fn configure_mac_link(mode: LinkMode) -> Result<(), &'static str> {
    // The firmware-configured GTX clock is 125 MHz. Linux changes the
    // JH7110 clock-controller rate before selecting 100/10 Mbps. Kairix does
    // not yet own that clock tree, so only program modes whose wire clock is
    // known to be valid instead of silently using a 125 MHz clock at 100/10.
    if mode.speed != 1000 {
        return Err("100/10 Mbps requires JH7110 GTX clock-rate control");
    }
    let speed_bits = match mode.speed {
        1000 => 0,
        _ => return Err("unknown PHY speed"),
    };
    let duplex = if mode.full_duplex { GMAC_CONFIG_DM } else { 0 };
    mmio_rmw(
        GMAC_PHYS,
        GMAC_CONFIG,
        GMAC_CONFIG_PS | GMAC_CONFIG_FES | GMAC_CONFIG_DM,
        speed_bits | duplex,
    );
    Ok(())
}

/// 重新把一个 RX descriptor 交给 DMA。
fn rearm_rx_descriptor(dma: &mut DmaMemory, index: usize) {
    let buffer_pa = virt_to_phys(dma.rx_buffers.bytes[index].as_ptr() as usize);
    let desc_ptr = &mut dma.rx_ring.slots[index].desc as *mut DmaDesc;
    dma_sync_range(dma.rx_buffers.bytes[index].as_ptr(), DMA_BUF_SIZE);
    unsafe {
        write_volatile(core::ptr::addr_of_mut!((*desc_ptr).des0), buffer_pa as u32);
        write_volatile(
            core::ptr::addr_of_mut!((*desc_ptr).des1),
            (buffer_pa >> 32) as u32,
        );
        write_volatile(core::ptr::addr_of_mut!((*desc_ptr).des2), 0);
        write_volatile(
            core::ptr::addr_of_mut!((*desc_ptr).des3),
            DESC_OWN | DESC_RX_BUF1_VALID,
        );
    }
    dma_sync_range(desc_ptr.cast(), size_of::<DmaDesc>());
}

/// 分配 cache-line 对齐的 DMA 内存。
fn alloc_dma_memory() -> Result<Box<DmaMemory>, &'static str> {
    let layout = Layout::from_size_align(size_of::<DmaMemory>(), align_of::<DmaMemory>())
        .map_err(|_| "invalid DWMAC DMA layout")?;
    let ptr = unsafe { alloc_zeroed(layout) } as *mut DmaMemory;
    if ptr.is_null() {
        return Err("failed to allocate DWMAC DMA memory");
    }
    Ok(unsafe { Box::from_raw(ptr) })
}

/// 从 MAC 地址寄存器读取 firmware 预置的 MAC。
fn read_mac_address() -> Option<[u8; 6]> {
    let low = mmio_read(GMAC_PHYS, GMAC_ADDR_LOW0);
    let high = mmio_read(GMAC_PHYS, GMAC_ADDR_HIGH0);
    let mac = [
        low as u8,
        (low >> 8) as u8,
        (low >> 16) as u8,
        (low >> 24) as u8,
        high as u8,
        (high >> 8) as u8,
    ];
    let all_zero = mac.iter().all(|byte| *byte == 0);
    let all_ff = mac.iter().all(|byte| *byte == 0xff);
    if !all_zero && !all_ff && mac[0] & 1 == 0 {
        Some(mac)
    } else {
        None
    }
}

/// 读取 YT8531 扩展寄存器。
fn phy_ext_read(reg: u16) -> Result<u16, &'static str> {
    mdio_write(PHY_ADDR, MII_EXT_ADDR, reg)?;
    mdio_read(PHY_ADDR, MII_EXT_DATA)
}

/// 写入 YT8531 扩展寄存器。
fn phy_ext_write(reg: u16, value: u16) -> Result<(), &'static str> {
    mdio_write(PHY_ADDR, MII_EXT_ADDR, reg)?;
    mdio_write(PHY_ADDR, MII_EXT_DATA, value)
}

/// 通过 MDIO 读取 PHY 寄存器。
fn mdio_read(phy: u8, reg: u8) -> Result<u16, &'static str> {
    mdio_wait_idle()?;
    let command = ((phy as u32) << MDIO_PHY_SHIFT)
        | ((reg as u32) << MDIO_REG_SHIFT)
        | MDIO_CLOCK_DIV_102
        | MDIO_READ
        | MDIO_BUSY;
    mmio_write(GMAC_PHYS, GMAC_MDIO_ADDR, command);
    mdio_wait_idle()?;
    Ok(mmio_read(GMAC_PHYS, GMAC_MDIO_DATA) as u16)
}

/// 通过 MDIO 写入 PHY 寄存器。
fn mdio_write(phy: u8, reg: u8, value: u16) -> Result<(), &'static str> {
    mdio_wait_idle()?;
    mmio_write(GMAC_PHYS, GMAC_MDIO_DATA, value as u32);
    let command = ((phy as u32) << MDIO_PHY_SHIFT)
        | ((reg as u32) << MDIO_REG_SHIFT)
        | MDIO_CLOCK_DIV_102
        | MDIO_WRITE
        | MDIO_BUSY;
    mmio_write(GMAC_PHYS, GMAC_MDIO_ADDR, command);
    mdio_wait_idle()
}

/// 等待 MDIO 控制器空闲。
fn mdio_wait_idle() -> Result<(), &'static str> {
    for _ in 0..100_000 {
        if mmio_read(GMAC_PHYS, GMAC_MDIO_ADDR) & MDIO_BUSY == 0 {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err("DWMAC MDIO transaction timed out")
}

/// 将内核直映虚拟地址转换为物理地址。
#[inline]
fn virt_to_phys(address: usize) -> u64 {
    if address >= VIRT_ADDR_START {
        (address - VIRT_ADDR_START) as u64
    } else {
        address as u64
    }
}

/// 将指定内存范围同步给 DMA 设备可见。
///
/// JH7110 当前路径依赖显式 cache line flush，描述符和 packet buffer 在交给
/// DMA 前后都通过该函数维护一致性。
fn dma_sync_range(ptr: *const u8, len: usize) {
    if len == 0 {
        return;
    }
    let start = virt_to_phys(ptr as usize) as usize & !(CACHE_LINE_SIZE - 1);
    let end =
        (virt_to_phys(ptr as usize) as usize + len + CACHE_LINE_SIZE - 1) & !(CACHE_LINE_SIZE - 1);
    unsafe {
        // The flush register and the DWMAC tail pointers are I/O accesses.
        // A plain `fence rw,rw` does not order them against cached memory, so
        // DMA could observe an old descriptor or packet buffer.
        core::arch::asm!("fence iorw, iorw", options(nostack, preserves_flags));
        let flush = (CCACHE_PHYS + CCACHE_FLUSH64 + VIRT_ADDR_START) as *mut u64;
        let mut line = start;
        while line < end {
            write_volatile(flush, line as u64);
            line += CACHE_LINE_SIZE;
        }
        core::arch::asm!("fence iorw, iorw", options(nostack, preserves_flags));
    }
}

/// 读取 32 位 MMIO 寄存器。
#[inline]
fn mmio_read(base: usize, offset: usize) -> u32 {
    unsafe { read_volatile((base + offset + VIRT_ADDR_START) as *const u32) }
}

/// 写入 32 位 MMIO 寄存器。
#[inline]
fn mmio_write(base: usize, offset: usize, value: u32) {
    unsafe { write_volatile((base + offset + VIRT_ADDR_START) as *mut u32, value) }
    fence(Ordering::SeqCst);
}

/// 原子式读改写 MMIO 寄存器中的位域。
#[inline]
fn mmio_rmw(base: usize, offset: usize, clear: u32, set: u32) {
    let value = mmio_read(base, offset);
    mmio_write(base, offset, (value & !clear) | set);
}
