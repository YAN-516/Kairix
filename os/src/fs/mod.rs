//参考chronix
///
pub mod devfs;
///
pub mod lwext4;
///
pub mod page;
pub mod vfs;
///
pub mod tempfs;
///
pub mod etc;
use alloc::boxed::Box;
use alloc::collections::btree_map::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use lazy_static::lazy_static;
use log::*;
use lwext4_rust::InodeTypes;
use spin::mutex::Mutex;

use crate::drivers::BLOCK_DEVICE;
use crate::fs::etc::init_etcfs;
use crate::sync::UPSafeCell;
use crate::fs::devfs::init_devfs;
use crate::fs::lwext4::{
    dentry::Ext4Dentry, 
    fstype::Ext4FSType, 
    inode::Ext4Inode,
};
use crate::fs::vfs::{
    dcache::GLOBAL_DCACHE,
    fstype::{FSType, MountFlags},
    inode::Inode,
    Dentry,
};

pub use self::vfs::file::File;
pub use self::vfs::superblock::{SuperBlock, SuperBlockInner};
pub use self::lwext4::file::{Ext4File};
pub use self::lwext4::superblock::Ext4SuperBlock;
use crate::fs::devfs::fstype::DevFsType;
use crate::fs::tempfs::fstype::TempFSType;
///
pub static FS_MANAGER: Mutex<BTreeMap<String, Arc<dyn FSType>>> =
    Mutex::new(BTreeMap::new());

/// the name of disk fs
pub const DISK_FS_NAME: &str = "ext4";
/// register all filesystem
fn register_all_fs() {
    let diskfs = Ext4FSType::new(DISK_FS_NAME);
    FS_MANAGER.lock().insert(diskfs.name().to_string(), diskfs);

    let devfs = DevFsType::new("devfs");
    FS_MANAGER.lock().insert(devfs.name().to_string(), devfs);

    let etcfs = TempFSType::new("etc");
    FS_MANAGER.lock().insert(etcfs.name().to_string(), etcfs);

    // let procfs = ProcFSType::new();
    // FS_MANAGER.lock().insert(procfs.name().to_string(), procfs);

    // let tmpfs = TmpFSType::new();
    // FS_MANAGER.lock().insert(tmpfs.name().to_string(), tmpfs);
}

/// get the file system by name
pub fn get_filesystem(name: &str) -> &'static Arc<dyn FSType> {
    let arc = FS_MANAGER.lock().get(name).unwrap().clone();
    Box::leak(Box::new(arc))
}

/// init the file system
pub fn init() {
    register_all_fs();

    //mount the root fs
    let rootfs = get_filesystem("ext4");
    let root_dentry = rootfs.mount("/", None, MountFlags::empty(), Some(BLOCK_DEVICE.clone())).unwrap();

    //mount the devfs
    let devfs = get_filesystem("devfs");
    let devfs_dentry = devfs.mount("dev", Some(root_dentry.clone()), MountFlags::empty(), None).unwrap();
    init_devfs(root_dentry.clone());
    root_dentry.add_child(devfs_dentry.clone());
    log::info!("[FS] insert path: {}", devfs_dentry.path());
    GLOBAL_DCACHE.insert(devfs_dentry.path(), devfs_dentry);

    //mount the etc tmpfs
    let etcfs = get_filesystem("etc");
    let etc_dentry = etcfs.mount("etc", Some(root_dentry.clone()), MountFlags::empty(), None).unwrap();
    init_etcfs(root_dentry.clone());
    root_dentry.add_child(etc_dentry.clone());
    log::info!("[FS] insert path: {}", etc_dentry.path());
    GLOBAL_DCACHE.insert(etc_dentry.path(), etc_dentry);

}
