//! VirtIO-net 相关的规范常量和共享内存结构。

/// VirtIO PCI vendor ID。
pub const VIRTIO_PCI_VENDOR_ID: u16 = 0x1AF4;
/// legacy VirtIO-net PCI device ID。
pub const VIRTIO_PCI_DEVICE_ID_NET: u16 = 0x1000;
/// modern VirtIO-net PCI device ID。
pub const VIRTIO_PCI_DEVICE_ID_NET_MODERN: u16 = 0x1041;

/// PCI capability: common config。
pub const VIRTIO_PCI_CAP_COMMON_CFG: u8 = 1;
/// PCI capability: notify config。
pub const VIRTIO_PCI_CAP_NOTIFY_CFG: u8 = 2;
/// PCI capability: ISR config。
pub const VIRTIO_PCI_CAP_ISR_CFG: u8 = 3;
/// PCI capability: device-specific config。
pub const VIRTIO_PCI_CAP_DEVICE_CFG: u8 = 4;

/// VirtIO 1.0+ modern device feature bit。
pub const VIRTIO_F_VERSION_1: u64 = 1 << 32;
/// VirtIO-net device config exposes MAC address。
pub const VIRTIO_NET_F_MAC: u64 = 1 << 5;

/// 设备复位状态。
pub const VIRTIO_STATUS_RESET: u8 = 0;
/// 驱动识别到设备。
pub const VIRTIO_STATUS_ACK: u8 = 1;
/// 驱动知道如何驱动该设备。
pub const VIRTIO_STATUS_DRIVER: u8 = 2;
/// 驱动初始化完成，设备可以开始工作。
pub const VIRTIO_STATUS_DRIVER_OK: u8 = 4;
/// 特性协商成功。
pub const VIRTIO_STATUS_FEATURES_OK: u8 = 8;
/// 不使用 MSI-X 中断向量。
pub const VIRTIO_MSI_NO_VECTOR: u16 = 0xffff;
#[allow(unused)]
/// descriptor 后面还有下一个 descriptor。
pub const VIRTQ_DESC_F_NEXT: u16 = 1;
/// descriptor 对设备可写，常用于 RX buffer。
pub const VIRTQ_DESC_F_WRITE: u16 = 2;

/// 当前驱动申请的 virtqueue 大小。
pub const QUEUE_SIZE: u16 = 256;

/// VirtIO PCI common config 结构。
///
/// 字段布局必须和 VirtIO 规范一致，驱动通过 volatile 读写访问。
#[repr(C)]
#[derive(Debug)]
pub struct VirtIOCommonCfg {
    pub device_feature_select: u32,
    pub device_feature: u32,
    pub driver_feature_select: u32,
    pub driver_feature: u32,
    pub msix_config: u16,
    pub num_queues: u16,
    pub device_status: u8,
    pub config_generation: u8,
    pub queue_select: u16,
    pub queue_size: u16,
    pub queue_msix_vector: u16,
    pub queue_enable: u16,
    pub queue_notify_off: u16,
    pub queue_desc: u64,
    pub queue_driver: u64,
    pub queue_device: u64,
}

/// Virtqueue descriptor。
#[repr(C)]
#[derive(Debug)]
pub struct VirtqDesc {
    /// buffer 的物理地址。
    pub addr: u64,
    /// buffer 长度。
    pub len: u32,
    /// descriptor 标志位。
    pub flags: u16,
    /// 下一个 descriptor 索引，仅在 `VIRTQ_DESC_F_NEXT` 置位时有效。
    pub next: u16,
}

/// Available ring，由驱动写入，告诉设备哪些 descriptor 可用。
#[repr(C)]
#[derive(Debug)]
pub struct VirtqAvail {
    pub flags: u16,
    pub idx: u16,
    pub ring: [u16; 0],
    pub used_event: u16,
}

/// Used ring element，由设备写入，告诉驱动处理完成的 descriptor。
#[repr(C)]
#[derive(Debug)]
pub struct VirtqUsedElem {
    /// 完成的 descriptor 链头索引。
    pub id: u32,
    /// 设备写入或消费的长度。
    pub len: u32,
}

/// Used ring，由设备维护的完成队列。
#[repr(C)]
#[derive(Debug)]
pub struct VirtqUsed {
    pub flags: u16,
    pub idx: u16,
    pub ring: [VirtqUsedElem; 0],
    pub avail_event: u16,
}
