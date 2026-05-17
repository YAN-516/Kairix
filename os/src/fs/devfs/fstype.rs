use alloc::sync::Arc;

use virtio_drivers::transport::pci::bus::PciRoot;

use crate::devices::BlockDevice;
use crate::fs::{
    devfs::superblock::DevSuperBlock,
    vfs::fstype::FsTypeInner,
    Dentry, FsType, MountFlags, SuperBlockInner,
};
use crate::fs::vfs::inode::{InodeMode, inode_alloc};
use crate::fs::tempfs::inode::TempInode;
use crate::fs::GLOBAL_DCACHE;
use crate::fs::tempfs::dentry::TempDentry;
/// the devfs fstype
pub struct DevFsType {
    inner: FsTypeInner,
}

impl DevFsType {
    ///
    pub fn new(name: &str) -> Arc<Self> {
        Arc::new( Self {
            inner: FsTypeInner::new(name),
        })
    }
}

impl FsType for DevFsType {
    fn inner(&self) -> &FsTypeInner {
        &self.inner
    }

    fn mount(&'static self, name: &str, parent: Option<Arc<dyn Dentry>>, flags: MountFlags, dev: Option<Arc<dyn BlockDevice>>) -> Option<Arc<dyn Dentry>> {
        let superblock = Arc::new(DevSuperBlock::new(SuperBlockInner::new(dev, parent.clone(), flags)));
        let root_inode = Arc::new(TempInode::new( InodeMode::DIR));
        let root_dentry = TempDentry::new(name, parent.clone());
        root_dentry.set_inode(root_inode);
        GLOBAL_DCACHE.insert(root_dentry.path(), root_dentry.clone());
        GLOBAL_DCACHE.pin(root_dentry.path());
        self.add_sb(&root_dentry.path(), superblock.clone());
        Some(root_dentry)
    }

    fn kill_sb(&self) -> isize {
        todo!()
    }
}