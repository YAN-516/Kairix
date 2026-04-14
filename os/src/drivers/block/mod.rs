pub mod virtio_blk;
#[cfg(target_arch = "loongarch64")]
mod pci;
#[cfg(target_arch = "loongarch64")]
mod probe;
pub use virtio_blk::VirtIOBlock;
pub use polyhal::println;
use crate::board::BlockDeviceImpl;
use alloc::sync::Arc;
use crate::devices::BlockDevice;
use lazy_static::*;
use core::cell::OnceCell;
#[cfg(target_arch = "riscv64")]
lazy_static! {
    pub static ref BLOCK_DEVICE: Arc<dyn BlockDevice> = Arc::new(BlockDeviceImpl::new());
}

static INNER_DEVICE: OnceCell<Arc<dyn BlockDevice + Send + Sync>> = OnceCell::new();

struct ProxyBlockDevice;
#[cfg(target_arch = "loongarch64")]
lazy_static! {
    pub static ref BLOCK_DEVICE: Arc<dyn BlockDevice> = Arc::new(ProxyBlockDevice);
}


#[allow(unused)]
pub fn block_device_test() {
    let block_device = BLOCK_DEVICE.clone();
    let mut write_buffer = [0u8; 512];
    let mut read_buffer = [0u8; 512];
    for i in 0..512 {
        for byte in write_buffer.iter_mut() {
            *byte = i as u8;
        }
        block_device.write_block(i as usize, &write_buffer);
        block_device.read_block(i as usize, &mut read_buffer);
        assert_eq!(write_buffer, read_buffer);
    }
    println!("block device test passed!");
}



// 安全的初始化函数（仅在 loongarch64 调用）
#[cfg(target_arch = "loongarch64")]
pub fn set_block_device(dev: Arc<dyn BlockDevice>) {
    INNER_DEVICE.set(dev).expect("Block device already set");
}
