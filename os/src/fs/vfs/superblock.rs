//! vfs super block
//! 
use alloc::sync::Arc;

use crate::devices::BlockDevice;
use crate::fs::vfs::inode::Inode;
use crate::fs::vfs::Dentry;

/// the base of super block of all file system
pub struct SuperBlockInner {
    /// the block device fs using
    pub device: Option<Arc<dyn BlockDevice>>,
    /// the root dentry
    pub root: Option<Arc<dyn Dentry>>,
}

impl SuperBlockInner {
    /// create a super block inner with device
    pub fn new(device: Option<Arc<dyn BlockDevice>>, root: Option<Arc<dyn Dentry>>) -> Self {
        Self {
            device,
            root,
        }
    }
}

/// super block trait left for file system implement
pub trait SuperBlock: Send + Sync {
    /// get the inner data of superblock
    fn inner(&self) -> &SuperBlockInner;
}

impl dyn SuperBlock {
    /// get the root dentry
    pub fn root(&self) -> Arc<dyn Dentry> {
        Arc::clone(&self.inner().root.as_ref().unwrap())
    }
}
