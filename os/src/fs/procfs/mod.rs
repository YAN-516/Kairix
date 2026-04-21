///
pub mod fstype;
///
pub mod superblock;

///
pub mod mounts;
///
pub mod meminfo;



use alloc::string::{String, ToString};
use alloc::sync::Arc;
use log::*;
use crate::drivers::BLOCK_DEVICE;
use crate::fs::vfs::{
    dcache::GLOBAL_DCACHE,
    Dentry,
};
use crate::fs::procfs::mounts::{MountsDentry,MountsInode};
use crate::fs::procfs::meminfo::{MeminfoDentry, MeminfoInode};

/// init the /proc
pub fn init_procfs(root_dentry: Arc<dyn Dentry>) {

    // add /proc/mounts
    let mounts_dentry = MountsDentry::new("mounts", Some(root_dentry.clone()));
    let mounts_inode = Arc::new(MountsInode::new());
    mounts_dentry.set_inode(mounts_inode);
    root_dentry.add_child(mounts_dentry.clone());
    GLOBAL_DCACHE.insert("/proc/mounts".to_string(), mounts_dentry.clone());
    info!("/proc/mounts initialized successfully.");

    // add /proc/meminfo
    let meminfo_dentry = MeminfoDentry::new("meminfo", Some(root_dentry.clone()));
    let meminfo_inode = Arc::new(MeminfoInode::new());
    meminfo_dentry.set_inode(meminfo_inode);
    root_dentry.add_child(meminfo_dentry.clone());
    GLOBAL_DCACHE.insert("/proc/meminfo".to_string(), meminfo_dentry.clone());
    info!("/proc/meminfo initialized successfully.");


}
