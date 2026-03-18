//! File system in os
mod osinode;
mod stdio;
mod disk;
mod ext4fs;
mod vfs;
mod superblock;
pub use osinode::{OSInode, OpenFlags, list_apps, open_file};
pub use stdio::{Stdin, Stdout};
pub use vfs::file::File;
use alloc::{collections::btree_map::BTreeMap, string::{String, ToString}, sync::Arc};
use log::*;
use crate::{drivers::BLOCK_DEVICE};
pub use superblock::Ext4SuperBlock;
pub use vfs::superblock::{SuperBlock, SuperBlockInner};
use lwext4_rust::{InodeTypes};
use crate::fs::ext4fs::Ext4Inode;
use crate::fs::vfs::vfs_ops::VfsInode;
use crate::sync::UPSafeCell;
use lazy_static::lazy_static;


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
    // create the ext4 file system using the block deviceS
    let root = Some(Arc::new(Ext4Inode::new("/", InodeTypes::EXT4_DE_DIR)));
    let root = root.map(|inode| inode as Arc<dyn VfsInode>);
    let lwext4_superblock = Arc::new(Ext4SuperBlock::new(
        SuperBlockInner::new(Some(BLOCK_DEVICE.clone()), root)));

    FS_MANAGER.exclusive_access().insert(DISK_FS_NAME.to_string(), lwext4_superblock);
    info!("lwext4 finish init");
}
