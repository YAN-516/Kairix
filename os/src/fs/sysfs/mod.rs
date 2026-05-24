pub mod sysfs_block;
use alloc::sync::Arc;
use crate::fs::Dentry;
use crate::fs::tmpfs::dentry::TempDentry;
use crate::fs::tmpfs::inode::TempInode;
use crate::fs::InodeMode;
use crate::fs::GLOBAL_DCACHE;
use crate::alloc::string::ToString;
use log::*;
use crate::fs::SysfsStatDentry;
use crate::fs::SysfsStatInode;
///
pub fn init_sysfs(root_dentry: Arc<dyn Dentry>){
    let block_dentry =TempDentry::new("block", Some(root_dentry.clone()));
    let block_inode = Arc::new(TempInode::new(InodeMode::DIR));
    block_dentry.set_inode(block_inode);
    root_dentry.add_child(block_dentry.clone());
    GLOBAL_DCACHE.insert("/sys/block".to_string(), block_dentry.clone());
    info!("[FS] insert path: /sys/block");

    let loop0_dentry = crate::fs::tmpfs::dentry::TempDentry::new("loop0", Some(block_dentry.clone()));
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
}