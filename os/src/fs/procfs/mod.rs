///
pub mod fstype;
///
pub mod superblock;

///
pub mod mounts;
///
pub mod meminfo;
///
pub mod self_dir;
///
pub mod smaps;
///
pub mod maps;
///
pub mod tainted;
///
pub mod pagemap;
///
pub mod status;


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
use crate::fs::procfs::self_dir::ProcSelfDirDentry;
use crate::fs::procfs::tainted::{TaintedDentry, TaintedInode};
use crate::fs::tempfs::dentry::TempDentry;
use crate::fs::tempfs::inode::TempInode;
use crate::fs::vfs::inode::InodeMode;

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

    // add /proc/self
    let self_dir_dentry = ProcSelfDirDentry::new("self", Some(root_dentry.clone()));
    let self_dir_inode = Arc::new(TempInode::new(InodeMode::DIR));
    self_dir_dentry.set_inode(self_dir_inode);
    root_dentry.add_child(self_dir_dentry.clone());
    GLOBAL_DCACHE.insert("/proc/self".to_string(), self_dir_dentry.clone());
    info!("/proc/self initialized successfully.");

    // add /proc/sys directory
    let sys_dentry = TempDentry::new("sys", Some(root_dentry.clone()));
    let sys_inode = Arc::new(TempInode::new(InodeMode::DIR));
    sys_dentry.set_inode(sys_inode);
    root_dentry.add_child(sys_dentry.clone());
    GLOBAL_DCACHE.insert("/proc/sys".to_string(), sys_dentry.clone());
    info!("/proc/sys initialized successfully.");

    // add /proc/sys/kernel directory
    let kernel_dentry = TempDentry::new("kernel", Some(sys_dentry.clone()));
    let kernel_inode = Arc::new(TempInode::new(InodeMode::DIR));
    kernel_dentry.set_inode(kernel_inode);
    sys_dentry.add_child(kernel_dentry.clone());
    GLOBAL_DCACHE.insert("/proc/sys/kernel".to_string(), kernel_dentry.clone());
    info!("/proc/sys/kernel initialized successfully.");

    // add /proc/sys/kernel/tainted
    let tainted_dentry = TaintedDentry::new("tainted", Some(kernel_dentry.clone()));
    let tainted_inode = Arc::new(TaintedInode::new());
    tainted_dentry.set_inode(tainted_inode);
    kernel_dentry.add_child(tainted_dentry.clone());
    GLOBAL_DCACHE.insert("/proc/sys/kernel/tainted".to_string(), tainted_dentry.clone());
    info!("/proc/sys/kernel/tainted initialized successfully.");
}