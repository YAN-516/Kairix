use crate::fs::vfs::{SuperBlock};
use crate::fs::SuperBlockInner;
use lwext4_rust::Ext4BlockWrapper;
use crate::fs::lwext4::disk::Disk;
use log::info;
/// The TEMPSuperBlock
#[allow(dead_code)]
pub struct TempSuperBlock {
    inner:SuperBlockInner,
}

unsafe impl Sync for TempSuperBlock {}
unsafe impl Send for TempSuperBlock {}

impl TempSuperBlock {
    /// Create a new Dev super block
    pub fn new(inner:SuperBlockInner) -> Self {
        Self { inner}
    }
}
impl SuperBlock for TempSuperBlock {
    fn inner(&self) -> &SuperBlockInner {
        &self.inner
    }
}

