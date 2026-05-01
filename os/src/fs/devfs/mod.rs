///
pub mod fstype;
///
pub mod host;
///
pub mod null;
///
pub mod rtc;
///
pub mod superblock;
///
pub mod tty;
///
pub mod urandom;
///
pub mod zero;

use crate::drivers::BLOCK_DEVICE;
use crate::fs::vfs::{Dentry, dcache::GLOBAL_DCACHE};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use log::*;

use crate::fs::devfs::null::{NullDentry, NullInode};
use crate::fs::devfs::rtc::{RtcDentry, RtcInode};
use crate::fs::devfs::tty::{TtyDentry, TtyInode};
use crate::fs::devfs::urandom::{UrandomDentry, UrandomInode};
use crate::fs::devfs::zero::{ZeroDentry, ZeroInode};

/// init the /dev
pub fn init_devfs(root_dentry: Arc<dyn Dentry>) {
    // add /dev/null
    let null_dentry = NullDentry::new("null", Some(root_dentry.clone()));
    let null_inode = Arc::new(NullInode::new());
    null_dentry.set_inode(null_inode);
    root_dentry.add_child(null_dentry.clone());
    GLOBAL_DCACHE.insert("/dev/null".to_string(), null_dentry.clone());
    info!("/dev/null initialized successfully.");

    // add /dev/zero
    let zero_dentry = ZeroDentry::new("zero", Some(root_dentry.clone()));
    let zero_inode = Arc::new(ZeroInode::new());
    zero_dentry.set_inode(zero_inode);
    root_dentry.add_child(zero_dentry.clone());
    GLOBAL_DCACHE.insert("/dev/zero".to_string(), zero_dentry.clone());
    info!("/dev/zero initialized successfully.");

    // add /dev/tty
    let tty_dentry = TtyDentry::new("tty", Some(root_dentry.clone()));
    let tty_inode = Arc::new(TtyInode::new());
    tty_dentry.set_inode(tty_inode);
    root_dentry.add_child(tty_dentry.clone());
    GLOBAL_DCACHE.insert("/dev/tty".to_string(), tty_dentry.clone());
    info!("/dev/tty initialized successfully.");

    // add /dev/rtc0 and /dev/rtc
    let rtc0_dentry = RtcDentry::new("rtc0", Some(root_dentry.clone()));
    let rtc0_inode = Arc::new(RtcInode::new());
    rtc0_dentry.set_inode(rtc0_inode);
    root_dentry.add_child(rtc0_dentry.clone());
    GLOBAL_DCACHE.insert("/dev/rtc0".to_string(), rtc0_dentry.clone());
    info!("/dev/rtc0 initialized successfully.");

    let rtc_dentry = RtcDentry::new("rtc", Some(root_dentry.clone()));
    let rtc_inode = Arc::new(RtcInode::new());
    rtc_dentry.set_inode(rtc_inode);
    root_dentry.add_child(rtc_dentry.clone());
    GLOBAL_DCACHE.insert("/dev/rtc".to_string(), rtc_dentry.clone());
    info!("/dev/rtc initialized successfully.");

    // add /dev/urandom
    let urandom_dentry = UrandomDentry::new("urandom", Some(root_dentry.clone()));
    let urandom_inode = Arc::new(UrandomInode::new());
    urandom_dentry.set_inode(urandom_inode);
    root_dentry.add_child(urandom_dentry.clone());
    GLOBAL_DCACHE.insert("/dev/urandom".to_string(), urandom_dentry.clone());
    info!("/dev/urandom initialized successfully.");
}
