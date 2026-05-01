use crate::fs::Dentry;
use crate::fs::GLOBAL_DCACHE;
use crate::fs::tempfs::dentry::TempDentry;
use crate::fs::tempfs::file::TempFile;
use crate::fs::tempfs::inode::TempInode;
use crate::fs::vfs::file::File;
use crate::fs::vfs::inode::InodeMode;
use crate::mm::UserBuffer;
use alloc::boxed::Box;
use alloc::string::ToString;
use alloc::sync::Arc;
use alloc::vec;
use log::*;

#[allow(unused)]
/// 初始化 /etc 挂载点中的默认文件。
pub fn init_etcfs(root_dentry: Arc<dyn Dentry>) {
    // add /etc/passwd
    let passwd_dentry = TempDentry::new("passwd", Some(root_dentry.clone()));
    let passwd_inode = Arc::new(TempInode::new(InodeMode::FILE));
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

    // add /etc/host with content
    let host_dentry = TempDentry::new("host", Some(root_dentry.clone()));
    let host_inode = Arc::new(TempInode::new(InodeMode::FILE));
    host_dentry.set_inode(host_inode.clone());

    let host_file = TempFile::new(host_dentry.clone());
    static CONTENT_HOST: &str = "127.0.0.1\tlocalhost\n127.0.1.1\tkairix\n";

    let data: &'static mut [u8] = Box::leak(CONTENT_HOST.as_bytes().to_vec().into_boxed_slice());
    let user_buf = UserBuffer::new(vec![data]);

    if let Ok(written) = host_file.write(user_buf) {
        if written > 0 {
            info!("/etc/host written with {} bytes", written);
        } else {
            error!("Failed to write /etc/host");
        }
    } else {
        error!("Failed to write /etc/host");
    }

    root_dentry.add_child(host_dentry.clone());
    GLOBAL_DCACHE.insert("/etc/host".to_string(), host_dentry.clone());
    info!("/etc/host initialized successfully.");

    // add /etc/hosts with content
    let hosts_dentry = TempDentry::new("hosts", Some(root_dentry.clone()));
    let hosts_inode = Arc::new(TempInode::new(InodeMode::FILE));
    hosts_dentry.set_inode(hosts_inode.clone());
    let hosts_file = TempFile::new(hosts_dentry.clone());

    // 使用静态字符串，生命周期为 'static
    static CONTENT_HOSTS: &str = "127.0.0.1\tlocalhost localhost.localdomain\n\
                            ::1\t\tlocalhost ip6-localhost ip6-loopback\n\
                            127.0.1.1\tkairix\n";

    let data: &'static mut [u8] = Box::leak(CONTENT_HOSTS.as_bytes().to_vec().into_boxed_slice());
    let user_buf = UserBuffer::new(vec![data]);

    if let Ok(written) = hosts_file.write(user_buf) {
        if written > 0 {
            info!("/etc/hosts written with {} bytes", written);
        } else {
            error!("Failed to write /etc/hosts");
        }
    } else {
        error!("Failed to write /etc/hosts");
    }

    root_dentry.add_child(hosts_dentry.clone());
    GLOBAL_DCACHE.insert("/etc/hosts".to_string(), hosts_dentry.clone());
    info!("/etc/hosts initialized successfully.");
}
