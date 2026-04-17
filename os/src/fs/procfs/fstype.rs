use alloc::sync::Arc;

use virtio_drivers::transport::pci::bus::PciRoot;

use crate::devices::BlockDevice;
use crate::fs::{
    procfs::superblock::ProcSuperBlock,
    vfs::fstype::FsTypeInner,
    Dentry, FsType, MountFlags, SuperBlockInner,
};
use crate::fs::vfs::inode::{InodeMode, inode_alloc};
use crate::fs::tempfs::inode::TempInode;
use crate::fs::GLOBAL_DCACHE;
use crate::fs::tempfs::dentry::TempDentry;
/// the procfs fstype
pub struct ProcFsType {
    inner: FsTypeInner,
}

impl ProcFsType {
    ///
    pub fn new(name: &str) -> Arc<Self> {
        Arc::new( Self {
            inner: FsTypeInner::new(name),
        })
    }
}

impl FsType for ProcFsType {
    fn inner(&self) -> &FsTypeInner {
        &self.inner
    }

    fn mount(&'static self, name: &str, parent: Option<Arc<dyn Dentry>>, _flags: MountFlags, dev: Option<Arc<dyn BlockDevice>>) -> Option<Arc<dyn Dentry>> {
        let superblock = Arc::new(ProcSuperBlock::new(SuperBlockInner::new(dev, parent.clone())));
        let root_inode = Arc::new(TempInode::new(inode_alloc(), InodeMode::DIR));
        let root_dentry = TempDentry::new(name, parent.clone());
        root_dentry.set_inode(root_inode);
        GLOBAL_DCACHE.insert(root_dentry.path(), root_dentry.clone());
        self.add_sb(&root_dentry.path(), superblock.clone());
        Some(root_dentry)
    }

    fn kill_sb(&self) -> isize {
        todo!()
    }
}