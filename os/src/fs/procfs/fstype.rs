use alloc::sync::Arc;

use virtio_drivers::transport::pci::bus::PciRoot;

use crate::devices::BlockDevice;
use crate::fs::procfs::pid_dir::ProcRootDentry;
use crate::fs::tmpfs::inode::TempInode;
use crate::fs::vfs::inode::{inode_alloc, InodeMode};
use crate::fs::GLOBAL_DCACHE;
use crate::fs::{
    procfs::superblock::ProcSuperBlock, vfs::fstype::FsTypeInner, Dentry, FsType, MountFlags,
    SuperBlockInner,
};
/// the procfs fstype
pub struct ProcFsType {
    inner: FsTypeInner,
}

impl ProcFsType {
    ///
    pub fn new(name: &str) -> Arc<Self> {
        Arc::new(Self {
            inner: FsTypeInner::new(name),
        })
    }
}

impl FsType for ProcFsType {
    fn inner(&self) -> &FsTypeInner {
        &self.inner
    }

    fn mount(
        &self,
        name: &str,
        parent: Option<Arc<dyn Dentry>>,
        flags: MountFlags,
        dev: Option<Arc<dyn BlockDevice>>,
    ) -> Option<Arc<dyn Dentry>> {
        let root_inode = Arc::new(TempInode::new(InodeMode::DIR));
        let root_dentry = ProcRootDentry::new(name, parent.clone());
        root_dentry.set_inode(root_inode);
        let superblock = Arc::new(ProcSuperBlock::new(SuperBlockInner::new(
            dev,
            Some(root_dentry.clone()),
            flags,
        )));
        GLOBAL_DCACHE.insert(root_dentry.path(), root_dentry.clone());
        GLOBAL_DCACHE.pin(root_dentry.path());
        self.add_sb(&root_dentry.path(), superblock.clone());
        Some(root_dentry)
    }

    fn kill_sb(&self) -> isize {
        todo!()
    }
}
