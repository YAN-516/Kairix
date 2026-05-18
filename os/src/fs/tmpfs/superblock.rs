use crate::fs::SuperBlockInner;
use crate::fs::vfs::SuperBlock;
use crate::fs::vfs::kstat::Statfs;
use crate::mm::{get_free_memory, get_total_memory};
use log::info;
use polyhal::consts::PAGE_SIZE;
/// The TEMPSuperBlock
#[allow(dead_code)]
pub struct TempSuperBlock {
    inner: SuperBlockInner,
}

unsafe impl Sync for TempSuperBlock {}
unsafe impl Send for TempSuperBlock {}

impl TempSuperBlock {
    /// Create a new Dev super block
    pub fn new(inner: SuperBlockInner) -> Self {
        Self { inner }
    }
}
impl SuperBlock for TempSuperBlock {
    fn inner(&self) -> &SuperBlockInner {
        &self.inner
    }

    fn statfs(&self) -> Statfs {
        let bsize = PAGE_SIZE as i64;
        let blocks = (get_total_memory() / PAGE_SIZE) as i64;
        let free = (get_free_memory() / PAGE_SIZE) as i64;
        let mut stat = Statfs::new();
        stat.f_type = 0x0102_1994; // TMPFS_MAGIC
        stat.f_bsize = bsize;
        stat.f_blocks = blocks;
        stat.f_bfree = free;
        stat.f_bavail = free;
        stat.f_files = 1024;
        stat.f_ffree = 512;
        stat.f_frsize = bsize;
        stat
    }
}
