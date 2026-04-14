use crate::fs::vfs::{SuperBlock};
use crate::fs::SuperBlockInner;
use lwext4_rust::Ext4BlockWrapper;
use crate::fs::lwext4::disk::Disk;
use log::info;
/// The DevSuperBlock
#[allow(dead_code)]
pub struct DevSuperBlock {
    inner:SuperBlockInner,
}

unsafe impl Sync for DevSuperBlock {}
unsafe impl Send for DevSuperBlock {}

impl DevSuperBlock {
    /// Create a new Dev super block
    pub fn new(inner:SuperBlockInner) -> Self {
        Self { inner}
    }
}
impl SuperBlock for DevSuperBlock {
    fn inner(&self) -> &SuperBlockInner {
        &self.inner
    }
}

