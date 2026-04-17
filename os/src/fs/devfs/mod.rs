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


use alloc::string::{String, ToString};
use alloc::sync::Arc;
use log::*;
use crate::drivers::BLOCK_DEVICE;
use crate::fs::vfs::{
    dcache::GLOBAL_DCACHE,
    Dentry,
};

use crate::fs::devfs::null::{NullDentry, NullInode};
use crate::fs::devfs::tty::{TtyDentry,TtyInode};

/// init the /dev
pub fn init_devfs(root_dentry: Arc<dyn Dentry>) {

    // add /dev/null
    let null_dentry = NullDentry::new("null", Some(root_dentry.clone()));
    let null_inode = Arc::new(NullInode::new());
    null_dentry.set_inode(null_inode);
    root_dentry.add_child(null_dentry.clone());
    GLOBAL_DCACHE.insert("/dev/null".to_string(), null_dentry.clone());
    info!("/dev/null initialized successfully.");

    // add /dev/tty
    let tty_dentry = TtyDentry::new("tty", Some(root_dentry.clone()));
    let tty_inode = Arc::new(TtyInode::new());
    tty_dentry.set_inode(tty_inode);
    root_dentry.add_child(tty_dentry.clone());
    GLOBAL_DCACHE.insert("/dev/tty".to_string(), tty_dentry.clone());
    info!("/dev/tty initialized successfully.");
}
