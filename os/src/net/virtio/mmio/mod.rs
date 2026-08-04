//! VirtIO MMIO transport。
//!
//! QEMU virt 等平台可能通过 MMIO 区域暴露 VirtIO 设备。本模块扫描固定
//! MMIO 地址窗口，找到 VirtIO-net 后提供统一的寄存器访问接口。

#[cfg(not(target_arch = "loongarch64"))]
use polyhal::consts::VIRT_ADDR_START;

#[cfg(target_arch = "loongarch64")]
const LOONGARCH_UNCACHED_DMW_BASE: usize = 0x8000_0000_0000_0000;

/// 将 MMIO 物理地址转换为内核可访问的虚拟地址。
#[inline]
fn phys_to_mmio_virt(paddr: usize) -> usize {
    #[cfg(target_arch = "loongarch64")]
    {
        return LOONGARCH_UNCACHED_DMW_BASE + paddr;
    }

    #[cfg(not(target_arch = "loongarch64"))]
    {
        paddr + VIRT_ADDR_START
    }
}

/// VirtIO MMIO magic value，ASCII "virt" 小端形式。
const VIRTIO_MMIO_MAGIC: u32 = 0x7472_6976;
/// legacy MMIO transport version。
const VIRTIO_MMIO_VERSION_LEGACY: u32 = 1;
/// modern MMIO transport version。
const VIRTIO_MMIO_VERSION_MODERN: u32 = 2;
/// VirtIO-net MMIO device id。
const VIRTIO_MMIO_DEVICE_ID_NET: u32 = 1;
/// legacy MMIO QueuePFN 使用的 guest page size。
const VIRTIO_MMIO_GUEST_PAGE_SIZE: u32 = 4096;

/// QEMU virt 默认 VirtIO MMIO 设备窗口起始地址。
const VIRTIO_MMIO_BASE_START: usize = 0x1000_1000;
/// 相邻 VirtIO MMIO 设备的地址间隔。
const VIRTIO_MMIO_DEVICE_STRIDE: usize = 0x1000;
/// 扫描的 VirtIO MMIO 设备数量。
const VIRTIO_MMIO_DEVICE_COUNT: usize = 8;

// VirtIO MMIO register offsets.
const MMIO_MAGIC_VALUE: usize = 0x000;
const MMIO_VERSION: usize = 0x004;
const MMIO_DEVICE_ID: usize = 0x008;
const MMIO_DEVICE_FEATURES: usize = 0x010;
const MMIO_DEVICE_FEATURES_SEL: usize = 0x014;
const MMIO_DRIVER_FEATURES: usize = 0x020;
const MMIO_DRIVER_FEATURES_SEL: usize = 0x024;
const MMIO_GUEST_PAGE_SIZE: usize = 0x028;
const MMIO_QUEUE_SEL: usize = 0x030;
const MMIO_QUEUE_NUM_MAX: usize = 0x034;
const MMIO_QUEUE_NUM: usize = 0x038;
const MMIO_QUEUE_ALIGN: usize = 0x03c;
const MMIO_QUEUE_PFN: usize = 0x040;
const MMIO_QUEUE_READY: usize = 0x044;
const MMIO_QUEUE_NOTIFY: usize = 0x050;
const MMIO_STATUS: usize = 0x070;
const MMIO_QUEUE_DESC_LOW: usize = 0x080;
const MMIO_QUEUE_DESC_HIGH: usize = 0x084;
const MMIO_QUEUE_DRIVER_LOW: usize = 0x090;
const MMIO_QUEUE_DRIVER_HIGH: usize = 0x094;
const MMIO_QUEUE_DEVICE_LOW: usize = 0x0a0;
const MMIO_QUEUE_DEVICE_HIGH: usize = 0x0a4;
const MMIO_CONFIG_SPACE: usize = 0x100;

/// 一个 VirtIO MMIO transport 实例。
#[derive(Clone, Copy)]
pub(crate) struct MmioNetTransport {
    /// MMIO 虚拟基址。
    base: *mut u8,
    /// MMIO 物理基址。
    phys_base: usize,
    /// transport version，legacy=1、modern=2。
    version: u32,
}

unsafe impl Send for MmioNetTransport {}
unsafe impl Sync for MmioNetTransport {}

impl MmioNetTransport {
    /// 尝试在指定物理基址构造 VirtIO-net MMIO transport。
    fn new(phys_base: usize) -> Option<Self> {
        let base = phys_to_mmio_virt(phys_base) as *mut u8;
        let transport = Self {
            base,
            phys_base,
            version: 0,
        };

        if transport.read32(MMIO_MAGIC_VALUE) != VIRTIO_MMIO_MAGIC {
            return None;
        }
        if transport.read32(MMIO_DEVICE_ID) != VIRTIO_MMIO_DEVICE_ID_NET {
            return None;
        }
        let version = transport.read32(MMIO_VERSION);
        if version != VIRTIO_MMIO_VERSION_LEGACY && version != VIRTIO_MMIO_VERSION_MODERN {
            log::info!("virtio-mmio net unsupported transport version {}", version);
            return None;
        }

        polyhal::println!("Found VirtIO-mmio net device version {}", version);
        Some(Self {
            base,
            phys_base,
            version,
        })
    }

