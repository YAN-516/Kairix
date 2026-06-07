//参照chronix的设计，ext4文件系统类型实现

use crate::devices::BlockDevice;
use crate::error::SysResult;
use crate::fs::lwext4::inode::Ext4Inode;
use crate::fs::vfs::inode::inode_alloc;
use crate::fs::vfs::{
    dentry::Dentry,
    fstype::{FsType, FsTypeInner, MountFlags},
};
use crate::fs::{Ext4Dentry, Ext4SuperBlock, GLOBAL_DCACHE, SuperBlock, SuperBlockInner};
use alloc::{
    format,
    string::{String, ToString},
    sync::Arc,
};
use core::sync::atomic::{AtomicUsize, Ordering};
use lwext4_rust::InodeTypes::EXT4_DE_DIR;

static EXT4_MOUNT_ID: AtomicUsize = AtomicUsize::new(0);
///
pub struct Ext4FsType {
    inner: FsTypeInner,
}

impl Ext4FsType {
    ///
    pub fn new(name: &str) -> Arc<Self> {
        Arc::new(Self {
            inner: FsTypeInner::new(name),
        })
    }
}

impl FsType for Ext4FsType {
    fn inner(&self) -> &FsTypeInner {
        &self.inner
    }
    fn kill_sb(&self) -> isize {
        todo!()
    }
    fn mount(
        &self,
        name: &str,
        parent: Option<Arc<dyn Dentry>>,
        flags: MountFlags,
        dev: Option<Arc<dyn BlockDevice>>,
    ) -> SysResult<Arc<dyn Dentry>> {
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

        let mount_id = EXT4_MOUNT_ID.fetch_add(1, Ordering::Relaxed);
        let root_inode = Arc::new(Ext4Inode::new(
            inode_alloc(),
            EXT4_DE_DIR,
            mount_point.clone(),
            mount_id,
        ));
        let root_dentry = Ext4Dentry::new(name, parent.clone(), mount_id);
        root_dentry.set_inode(root_inode);
        let dev_name = format!("ext4_{}", mount_id);
        let superblock = Arc::new(Ext4SuperBlock::new(
            SuperBlockInner::new(dev.clone(), Some(root_dentry.clone()), flags),
            &dev_name,
            &mount_point,
        )?);
        GLOBAL_DCACHE.insert(mount_point.clone(), root_dentry.clone());
        GLOBAL_DCACHE.pin(mount_point.clone());
        self.add_sb(&mount_point, superblock.clone());
        Ok(root_dentry)
    }
}
