//! vfs super block
//! 
use alloc::sync::Arc;

use crate::devices::BlockDevice;
use crate::fs::vfs::inode::Inode;
use crate::fs::vfs::Dentry;
use crate::fs::vfs::fstype::MountFlags;
use crate::fs::vfs::kstat::Statfs;

/// the base of super block of all file system
pub struct SuperBlockInner {
    /// the block device fs using
    pub device: Option<Arc<dyn BlockDevice>>,
    /// the root dentry
    pub root: Option<Arc<dyn Dentry>>,
    /// mount flags
    pub flags: MountFlags,
}

impl SuperBlockInner {
    /// create a super block inner with device
    pub fn new(device: Option<Arc<dyn BlockDevice>>, root: Option<Arc<dyn Dentry>>, flags: MountFlags) -> Self {
        Self {
            device,
            root,
            flags,
        }
    }

    /// check if the filesystem is mounted read-only
    pub fn is_readonly(&self) -> bool {
        self.flags.contains(MountFlags::MS_RDONLY)
    }
}

/// super block trait left for file system implement
pub trait SuperBlock: Send + Sync {
    /// get the inner data of superblock
    fn inner(&self) -> &SuperBlockInner;
    /// get filesystem statistics
    fn statfs(&self) -> Statfs {
        Statfs::new()
    }
}

impl dyn SuperBlock {
    /// get the root dentry
    pub fn root(&self) -> Arc<dyn Dentry> {
        Arc::clone(&self.inner().root.as_ref().unwrap())
    }
}
