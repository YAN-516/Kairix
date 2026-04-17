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
    let passwd_inode = Arc::new(TempInode::new(inode_alloc(), InodeMode::FILE));
    passwd_dentry.set_inode(passwd_inode);
    root_dentry.add_child(passwd_dentry.clone());
    GLOBAL_DCACHE.insert("/etc/passwd".to_string(), passwd_dentry.clone());
    info!("/etc/passwd initialized successfully.");

    // add /etc/adjtime
    let adjtime_dentry = TempDentry::new("adjtime", Some(root_dentry.clone()));
    let adjtime_inode = Arc::new(TempInode::new(inode_alloc(), InodeMode::FILE));
    adjtime_dentry.set_inode(adjtime_inode);
    root_dentry.add_child(adjtime_dentry.clone());
    GLOBAL_DCACHE.insert("/etc/adjtime".to_string(), adjtime_dentry.clone());
    info!("/etc/adjtime initialized successfully.");
}
