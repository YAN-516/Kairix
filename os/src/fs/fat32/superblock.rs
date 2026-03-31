use crate::drivers::BLOCK_DEVICE;
use crate::fs::SuperBlockInner;
use crate::fs::lwext4::disk::Disk;
use crate::fs::Arc;
use log::info;
use fatfs::FileSystem;
use crate::fs::fat32::io::FatIoAdapter;
use crate::fs::SuperBlock;
pub struct Fat32SuperBlock {
    pub inner:SuperBlockInner,
    pub block: Arc<FileSystem<FatIoAdapter>>,
}

unsafe impl Sync for Fat32SuperBlock {}
unsafe impl Send for Fat32SuperBlock {}

impl Fat32SuperBlock {
    /// Create a new Fat32 super block
    pub fn new(inner:SuperBlockInner) -> Self {
        // let disk =Disk::new(BLOCK_DEVICE.clone());
        let block_device = inner.device.as_ref().unwrap().clone();
        let io_adapter = FatIoAdapter::new(block_device);
        let block = Arc::new(FileSystem::new(io_adapter, fatfs::FsOptions::new())
            .expect("failed to initialize FAT32 filesystem"));
       
        Self { inner, block }
    }
}
impl SuperBlock for Fat32SuperBlock {
    fn inner(&self) -> &SuperBlockInner {
        &self.inner
    }
}

