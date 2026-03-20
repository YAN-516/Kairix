//! File system in os
mod stdio;
pub mod vfs;
mod lwext4;
pub use lwext4::file::{Ext4File, OpenFlags, list_apps, open_file};
pub use stdio::{Stdin, Stdout};
pub use vfs::file::File;
use alloc::{collections::btree_map::BTreeMap, string::{String, ToString}, sync::Arc};
use log::*;
use crate::{drivers::BLOCK_DEVICE};
pub use crate::fs::lwext4::superblock::Ext4SuperBlock;
pub use vfs::superblock::{SuperBlock, SuperBlockInner};
use lwext4_rust::{InodeTypes};
use crate::fs::lwext4::inode::Ext4Inode;
use crate::fs::vfs::inode::Inode;
use crate::sync::UPSafeCell;
use lazy_static::lazy_static;
use crate::fs::lwext4::dentry::Ext4Dentry;

lazy_static! {
/// file system manager
/// hold the lifetime of all file system
/// maintain the mapping
    pub static ref FS_MANAGER: UPSafeCell<BTreeMap<String, Arc<dyn SuperBlock>>> =
        unsafe{UPSafeCell::new(BTreeMap::new())};
}
/// the default filesystem on disk
pub const DISK_FS_NAME: &str = "lwext4";


/// init the file system
pub fn init() {

    let root_inode = Arc::new(Ext4Inode::new(
        "/", 
        InodeTypes::EXT4_DE_DIR
    )) as Arc<dyn Inode>;
    //root_dentry dont have parent
    let root_dentry = Ext4Dentry::new(
        "/",                  
        None               
    );
    root_dentry.set_inode(root_inode);
    // SuperBlock should contain root_dentry
    let lwext4_superblock = Arc::new(Ext4SuperBlock::new(
        SuperBlockInner::new(
            Some(BLOCK_DEVICE.clone()), 
            Some(root_dentry) 
        )
    ));

    FS_MANAGER.exclusive_access().insert(DISK_FS_NAME.to_string(), lwext4_superblock);
    info!("lwext4 finish init with VFS root dentry");
}
