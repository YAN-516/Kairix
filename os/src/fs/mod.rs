//参考chronix
///
pub mod devfs;
///
pub mod etc;
///
pub mod lwext4;
///
pub mod page;
///
pub mod procfs;
///
pub mod tempfs;
pub mod fat32;
pub mod vfs;
use alloc::boxed::Box;
use alloc::collections::btree_map::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use lazy_static::lazy_static;
use log::*;
use lwext4_rust::InodeTypes;
use spin::mutex::Mutex;

pub use self::lwext4::file::Ext4File;
pub use self::lwext4::superblock::Ext4SuperBlock;
pub use self::vfs::file::File;
pub use self::vfs::superblock::{SuperBlock, SuperBlockInner};
use crate::drivers::BLOCK_DEVICE;
use crate::fs::devfs::fstype::DevFsType;
use crate::fs::devfs::init_devfs;
use crate::fs::etc::init_etcfs;
use crate::fs::fat32::fstype::Fat32FsType;
use crate::fs::lwext4::{dentry::Ext4Dentry, fstype::Ext4FsType, inode::Ext4Inode};
use crate::fs::procfs::fstype::ProcFsType;
use crate::fs::procfs::init_procfs;
use crate::fs::tempfs::fstype::TempFsType;
use crate::fs::tempfs::init_tempfs;
use crate::fs::vfs::{
    Dentry,
    dcache::GLOBAL_DCACHE,
    fstype::{FsType, MountFlags},
    inode::{Inode, InodeMode},
    path::resolve_path,
};
///
pub static FS_MANAGER: Mutex<BTreeMap<String, Arc<dyn FsType>>> = Mutex::new(BTreeMap::new());

/// the name of disk fs
pub const DISK_FS_NAME: &str = "ext4";

/// 根据绝对路径查找对应的 superblock（最长前缀匹配）
pub fn find_superblock_by_path(path: &str) -> Option<Arc<dyn SuperBlock>> {
    let fs_mgr = FS_MANAGER.lock();
    let mut best_sb: Option<Arc<dyn SuperBlock>> = None;
    let mut best_len = 0usize;
    for (_name, fstype) in fs_mgr.iter() {
        let supers = fstype.inner().supers.lock();
        for (mp, sb) in supers.iter() {
            if path.starts_with(mp) {
                let matched = if mp.ends_with('/') {
                    true
                } else {
                    path.len() == mp.len() || path.as_bytes().get(mp.len()) == Some(&b'/')
                };
                if matched && mp.len() >= best_len {
                    best_len = mp.len();
                    best_sb = Some(sb.clone());
                }
            }
        }
    }
    best_sb
}
/// register all filesystem
fn register_all_fs() {
    let diskfs = Ext4FsType::new(DISK_FS_NAME);
    FS_MANAGER.lock().insert(diskfs.name().to_string(), diskfs);

    let fat32fs = Fat32FsType::new("fat32");
    FS_MANAGER.lock().insert(fat32fs.name().to_string(), fat32fs);

    let devfs = DevFsType::new("devfs");
    FS_MANAGER.lock().insert(devfs.name().to_string(), devfs);

    let etcfs = TempFsType::new("etc");
    FS_MANAGER.lock().insert(etcfs.name().to_string(), etcfs);

    let procfs = ProcFsType::new("proc");
    FS_MANAGER.lock().insert(procfs.name().to_string(), procfs);

    let tmpfs = TempFsType::new("tmpfs");
    FS_MANAGER.lock().insert(tmpfs.name().to_string(), tmpfs);
}

/// get the file system by name
pub fn get_filesystem(name: &str) -> &'static Arc<dyn FsType> {
    let arc = FS_MANAGER.lock().get(name).unwrap().clone();
    Box::leak(Box::new(arc))
}

/// init the file system
pub fn init() {
    register_all_fs();

    //mount the root fs
    let rootfs = get_filesystem("ext4");
    let root_dentry = rootfs
        .mount("/", None, MountFlags::empty(), Some(BLOCK_DEVICE.clone()))
        .unwrap();
    GLOBAL_DCACHE.insert("/".to_string(), root_dentry.clone());
    GLOBAL_DCACHE.pin("/".to_string());

    //mount the devfs
    let devfs = get_filesystem("devfs");
    let devfs_dentry = devfs
        .mount("dev", Some(root_dentry.clone()), MountFlags::empty(), None)
        .unwrap();
    init_devfs(devfs_dentry.clone());
    root_dentry.add_child(devfs_dentry.clone());
    info!("[FS] insert path: {}", devfs_dentry.path());
    GLOBAL_DCACHE.insert(devfs_dentry.path(), devfs_dentry.clone());
    GLOBAL_DCACHE.pin(devfs_dentry.path());

    // mount /dev/shm (required by shm_open)
    let shm_tmpfs = get_filesystem("tmpfs");
    let shm_dentry = shm_tmpfs
        .mount("shm", Some(devfs_dentry.clone()), MountFlags::empty(), None)
        .unwrap();
    devfs_dentry.add_child(shm_dentry.clone());
    info!("[FS] insert path: {}", shm_dentry.path());
    GLOBAL_DCACHE.insert(shm_dentry.path(), shm_dentry.clone());
    GLOBAL_DCACHE.pin(shm_dentry.path());

    //mount the etc tmpfs
    let etcfs = get_filesystem("etc");
    let etc_dentry = etcfs
        .mount("etc", Some(root_dentry.clone()), MountFlags::empty(), None)
        .unwrap();
    init_etcfs(etc_dentry.clone());
    root_dentry.add_child(etc_dentry.clone());
    info!("[FS] insert path: {}", etc_dentry.path());
    GLOBAL_DCACHE.insert(etc_dentry.path(), etc_dentry.clone());
    GLOBAL_DCACHE.pin(etc_dentry.path());

    //mount the proc
    let procfs = get_filesystem("proc");
    let proc_dentry = procfs
        .mount("proc", Some(root_dentry.clone()), MountFlags::empty(), None)
        .unwrap();
    init_procfs(proc_dentry.clone());
    root_dentry.add_child(proc_dentry.clone());
    info!("[FS] insert path: {}", proc_dentry.path());
    GLOBAL_DCACHE.insert(proc_dentry.path(), proc_dentry.clone());
    GLOBAL_DCACHE.pin(proc_dentry.path());

    //mount the tmpfs
    let tmpfs = get_filesystem("tmpfs");
    let tmp_dentry = tmpfs
        .mount("tmp", Some(root_dentry.clone()), MountFlags::empty(), None)
        .unwrap();
    init_tempfs(tmp_dentry.clone());
    root_dentry.add_child(tmp_dentry.clone());
    info!("[FS] insert path: {}", tmp_dentry.path());
    GLOBAL_DCACHE.insert(tmp_dentry.path(), tmp_dentry.clone());
    GLOBAL_DCACHE.pin(tmp_dentry.path());

    // // 兼容 musl/glibc/libctest：确保临时目录存在，避免 mkstemp("/tmp/...") 因父目录不存在失败。
    // if resolve_path(root_dentry.clone(), "/tmp").is_none() {
    //     let _ = root_dentry.create("tmp", InodeMode::DIR);
    // }
    // if resolve_path(root_dentry.clone(), "/var").is_none() {
    //     let _ = root_dentry.create("var", InodeMode::DIR);
    // }
    // if let Some(var_dentry) = resolve_path(root_dentry.clone(), "/var") {
    //     if resolve_path(root_dentry.clone(), "/var/tmp").is_none() {
    //         let _ = var_dentry.create("tmp", InodeMode::DIR);
    //     }
    // }
}
