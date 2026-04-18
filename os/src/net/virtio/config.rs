// net/virtio/config.rs

// VirtIO 规范常量
pub const VIRTIO_PCI_VENDOR_ID: u16 = 0x1AF4;
pub const VIRTIO_PCI_DEVICE_ID_NET: u16 = 0x1000;
pub const VIRTIO_PCI_DEVICE_ID_NET_MODERN: u16 = 0x1041;

pub const VIRTIO_PCI_CAP_COMMON_CFG: u8 = 1;
pub const VIRTIO_PCI_CAP_NOTIFY_CFG: u8 = 2;
pub const VIRTIO_PCI_CAP_ISR_CFG: u8 = 3;
pub const VIRTIO_PCI_CAP_DEVICE_CFG: u8 = 4;

pub const VIRTIO_F_VERSION_1: u64 = 1 << 32;
pub const VIRTIO_NET_F_MAC: u64 = 1 << 5;

pub const VIRTIO_STATUS_RESET: u8 = 0;
pub const VIRTIO_STATUS_ACK: u8 = 1;
pub const VIRTIO_STATUS_DRIVER: u8 = 2;
pub const VIRTIO_STATUS_DRIVER_OK: u8 = 4;
pub const VIRTIO_STATUS_FEATURES_OK: u8 = 8;
#[allow(unused)]
pub const VIRTQ_DESC_F_NEXT: u16 = 1;
pub const VIRTQ_DESC_F_WRITE: u16 = 2;

pub const QUEUE_SIZE: u16 = 256;

/// VirtIO 通用配置
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
    pub queue_desc_lo: u32,
    pub queue_desc_hi: u32,
    pub queue_avail_lo: u32,
    pub queue_avail_hi: u32,
    pub queue_used_lo: u32,
    pub queue_used_hi: u32,
}

/// 描述符
#[repr(C)]
#[derive(Debug)]
pub struct VirtqDesc {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}

/// Available ring
#[repr(C)]
#[derive(Debug)]
pub struct VirtqAvail {
    pub flags: u16,
    pub idx: u16,
    pub ring: [u16; 0],
    pub used_event: u16,
}

/// Used ring element
#[repr(C)]
#[derive(Debug)]
pub struct VirtqUsedElem {
    pub id: u32,
    pub len: u32,
}

/// Used ring
#[repr(C)]
#[derive(Debug)]
pub struct VirtqUsed {
    pub flags: u16,
    pub idx: u16,
    pub ring: [VirtqUsedElem; 0],
    pub avail_event: u16,
}
