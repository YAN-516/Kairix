use alloc::sync::Arc;

use crate::fs::Dentry;
use crate::fs::tempfs::dentry::TempDentry;
use crate::fs::tempfs::inode::TempInode;
use crate::fs::vfs::inode::InodeMode;
use crate::fs::GLOBAL_DCACHE;
use crate::fs::vfs::inode::inode_alloc;
use crate::alloc::string::ToString;
use log::*;

#[allow(unused)]
///
pub fn init_etcfs(root_dentry: Arc<dyn Dentry>) {
    // add /etc/passwd
    let passwd_dentry = TempDentry::new("passwd", Some(root_dentry.clone()));
    let passwd_inode = Arc::new(TempInode::new( InodeMode::FILE));
    passwd_dentry.set_inode(passwd_inode);
    root_dentry.add_child(passwd_dentry.clone());
    GLOBAL_DCACHE.insert("/etc/passwd".to_string(), passwd_dentry.clone());
    info!("/etc/passwd initialized successfully.");

    // add /etc/adjtime
    let adjtime_dentry = TempDentry::new("adjtime", Some(root_dentry.clone()));
    let adjtime_inode = Arc::new(TempInode::new(InodeMode::FILE));
    adjtime_dentry.set_inode(adjtime_inode);
    root_dentry.add_child(adjtime_dentry.clone());
    GLOBAL_DCACHE.insert("/etc/adjtime".to_string(), adjtime_dentry.clone());
    info!("/etc/adjtime initialized successfully.");

    // add /etc/group
    let group_dentry = TempDentry::new("group", Some(root_dentry.clone()));
    let group_inode = Arc::new(TempInode::new(InodeMode::FILE));
    group_dentry.set_inode(group_inode);
    root_dentry.add_child(group_dentry.clone());
    GLOBAL_DCACHE.insert("/etc/group".to_string(), group_dentry.clone());
    info!("/etc/group initialized successfully.");

    // add /etc/localtime
    let localtime_dentry = TempDentry::new("localtime", Some(root_dentry.clone()));
    let localtime_inode = Arc::new(TempInode::new(InodeMode::FILE));
    localtime_dentry.set_inode(localtime_inode);
    root_dentry.add_child(localtime_dentry.clone());
    GLOBAL_DCACHE.insert("/etc/localtime".to_string(), localtime_dentry.clone());
    info!("/etc/localtime initialized successfully.");
}
