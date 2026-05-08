//参照chronix的设计，ext4文件系统类型实现

use crate::devices::BlockDevice;
use crate::fs::lwext4::{
    inode::Ext4Inode,
    superblock,
};
use crate::fs::vfs::inode::inode_alloc;
use crate::fs::vfs::{
    fstype::{FsType, FsTypeInner},
    dentry::{Dentry, DentryState},
    fstype::MountFlags,
    inode::{Inode, InodeInner},
};
use crate::fs::{SuperBlock, SuperBlockInner,Ext4Dentry,Ext4SuperBlock,GLOBAL_DCACHE};
use alloc::{
    string::ToString,
    sync::Arc,
};
use lwext4_rust::InodeTypes::EXT4_DE_DIR;
///
pub struct Ext4FsType {
    inner: FsTypeInner,
}

impl Ext4FsType {
    ///
    pub fn new(name: &str) -> Arc<Self> {
        Arc::new(Self{
            inner: FsTypeInner::new(name),
        })
    }
}


/// mount point for disk fs
static DISK_MP: &str = "/";
/// mount point for sdcard fs
static SDCARD_MP: &str = "sdcard/";


impl FsType for Ext4FsType {
    fn inner(&self) -> &FsTypeInner {
        &self.inner
    }
    fn kill_sb(&self) -> isize {
        todo!()
    }
    fn mount(&'static self, name: &str, parent: Option<Arc<dyn Dentry>>, _flags: MountFlags, dev: Option<Arc<dyn BlockDevice>>) -> Option<Arc<dyn Dentry>> {
        // can be dangerous..

        let mount_point_path = if parent.is_none() {
            DISK_MP
        } else {
            SDCARD_MP
        };

        let root_inode = Arc::new(Ext4Inode::new(inode_alloc(),EXT4_DE_DIR, "/".to_string()));
        let root_dentry = Ext4Dentry::new(name, parent.clone());
        root_dentry.set_inode(root_inode);
        let superblock =Arc::new(Ext4SuperBlock::new(SuperBlockInner::new(dev.clone(), Some(root_dentry.clone()))));
        GLOBAL_DCACHE.insert(mount_point_path.to_string(), root_dentry.clone());
        GLOBAL_DCACHE.pin(mount_point_path.to_string());
        self.add_sb(&mount_point_path, superblock.clone());
        Some(root_dentry)
    }
}