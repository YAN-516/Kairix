use crate::fs::vfs::{SuperBlock};
use crate::fs::SuperBlockInner;
use log::info;
/// The ProcSuperBlock
#[allow(dead_code)]
pub struct ProcSuperBlock {
    inner:SuperBlockInner,
}

unsafe impl Sync for ProcSuperBlock {}
unsafe impl Send for ProcSuperBlock {}

#[allow(unused)]
impl ProcSuperBlock {
    /// Create a new Dev super block
    pub fn new(inner:SuperBlockInner) -> Self {
        Self { inner}
    }
}
impl SuperBlock for ProcSuperBlock {
    fn inner(&self) -> &SuperBlockInner {
        &self.inner
    }
}

