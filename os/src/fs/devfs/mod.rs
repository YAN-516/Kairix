///
pub mod fstype;
///
pub mod null;
///
pub mod superblock;
///
pub mod tty;
///
pub mod urandom;
use alloc::collections::btree_map::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use lazy_static::lazy_static;
use log::*;

use lwext4_rust::InodeTypes;

use crate::drivers::BLOCK_DEVICE;
use crate::sync::UPSafeCell;

use crate::fs::{SuperBlock, SuperBlockInner};
use crate::fs::vfs::{
    dcache::GLOBAL_DCACHE,
    inode::Inode,
    Dentry,
};

use crate::fs::devfs::null::{NullDentry, NullInode};

use crate::fs::lwext4::{
    dentry::Ext4Dentry,
    inode::Ext4Inode,
};

use crate::fs::devfs::tty::TtyDentry;
use crate::fs::devfs::tty::TtyInode;

/// init the /dev
pub fn init_devfs(root_dentry: Arc<dyn Dentry>) {

    // add /dev/null
    let null_dentry = Arc::new(NullDentry::new("null", Some(Arc::downgrade(&root_dentry))));
    let null_inode = Arc::new(NullInode::new());
    null_dentry.set_inode(null_inode);
    root_dentry.add_child(null_dentry.clone());
    GLOBAL_DCACHE.insert("/dev/null".to_string(), null_dentry.clone());
    info!("/dev/null initialized successfully.");

    // add /dev/tty
    let tty_dentry = Arc::new(TtyDentry::new("tty", Some(Arc::downgrade(&root_dentry))));
    let tty_inode = Arc::new(TtyInode::new());
    tty_dentry.set_inode(tty_inode);
    root_dentry.add_child(tty_dentry.clone());
    GLOBAL_DCACHE.insert("/dev/tty".to_string(), tty_dentry.clone());
    info!("/dev/tty initialized successfully.");
}