    /// 读取 32 位 MMIO 寄存器。
    #[inline]
    fn read32(&self, offset: usize) -> u32 {
        unsafe { self.base.add(offset).cast::<u32>().read_volatile() }
    }

    /// 写入 32 位 MMIO 寄存器。
    #[inline]
    fn write32(&self, offset: usize, value: u32) {
        unsafe {
            self.base.add(offset).cast::<u32>().write_volatile(value);
        }
    }

    /// 返回 device-specific config 空间指针。
    pub(crate) fn device_config(&self) -> *mut u8 {
        unsafe { self.base.add(MMIO_CONFIG_SPACE) }
    }

    /// 复位设备。
    pub(crate) fn reset(&self) {
        self.write32(MMIO_STATUS, 0);
    }

    /// OR 写入 device status。
    pub(crate) fn add_status(&self, status: u8) {
        let current = self.read32(MMIO_STATUS);
        self.write32(MMIO_STATUS, current | status as u32);
    }

    /// 读取 device status。
    pub(crate) fn status(&self) -> u8 {
        self.read32(MMIO_STATUS) as u8
    }

    /// 读取设备 feature bits。
    pub(crate) fn read_device_features(&self) -> u64 {
        self.write32(MMIO_DEVICE_FEATURES_SEL, 0);
        let low = self.read32(MMIO_DEVICE_FEATURES) as u64;
        if self.is_legacy() {
            low
        } else {
            self.write32(MMIO_DEVICE_FEATURES_SEL, 1);
            low | ((self.read32(MMIO_DEVICE_FEATURES) as u64) << 32)
        }
    }

    /// 写入驱动选择的 feature bits。
    pub(crate) fn write_driver_features(&self, features: u64) {
        self.write32(MMIO_DRIVER_FEATURES_SEL, 0);
        self.write32(MMIO_DRIVER_FEATURES, features as u32);
        self.write32(MMIO_DRIVER_FEATURES_SEL, 1);
        self.write32(MMIO_DRIVER_FEATURES, (features >> 32) as u32);
    }

    /// 是否为 legacy MMIO transport。
    pub(crate) fn is_legacy(&self) -> bool {
        self.version == VIRTIO_MMIO_VERSION_LEGACY
    }

    /// 查询指定队列支持的最大大小。
    pub(crate) fn max_queue_size(&self, queue_idx: u16) -> u16 {
        self.write32(MMIO_QUEUE_SEL, queue_idx as u32);
        self.read32(MMIO_QUEUE_NUM_MAX) as u16
    }

    /// 配置指定 virtqueue 的大小和物理地址。
    pub(crate) fn setup_queue(
        &self,
        queue_idx: u16,
        queue_size: u16,
        desc_pa: u64,
        avail_pa: u64,
        used_pa: u64,
    ) {
        self.write32(MMIO_QUEUE_SEL, queue_idx as u32);
        self.write32(MMIO_QUEUE_NUM, queue_size as u32);
        if self.version == VIRTIO_MMIO_VERSION_LEGACY {
            let _ = avail_pa;
            let _ = used_pa;
            self.write32(MMIO_GUEST_PAGE_SIZE, VIRTIO_MMIO_GUEST_PAGE_SIZE);
            self.write32(MMIO_QUEUE_ALIGN, VIRTIO_MMIO_GUEST_PAGE_SIZE);
            self.write32(
                MMIO_QUEUE_PFN,
                (desc_pa / VIRTIO_MMIO_GUEST_PAGE_SIZE as u64) as u32,
            );
        } else {
            self.write32(MMIO_QUEUE_DESC_LOW, desc_pa as u32);
            self.write32(MMIO_QUEUE_DESC_HIGH, (desc_pa >> 32) as u32);
            self.write32(MMIO_QUEUE_DRIVER_LOW, avail_pa as u32);
            self.write32(MMIO_QUEUE_DRIVER_HIGH, (avail_pa >> 32) as u32);
            self.write32(MMIO_QUEUE_DEVICE_LOW, used_pa as u32);
            self.write32(MMIO_QUEUE_DEVICE_HIGH, (used_pa >> 32) as u32);
            self.write32(MMIO_QUEUE_READY, 1);
        }
    }

    /// 通知设备处理指定队列。
    pub(crate) fn notify(&self, queue_idx: u16) {
        self.write32(MMIO_QUEUE_NOTIFY, queue_idx as u32);
    }

    #[allow(unused)]
    /// 返回 MMIO transport 的物理基址。
    pub(crate) fn phys_base(&self) -> usize {
        self.phys_base
    }
}

/// 扫描默认 MMIO 窗口，寻找 VirtIO-net 设备。
pub(crate) fn probe_virtio_net() -> Option<MmioNetTransport> {
    for idx in 0..VIRTIO_MMIO_DEVICE_COUNT {
        let base = VIRTIO_MMIO_BASE_START + idx * VIRTIO_MMIO_DEVICE_STRIDE;
        if let Some(transport) = MmioNetTransport::new(base) {
            return Some(transport);
        }
    }
    None
}
