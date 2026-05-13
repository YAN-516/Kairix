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
pub mod pid_max;
pub mod net_ipv4_conf;

/// NetNsTagKind: lo or default
#[derive(Clone, Copy)]
pub enum NetNsTagKind {
    /// lo tag
    Lo,
    /// default tag
    Default,
}

use alloc::string::{String, ToString};
use alloc::format;
use alloc::vec;
use alloc::sync::Arc;
use crate::fs::tempfs::dentry::TempDentry;
use log::*;
use crate::drivers::BLOCK_DEVICE;
use crate::fs::vfs::{
    dcache::GLOBAL_DCACHE,
    Dentry,
    OpenFlags,
};
use crate::fs::procfs::mounts::{MountsDentry,MountsInode};
use crate::fs::procfs::meminfo::{MeminfoDentry, MeminfoInode};
use crate::fs::procfs::self_dir::ProcSelfDirDentry;
use crate::fs::procfs::pid_max::{PidMaxDentry, PidMaxInode};
use crate::fs::tempfs::inode::TempInode;
use crate::fs::vfs::inode::InodeMode;
use crate::mm::UserBuffer;

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

    // add /proc/sys/kernel/pid_max
    let sys_dir_dentry = TempDentry::new("sys", Some(root_dentry.clone()));
    let sys_dir_inode = Arc::new(TempInode::new(InodeMode::DIR));
    sys_dir_dentry.set_inode(sys_dir_inode);
    root_dentry.add_child(sys_dir_dentry.clone());
    GLOBAL_DCACHE.insert("/proc/sys".to_string(), sys_dir_dentry.clone());

    let kernel_dir_dentry = TempDentry::new("kernel", Some(sys_dir_dentry.clone()));
    let kernel_dir_inode = Arc::new(TempInode::new(InodeMode::DIR));
    kernel_dir_dentry.set_inode(kernel_dir_inode);
    sys_dir_dentry.add_child(kernel_dir_dentry.clone());
    GLOBAL_DCACHE.insert("/proc/sys/kernel".to_string(), kernel_dir_dentry.clone());

    let pid_max_dentry = PidMaxDentry::new("pid_max", Some(kernel_dir_dentry.clone()));
    let pid_max_inode = Arc::new(PidMaxInode::new());
    pid_max_dentry.set_inode(pid_max_inode);
    kernel_dir_dentry.add_child(pid_max_dentry.clone());
    GLOBAL_DCACHE.insert("/proc/sys/kernel/pid_max".to_string(), pid_max_dentry.clone());
    info!("/proc/sys/kernel/pid_max initialized successfully.");

    // 为 clone09 创建 /proc/sys/net/ipv4/conf/lo/tag 和 default/tag
    let net_dir = TempDentry::new("net", Some(sys_dir_dentry.clone()));
    net_dir.set_inode(Arc::new(TempInode::new(InodeMode::DIR)));
    sys_dir_dentry.add_child(net_dir.clone());
    GLOBAL_DCACHE.insert("/proc/sys/net".to_string(), net_dir.clone());

    let ipv4_dir = TempDentry::new("ipv4", Some(net_dir.clone()));
    ipv4_dir.set_inode(Arc::new(TempInode::new(InodeMode::DIR)));
    net_dir.add_child(ipv4_dir.clone());
    GLOBAL_DCACHE.insert("/proc/sys/net/ipv4".to_string(), ipv4_dir.clone());

    let conf_dir = TempDentry::new("conf", Some(ipv4_dir.clone()));
    conf_dir.set_inode(Arc::new(TempInode::new(InodeMode::DIR)));
    ipv4_dir.add_child(conf_dir.clone());
    GLOBAL_DCACHE.insert("/proc/sys/net/ipv4/conf".to_string(), conf_dir.clone());

    for (dir_name, kind) in [("lo", NetNsTagKind::Lo), ("default", NetNsTagKind::Default)] {
        let sub_dir = TempDentry::new(dir_name, Some(conf_dir.clone()));
        sub_dir.set_inode(Arc::new(TempInode::new(InodeMode::DIR)));
        conf_dir.add_child(sub_dir.clone());
        let sub_path = format!("/proc/sys/net/ipv4/conf/{}", dir_name);
        GLOBAL_DCACHE.insert(sub_path.clone(), sub_dir.clone());

        let tag_dentry = net_ipv4_conf::NetNsTagDentry::new("tag", Some(sub_dir.clone()), kind);
        let tag_inode = Arc::new(net_ipv4_conf::NetNsTagInode::new());
        tag_dentry.set_inode(tag_inode);
        sub_dir.add_child(tag_dentry.clone());
        let tag_path = format!("{}/tag", sub_path);
        GLOBAL_DCACHE.insert(tag_path.clone(), tag_dentry.clone());
        info!("{} initialized successfully.", tag_path);
    }
}
