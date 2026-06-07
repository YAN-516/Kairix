use crate::devices::BlockDevice;
use crate::error::SysError;
use crate::fs::SuperBlock;
use crate::fs::SuperBlockInner;
use crate::fs::fat32::io::FatIoAdapter;
use crate::fs::vfs::kstat::Statfs;
use alloc::string::String;
use alloc::string::ToString;
use alloc::sync::Arc;
use fatfs::{FileSystem, LossyOemCpConverter, NullTimeProvider};
use spin::mutex::Mutex;

pub struct Fat32SuperBlock {
    pub inner: SuperBlockInner,
    pub fs: Mutex<FileSystem<FatIoAdapter, NullTimeProvider, LossyOemCpConverter>>,
    pub mount_point: String,
}

unsafe impl Sync for Fat32SuperBlock {}
unsafe impl Send for Fat32SuperBlock {}

impl Fat32SuperBlock {
    pub fn new(inner: SuperBlockInner, mount_point: &str) -> Result<Self, SysError> {
        let block_device = inner.device.as_ref().ok_or(SysError::ENODEV)?.clone();
        let io_adapter = FatIoAdapter::new(block_device);
        let fs = FileSystem::new(io_adapter, fatfs::FsOptions::new()).map_err(|_| SysError::EIO)?;
        Ok(Self {
            inner,
            fs: Mutex::new(fs),
            mount_point: mount_point.to_string(),
        })
    }
}

impl SuperBlock for Fat32SuperBlock {
    fn inner(&self) -> &SuperBlockInner {
        &self.inner
    }

    fn statfs(&self) -> Statfs {
        let mut stat = Statfs::new();
        stat.f_type = 0x4d44; // FAT_SUPER_MAGIC
        let fs = self.fs.lock();
        if let Ok(stats) = fs.stats() {
            stat.f_bsize = stats.cluster_size() as i64;
            stat.f_blocks = stats.total_clusters() as i64;
            stat.f_bfree = stats.free_clusters() as i64;
            stat.f_bavail = stats.free_clusters() as i64;
            stat.f_frsize = stats.cluster_size() as i64;
        }
        stat.f_namelen = 255;
        stat
    }
}
