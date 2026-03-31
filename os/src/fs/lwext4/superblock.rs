use crate::fs::vfs::{SuperBlock};
use crate::fs::SuperBlockInner;
use lwext4_rust::Ext4BlockWrapper;
use crate::fs::lwext4::disk::Disk;
use log::info;
/// The Ext4SuperBlock
#[allow(dead_code)]
pub struct Ext4SuperBlock {
    inner:SuperBlockInner,
    block: Ext4BlockWrapper<Disk>,
}

unsafe impl Sync for Ext4SuperBlock {}
unsafe impl Send for Ext4SuperBlock {}

impl Ext4SuperBlock {
    /// Create a new Ext4 super block
    pub fn new(inner:SuperBlockInner) -> Self {
        // let disk =Disk::new(BLOCK_DEVICE.clone());
        let block_device = inner.device.as_ref().unwrap().clone();
        let disk = Disk::new(block_device);

        info!(
            "Got Disk size:{}, position:{}",
            disk.size(),
            disk.position()
        );
        let block = Ext4BlockWrapper::<Disk>::new(disk)
            .expect("failed to initialize EXT4 filesystem");
       
        Self { inner, block }
    }
}
impl SuperBlock for Ext4SuperBlock {
    fn inner(&self) -> &SuperBlockInner {
        &self.inner
    }
}

