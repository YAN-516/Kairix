#![allow(missing_docs)]
use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::Dentry;
use crate::fs::File;
use crate::fs::vfs::{DentryInner, OpenFlags, inode::InodeMode};
use crate::fs::tempfs::inode::TempInode;
use crate::fs::procfs::pid_stat::{PidStatDentry, PidStatInode};
use alloc::sync::{Arc, Weak};
use alloc::string::String;

/// /proc/[pid] 目录：查找子项时动态生成 proc 文件。
pub struct PidDirDentry {
    inner: DentryInner,
    self_weak: Weak<PidDirDentry>,
    pid: usize,
}

impl PidDirDentry {
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>, pid: usize) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|me: &Weak<PidDirDentry>| Self {
            inner: DentryInner::new(name, parent_weak),
            self_weak: me.clone(),
            pid,
        })
    }
}

impl Dentry for PidDirDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }

    fn find(&self, name: &str) -> SysResult<Arc<dyn Dentry>> {
        let me = self.self_weak.upgrade().unwrap();
        match name {
            "stat" => {
                let dentry = PidStatDentry::new(
                    "stat",
                    Some(me as Arc<dyn Dentry>),
                    self.pid,
                );
                let inode = Arc::new(PidStatInode::new());
                dentry.set_inode(inode);
                Ok(dentry)
            }
            _ => Err(SysError::ENOENT),
        }
    }

    fn open(self: Arc<Self>, _flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Err(SysError::EISDIR)
    }
}
