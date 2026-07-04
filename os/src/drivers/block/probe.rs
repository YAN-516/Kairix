use super::BLOCK_DEVICE;
use super::VirtIOBlock;
use crate::drivers::block::BlockDevice;
use alloc::sync::Arc;
use log::info;
use virtio_drivers::transport::pci::PciTransport;
use virtio_drivers::transport::*;
// use super::set_block_device;

pub fn _virtio_device(transport: PciTransport) {
    let device_type = transport.device_type();
    info!("VirtIO device type: {:?}", device_type);

    match device_type {
        virtio_drivers::transport::DeviceType::Block => {
            info!("Creating VirtIO block device");
            let blk = VirtIOBlock::new_pci(transport);
            _register_block_device(Arc::new(blk));
        }
        _ => {
            info!("Unsupported VirtIO device type: {:?}", device_type);
        }
    }
}

pub fn _register_block_device(_dev: Arc<dyn BlockDevice>) {
    // let ptr = &*BLOCK_DEVICE as *const Arc<dyn BlockDevice> as *mut Arc<dyn BlockDevice>;
    // unsafe {
    //     core::ptr::write(ptr, dev);
    // }
    // set_block_device(dev);

    info!("Block device registered");
}
