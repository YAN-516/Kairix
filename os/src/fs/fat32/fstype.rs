use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::sync::Arc;
use crate::devices::BlockDevice;
use crate::fs::fat32::dentry::Fat32Dentry;
use crate::fs::fat32::inode::Fat32Inode;
use crate::fs::fat32::superblock::Fat32SuperBlock;
use crate::fs::vfs::dcache::GLOBAL_DCACHE;
use crate::fs::vfs::fstype::{FsType, FsTypeInner, MountFlags};
use crate::fs::vfs::inode::{inode_alloc, InodeMode};
use crate::fs::Dentry;
use crate::fs::SuperBlockInner;

pub struct Fat32FsType {
    inner: FsTypeInner,
}

impl Fat32FsType {
    pub fn new(name: &str) -> Arc<Self> {
        Arc::new(Self {
            inner: FsTypeInner::new(name),
        })
    }
}

impl FsType for Fat32FsType {
    fn inner(&self) -> &FsTypeInner {
        &self.inner
    }

    fn kill_sb(&self) -> isize {
        todo!()
    }

    fn mount(
        &'static self,
        name: &str,
        parent: Option<Arc<dyn Dentry>>,
        _flags: MountFlags,
        dev: Option<Arc<dyn BlockDevice>>,
    ) -> Option<Arc<dyn Dentry>> {
        let mount_point = if let Some(ref p) = parent {
            let pp = p.path();
            if pp == "/" {
                format!("/{}", name)
            } else {
                format!("{}/{}", pp, name)
            }
        } else {
            "/".to_string()
        };

        let superblock = Arc::new(
            Fat32SuperBlock::new(SuperBlockInner::new(dev, None), &mount_point).ok()?,
        );
        let sb_weak = Arc::downgrade(&superblock);

        let root_inode = Arc::new(Fat32Inode::new(
            inode_alloc(),
            0,
            InodeMode::DIR | InodeMode::from_bits_truncate(0o777),
            String::new(),
            true,
            sb_weak.clone(),
        ));
        let root_dentry = Fat32Dentry::new(name, parent, String::new(), sb_weak);
        root_dentry.set_inode(root_inode);

        let root_path = root_dentry.path();
        GLOBAL_DCACHE.insert(root_path.clone(), root_dentry.clone());
        GLOBAL_DCACHE.pin(root_path);

        self.add_sb(&mount_point, superblock);
        Some(root_dentry)
    }
}
