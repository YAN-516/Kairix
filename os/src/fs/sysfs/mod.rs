pub mod sysfs_block;
use crate::alloc::string::ToString;
use crate::fs::Dentry;
use crate::fs::GLOBAL_DCACHE;
use crate::fs::InodeMode;
use crate::fs::SysfsStatDentry;
use crate::fs::SysfsStatInode;
use crate::fs::sysfs::sysfs_block::{SysfsTextDentry, SysfsTextInode};
use crate::fs::tmpfs::dentry::TempDentry;
use crate::fs::tmpfs::inode::TempInode;
use alloc::sync::Arc;
use log::*;

fn add_dir(parent: &Arc<dyn Dentry>, path: &str, name: &str) -> Arc<dyn Dentry> {
    let dentry = TempDentry::new(name, Some(parent.clone()));
    let inode = Arc::new(TempInode::new(InodeMode::DIR));
    dentry.set_inode(inode);
    parent.add_child(dentry.clone());
    GLOBAL_DCACHE.insert(path.to_string(), dentry.clone());
    info!("[FS] insert path: {}", path);
    dentry
}

fn add_text_file(parent: &Arc<dyn Dentry>, path: &str, name: &str, content: &'static str) {
    let dentry = SysfsTextDentry::new(name, Some(parent.clone()), content);
    let inode = Arc::new(SysfsTextInode::new(content.len()));
    dentry.set_inode(inode);
    parent.add_child(dentry.clone());
    GLOBAL_DCACHE.insert(path.to_string(), dentry);
    info!("[FS] insert path: {}", path);
}

///
pub fn init_sysfs(root_dentry: Arc<dyn Dentry>) {
    let block_dentry = add_dir(&root_dentry, "/sys/block", "block");

    let loop0_dentry =
        crate::fs::tmpfs::dentry::TempDentry::new("loop0", Some(block_dentry.clone()));
    let loop0_inode = Arc::new(crate::fs::tmpfs::inode::TempInode::new(InodeMode::DIR));
    loop0_dentry.set_inode(loop0_inode);
    block_dentry.add_child(loop0_dentry.clone());
    GLOBAL_DCACHE.insert("/sys/block/loop0".to_string(), loop0_dentry.clone());
    info!("[FS] insert path: /sys/block/loop0");

    let stat_dentry = SysfsStatDentry::new("stat", Some(loop0_dentry.clone()));
    let stat_inode = Arc::new(SysfsStatInode::new());
    stat_dentry.set_inode(stat_inode);
    loop0_dentry.add_child(stat_dentry.clone());
    GLOBAL_DCACHE.insert("/sys/block/loop0/stat".to_string(), stat_dentry.clone());
    info!("[FS] insert path: /sys/block/loop0/stat");

    let devices_dentry = add_dir(&root_dentry, "/sys/devices", "devices");
    let system_dentry = add_dir(
        &(devices_dentry.clone() as Arc<dyn Dentry>),
        "/sys/devices/system",
        "system",
    );
    let node_dentry = add_dir(
        &(system_dentry.clone() as Arc<dyn Dentry>),
        "/sys/devices/system/node",
        "node",
    );
    let node0_dentry = add_dir(
        &(node_dentry.clone() as Arc<dyn Dentry>),
        "/sys/devices/system/node/node0",
        "node0",
    );

    add_text_file(
        &(node_dentry.clone() as Arc<dyn Dentry>),
        "/sys/devices/system/node/online",
        "online",
        "0\n",
    );
    add_text_file(
        &(node_dentry.clone() as Arc<dyn Dentry>),
        "/sys/devices/system/node/possible",
        "possible",
        "0\n",
    );
    add_text_file(
        &(node_dentry.clone() as Arc<dyn Dentry>),
        "/sys/devices/system/node/has_cpu",
        "has_cpu",
        "0\n",
    );
    add_text_file(
        &(node_dentry.clone() as Arc<dyn Dentry>),
        "/sys/devices/system/node/has_memory",
        "has_memory",
        "0\n",
    );
    add_text_file(
        &(node0_dentry.clone() as Arc<dyn Dentry>),
        "/sys/devices/system/node/node0/cpulist",
        "cpulist",
        "0\n",
    );
    add_text_file(
        &(node0_dentry.clone() as Arc<dyn Dentry>),
        "/sys/devices/system/node/node0/cpumap",
        "cpumap",
        "1\n",
    );
    add_text_file(
        &(node0_dentry.clone() as Arc<dyn Dentry>),
        "/sys/devices/system/node/node0/meminfo",
        "meminfo",
        "Node 0 MemTotal: 1048576 kB\nNode 0 MemFree: 1048576 kB\n",
    );
}
