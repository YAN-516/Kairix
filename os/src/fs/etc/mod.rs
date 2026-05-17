///
pub mod adjtime;
///
pub mod group;
///
pub mod host;
///
pub mod hosts;
///
pub mod localtime;
///
pub mod passwd;

use alloc::string::{String, ToString};
use alloc::sync::Arc;
use log::*;
use crate::fs::vfs::{
    dcache::GLOBAL_DCACHE,
    Dentry,
};
use crate::fs::etc::passwd::{PasswdDentry, PasswdInode};
use crate::fs::etc::group::{GroupDentry, GroupInode};
use crate::fs::etc::host::{HostDentry, HostInode};
use crate::fs::etc::hosts::{HostsDentry, HostsInode};
use crate::fs::etc::adjtime::{AdjtimeDentry, AdjtimeInode};
use crate::fs::etc::localtime::{LocaltimeDentry, LocaltimeInode};

/// init the /etc
pub fn init_etcfs(root_dentry: Arc<dyn Dentry>) {
    // add /etc/passwd
    let passwd_dentry = PasswdDentry::new("passwd", Some(root_dentry.clone()));
    let passwd_inode = Arc::new(PasswdInode::new());
    passwd_dentry.set_inode(passwd_inode);
    root_dentry.add_child(passwd_dentry.clone());
    GLOBAL_DCACHE.insert("/etc/passwd".to_string(), passwd_dentry.clone());
    info!("/etc/passwd initialized successfully.");

    // add /etc/group
    let group_dentry = GroupDentry::new("group", Some(root_dentry.clone()));
    let group_inode = Arc::new(GroupInode::new());
    group_dentry.set_inode(group_inode);
    root_dentry.add_child(group_dentry.clone());
    GLOBAL_DCACHE.insert("/etc/group".to_string(), group_dentry.clone());
    info!("/etc/group initialized successfully.");

    // add /etc/adjtime
    let adjtime_dentry = AdjtimeDentry::new("adjtime", Some(root_dentry.clone()));
    let adjtime_inode = Arc::new(AdjtimeInode::new());
    adjtime_dentry.set_inode(adjtime_inode);
    root_dentry.add_child(adjtime_dentry.clone());
    GLOBAL_DCACHE.insert("/etc/adjtime".to_string(), adjtime_dentry.clone());
    info!("/etc/adjtime initialized successfully.");

    // add /etc/localtime
    let localtime_dentry = LocaltimeDentry::new("localtime", Some(root_dentry.clone()));
    let localtime_inode = Arc::new(LocaltimeInode::new());
    localtime_dentry.set_inode(localtime_inode);
    root_dentry.add_child(localtime_dentry.clone());
    GLOBAL_DCACHE.insert("/etc/localtime".to_string(), localtime_dentry.clone());
    info!("/etc/localtime initialized successfully.");

    // add /etc/host
    let host_dentry = HostDentry::new("host", Some(root_dentry.clone()));
    let host_inode = Arc::new(HostInode::new());
    host_dentry.set_inode(host_inode);
    root_dentry.add_child(host_dentry.clone());
    GLOBAL_DCACHE.insert("/etc/host".to_string(), host_dentry.clone());
    info!("/etc/host initialized successfully.");

    // add /etc/hosts
    let hosts_dentry = HostsDentry::new("hosts", Some(root_dentry.clone()));
    let hosts_inode = Arc::new(HostsInode::new());
    hosts_dentry.set_inode(hosts_inode);
    root_dentry.add_child(hosts_dentry.clone());
    GLOBAL_DCACHE.insert("/etc/hosts".to_string(), hosts_dentry.clone());
    info!("/etc/hosts initialized successfully.");
}