use alloc::sync::Arc;
use crate::alloc::string::ToString;
use crate::fs::vfs::dcache::GLOBAL_DCACHE;
use crate::fs::vfs::fstype::FsTypeInner;
use crate::fs::FsType;
use crate::fs::Dentry;
use crate::fs::MountFlags;
use crate::devices::BlockDevice;
use crate::fs::cgroup2::dentry::Cgroup2Dentry;
use crate::fs::cgroup2::inode::Cgroup2Inode;
use crate::fs::vfs::inode::InodeMode;
use crate::fs::SuperBlockInner;
use crate::fs::tempfs::superblock::TempSuperBlock;

pub struct Cgroup2FsType {
    inner: FsTypeInner,
}

impl Cgroup2FsType {
    pub fn new(name: &str) -> Arc<Self> {
        Arc::new(Self {
            inner: FsTypeInner::new(name),
        })
    }
}

impl FsType for Cgroup2FsType {
    fn inner(&self) -> &FsTypeInner {
        &self.inner
    }
    fn mount(&'static self, name: &str, parent: Option<Arc<dyn Dentry>>, _flags: MountFlags, dev: Option<Arc<dyn BlockDevice>>) -> Option<Arc<dyn Dentry>> {
        let superblock = Arc::new(TempSuperBlock::new(SuperBlockInner::new(dev, parent.clone())));
        let root_inode = Arc::new(Cgroup2Inode::new(InodeMode::DIR));
        let root_dentry = Cgroup2Dentry::new(name, parent.clone());
        root_dentry.set_inode(root_inode);

        // 在根目录下自动创建 cgroup 文件
        let procs = Cgroup2Dentry::new("cgroup.procs", Some(root_dentry.clone()));
        procs.set_inode(Arc::new(Cgroup2Inode::new(InodeMode::FILE)));
        root_dentry.get_dentryinner().children.lock().insert("cgroup.procs".to_string(), procs);

        let ctrls = Cgroup2Dentry::new("cgroup.controllers", Some(root_dentry.clone()));
        ctrls.set_inode(Arc::new(Cgroup2Inode::new(InodeMode::FILE)));
        root_dentry.get_dentryinner().children.lock().insert("cgroup.controllers".to_string(), ctrls);

        let subtree = Cgroup2Dentry::new("cgroup.subtree_control", Some(root_dentry.clone()));
        subtree.set_inode(Arc::new(Cgroup2Inode::new(InodeMode::FILE)));
        root_dentry.get_dentryinner().children.lock().insert("cgroup.subtree_control".to_string(), subtree);

        GLOBAL_DCACHE.insert(root_dentry.path(), root_dentry.clone());
        GLOBAL_DCACHE.pin(root_dentry.path());
        self.add_sb(&root_dentry.path(), superblock.clone());
        Some(root_dentry)
    }
    fn kill_sb(&self) -> isize {
        todo!()
    }
}
