//! vfs super block
//!
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::devices::BlockDevice;
use crate::fs::vfs::Dentry;
use crate::fs::vfs::fstype::MountFlags;
use crate::fs::vfs::inode::Inode;
use crate::fs::vfs::kstat::Statfs;
use crate::sync::SpinNoIrqLock;

/// the base of super block of all file system
pub struct SuperBlockInner {
    /// the block device fs using
    pub device: Option<Arc<dyn BlockDevice>>,
    /// the root dentry
    pub root: SpinNoIrqLock<Option<Arc<dyn Dentry>>>,
    /// mount flags
    pub flags: AtomicU32,
}

impl SuperBlockInner {
    /// create a super block inner with device
    pub fn new(
        device: Option<Arc<dyn BlockDevice>>,
        root: Option<Arc<dyn Dentry>>,
        flags: MountFlags,
    ) -> Self {
        Self {
            device,
            root: SpinNoIrqLock::new(root),
            flags: AtomicU32::new(flags.bits()),
        }
    }

    /// Set or replace the root dentry after filesystem-specific mount setup.
    pub fn set_root(&self, root: Arc<dyn Dentry>) {
        *self.root.lock() = Some(root);
    }

    /// Update mount flags, used by remount.
    pub fn set_flags(&self, flags: MountFlags) {
        self.flags.store(flags.bits(), Ordering::Relaxed);
    }

    /// check if the filesystem is mounted read-only
    pub fn is_readonly(&self) -> bool {
        self.flags().contains(MountFlags::MS_RDONLY)
    }

    /// Get current mount flags.
    pub fn flags(&self) -> MountFlags {
        MountFlags::from_bits_truncate(self.flags.load(Ordering::Relaxed))
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
        self.inner()
            .root
            .lock()
            .as_ref()
            .expect("superblock root dentry is not initialized")
            .clone()
    }
}
