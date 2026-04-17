use alloc::sync::Arc;
use crate::fs::vfs::dcache::GLOBAL_DCACHE;
use crate::fs::vfs::fstype::FsTypeInner;
use crate::fs::FsType;
use crate::fs::Dentry;
use crate::fs::MountFlags;
use crate::devices::BlockDevice;
use crate::fs::tempfs::superblock::TempSuperBlock;
use crate::fs::SuperBlockInner;
use crate::fs::tempfs::inode::TempInode;
use crate::fs::vfs::inode::InodeMode;
use crate::fs::tempfs::dentry::TempDentry;
use crate::fs::vfs::inode::inode_alloc;
/// The temporary filesystem type
pub struct TempFsType {
    inner: FsTypeInner,
}

impl TempFsType {
    ///
    pub fn new(name: &str) -> Arc<Self> {
        Arc::new(Self {
            inner: FsTypeInner::new(name),
        })
    }
}

impl FsType for TempFsType {
    fn inner(&self) -> &FsTypeInner {
        &self.inner
    }
    fn mount(&'static self, name: &str, parent: Option<Arc<dyn Dentry>>, _flags: MountFlags, dev: Option<Arc<dyn BlockDevice>>) -> Option<Arc<dyn Dentry>> {
        let superblock = Arc::new(TempSuperBlock::new(SuperBlockInner::new(dev, parent.clone())));
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