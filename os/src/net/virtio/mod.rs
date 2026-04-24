// net/virtio/mod.rs
pub mod config;
pub mod device;
pub mod pci;
pub mod probe;
pub mod virtqueue;

pub use device::VirtIONetDevice;
