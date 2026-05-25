//! vfs super block
//! 
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU32, Ordering};

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
    pub flags: AtomicU32,
}

impl SuperBlockInner {
    /// create a super block inner with device
    pub fn new(device: Option<Arc<dyn BlockDevice>>, root: Option<Arc<dyn Dentry>>, flags: MountFlags) -> Self {
        Self {
            device,
            root,
            flags: AtomicU32::new(flags.bits()),
        }
    }

    /// Update mount flags, used by remount.
    pub fn set_flags(&self, flags: MountFlags) {
        self.flags.store(flags.bits(), Ordering::Relaxed);
    }

    /// check if the filesystem is mounted read-only
    pub fn is_readonly(&self) -> bool {
        MountFlags::from_bits_truncate(self.flags.load(Ordering::Relaxed))
            .contains(MountFlags::MS_RDONLY)
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
