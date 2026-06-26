#![allow(missing_docs)]

//! Filesystem event notification implementations.

use crate::error::SyscallResult;
use crate::fs::vfs::{Dentry, File};
use alloc::sync::Arc;

pub mod fanotify;
pub mod inotify;

use fanotify::{
    FAN_ACCESS, FAN_ACCESS_PERM, FAN_ATTRIB, FAN_CLOSE_NOWRITE, FAN_CLOSE_WRITE, FAN_MODIFY,
    fanotify_check_permission_dentry, fanotify_may_have_instances, fanotify_notify_dentry,
    fanotify_notify_path,
};
use inotify::{
    IN_ACCESS, IN_ATTRIB, IN_CLOSE_NOWRITE, IN_CLOSE_WRITE, IN_ISDIR, IN_MODIFY,
    inotify_may_have_instances, inotify_notify_dentry, inotify_notify_path,
};

#[derive(Clone)]
pub struct NotifyTarget {
    dentry: Arc<dyn Dentry>,
}

impl NotifyTarget {
    pub fn new(dentry: Arc<dyn Dentry>) -> Self {
        Self { dentry }
    }

    pub fn dentry(&self) -> Arc<dyn Dentry> {
        self.dentry.clone()
    }

    pub fn path(&self) -> alloc::string::String {
        self.dentry.path()
    }

    fn is_dir(&self) -> bool {
        self.dentry.get_inode().is_some_and(|inode| {
            inode
                .get_mode()
                .contains(crate::fs::vfs::inode::InodeMode::DIR)
        })
    }
}

pub fn notify_target_for_file(file: &Arc<dyn File + Send + Sync>) -> Option<NotifyTarget> {
    if file.is_pipe() || file.is_socket() || file.is_path_only() {
        return None;
    }
    file.get_inode()
        .map(|_| NotifyTarget::new(file.get_dentry()))
}

pub fn notify_target_for_file_if_needed(
    file: &Arc<dyn File + Send + Sync>,
) -> Option<NotifyTarget> {
    if !inotify_may_have_instances() && !fanotify_may_have_instances() {
        return None;
    }
    notify_target_for_file(file)
}

pub fn notify_access_permission(target: Option<&NotifyTarget>) -> SyscallResult {
    if !fanotify_may_have_instances() {
        return Ok(0);
    }
    if let Some(target) = target {
        fanotify_check_permission_dentry(target.dentry(), FAN_ACCESS_PERM)?;
    }
    Ok(0)
}

pub fn notify_access(target: &NotifyTarget) {
    inotify_notify_dentry(target.dentry(), IN_ACCESS);
    fanotify_notify_dentry(target.dentry(), FAN_ACCESS);
}

pub fn notify_modify(target: &NotifyTarget) {
    inotify_notify_dentry(target.dentry(), IN_MODIFY);
    fanotify_notify_dentry(target.dentry(), FAN_MODIFY);
}

pub fn notify_attrib(target: &NotifyTarget) {
    let mask = IN_ATTRIB | if target.is_dir() { IN_ISDIR } else { 0 };
    inotify_notify_dentry(target.dentry(), mask);
    fanotify_notify_dentry(target.dentry(), FAN_ATTRIB);
}

pub fn notify_close(target: &NotifyTarget, writable: bool) {
    if writable {
        inotify_notify_dentry(target.dentry(), IN_CLOSE_WRITE);
        fanotify_notify_dentry(target.dentry(), FAN_CLOSE_WRITE);
    } else {
        inotify_notify_dentry(target.dentry(), IN_CLOSE_NOWRITE);
        fanotify_notify_dentry(target.dentry(), FAN_CLOSE_NOWRITE);
    }
}

pub fn notify_path_access(path: &str) {
    inotify_notify_path(path, IN_ACCESS);
    fanotify_notify_path(path, FAN_ACCESS);
}

pub fn notify_path_modify(path: &str) {
    inotify_notify_path(path, IN_MODIFY);
    fanotify_notify_path(path, FAN_MODIFY);
}
