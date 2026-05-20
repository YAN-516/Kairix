///
pub mod fstype;
///
pub mod superblock;

///
pub mod inotify;
///
pub mod maps;
///
pub mod meminfo;
///
pub mod mounts;
///
pub mod pid_dir;
///
pub mod pipe_max_size;
///
pub mod self_dir;
///
pub mod smaps;
///
pub mod tainted;

use crate::drivers::BLOCK_DEVICE;
use crate::fs::procfs::inotify::{InotifySysctlDentry, InotifySysctlInode, InotifySysctlKind};
use crate::fs::procfs::meminfo::{MeminfoDentry, MeminfoInode};
use crate::fs::procfs::mounts::{MountsDentry, MountsInode};
use crate::fs::procfs::pipe_max_size::{PipeMaxSizeDentry, PipeMaxSizeInode};
use crate::fs::procfs::self_dir::ProcSelfDirDentry;
use crate::fs::procfs::tainted::{TaintedDentry, TaintedInode};
use crate::fs::tmpfs::dentry::TempDentry;
use crate::fs::tmpfs::inode::TempInode;
use crate::fs::vfs::inode::InodeMode;
use crate::fs::vfs::{dcache::GLOBAL_DCACHE, Dentry};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use log::*;

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
    GLOBAL_DCACHE.insert(
        "/proc/sys/kernel/tainted".to_string(),
        tainted_dentry.clone(),
    );
    info!("/proc/sys/kernel/tainted initialized successfully.");

    // add /proc/sys/fs directory
    let fs_dentry = crate::fs::tmpfs::dentry::TempDentry::new("fs", Some(sys_dentry.clone()));
    let fs_inode = Arc::new(crate::fs::tmpfs::inode::TempInode::new(InodeMode::DIR));
    fs_dentry.set_inode(fs_inode);
    sys_dentry.add_child(fs_dentry.clone());
    GLOBAL_DCACHE.insert("/proc/sys/fs".to_string(), fs_dentry.clone());
    info!("/proc/sys/fs initialized successfully.");

    // add /proc/sys/fs/pipe-max-size
    let pipe_max_size_dentry = PipeMaxSizeDentry::new("pipe-max-size", Some(fs_dentry.clone()));
    let pipe_max_size_inode = Arc::new(PipeMaxSizeInode::new());
    pipe_max_size_dentry.set_inode(pipe_max_size_inode);
    fs_dentry.add_child(pipe_max_size_dentry.clone());
    GLOBAL_DCACHE.insert(
        "/proc/sys/fs/pipe-max-size".to_string(),
        pipe_max_size_dentry.clone(),
    );
    info!("/proc/sys/fs/pipe-max-size initialized successfully.");

    // add /proc/sys/fs/inotify directory
    let inotify_dentry = TempDentry::new("inotify", Some(fs_dentry.clone()));
    let inotify_inode = Arc::new(TempInode::new(InodeMode::DIR));
    inotify_dentry.set_inode(inotify_inode);
    fs_dentry.add_child(inotify_dentry.clone());
    GLOBAL_DCACHE.insert("/proc/sys/fs/inotify".to_string(), inotify_dentry.clone());
    info!("/proc/sys/fs/inotify initialized successfully.");

    add_inotify_sysctl(
        inotify_dentry.clone(),
        "max_user_instances",
        InotifySysctlKind::MaxUserInstances,
    );
    add_inotify_sysctl(
        inotify_dentry.clone(),
        "max_user_watches",
        InotifySysctlKind::MaxUserWatches,
    );
    add_inotify_sysctl(
        inotify_dentry,
        "max_queued_events",
        InotifySysctlKind::MaxQueuedEvents,
    );
}

fn add_inotify_sysctl(parent: Arc<dyn Dentry>, name: &str, kind: InotifySysctlKind) {
    let dentry = InotifySysctlDentry::new(name, Some(parent.clone()), kind);
    let inode = Arc::new(InotifySysctlInode::new());
    dentry.set_inode(inode);
    parent.add_child(dentry.clone());
    GLOBAL_DCACHE.insert(
        alloc::format!("/proc/sys/fs/inotify/{}", name),
        dentry.clone(),
    );
    info!("/proc/sys/fs/inotify/{} initialized successfully.", name);
}
