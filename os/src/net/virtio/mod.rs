//! VirtIO-net 驱动模块。
//!
//! 子模块按职责拆分为规范常量、设备主体、PCI/MMIO transport、探测流程和
//! virtqueue 内存管理。

pub mod config;
pub mod device;
pub mod mmio;
pub mod pci;
pub mod probe;
pub mod virtqueue;

pub use device::VirtIONetDevice;
