#[cfg(all(target_arch = "loongarch64", board = "2k1000"))]
pub mod ahci;
pub mod partition;
#[cfg(target_arch = "loongarch64")]
#[allow(dead_code)]
pub mod pci;
#[cfg(all(target_arch = "loongarch64", not(board = "2k1000")))]
#[allow(dead_code)]
mod probe;
pub mod ramdisk;
#[cfg(board = "visionfive2")]
pub mod vf2_sd;
#[allow(dead_code)]
pub mod virtio_blk;
#[cfg(not(board = "2k1000"))]
use crate::board::BlockDeviceImpl;
use crate::devices::BlockDevice;
#[cfg(all(target_arch = "loongarch64", board = "2k1000"))]
pub use ahci::AhciBlock;
use alloc::sync::Arc;
use lazy_static::*;
pub use ramdisk::RamDisk;
#[cfg(board = "visionfive2")]
pub use vf2_sd::Vf2SdBlock;
#[cfg(not(board = "2k1000"))]
pub use virtio_blk::VirtIOBlock;
// #[cfg(target_arch = "riscv64")]
struct BlockDeviceSlot {
    backend: crate::sync::SpinNoIrqLock<Option<Arc<dyn BlockDevice>>>,
}

impl BlockDeviceSlot {
    #[cfg(not(board = "2k1000"))]
    fn backend(&self) -> Arc<dyn BlockDevice> {
        let mut backend = self.backend.lock();
        if backend.is_none() {
            *backend = Some(Arc::new(BlockDeviceImpl::new()));
        }
        backend.as_ref().unwrap().clone()
    }

    #[cfg(board = "2k1000")]
    fn backend(&self) -> Arc<dyn BlockDevice> {
        self.backend
            .lock()
            .as_ref()
            .unwrap_or_else(|| panic!("2K1000 block device used before SATA/initrd registration"))
            .clone()
    }
}

impl BlockDevice for BlockDeviceSlot {
    fn size(&self) -> u64 {
        self.backend().size()
    }

    fn block_size(&self) -> usize {
        self.backend().block_size()
    }

    fn read_block(&self, block_id: usize, buf: &mut [u8]) {
        self.backend().read_block(block_id, buf)
    }

    fn write_block(&self, block_id: usize, buf: &[u8]) {
        self.backend().write_block(block_id, buf)
    }
}

lazy_static! {
    static ref BLOCK_DEVICE_SLOT: Arc<BlockDeviceSlot> = Arc::new(BlockDeviceSlot {
        backend: crate::sync::SpinNoIrqLock::new(None),
    });
    pub static ref BLOCK_DEVICE: Arc<dyn BlockDevice> = BLOCK_DEVICE_SLOT.clone();
}

#[allow(unused)]
pub fn set_block_device(device: Arc<dyn BlockDevice>) {
    *BLOCK_DEVICE_SLOT.backend.lock() = Some(device);
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
    polyhal::println!("block device test passed!");
}
