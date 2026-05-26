#![allow(missing_docs)]
use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::Dentry;
use crate::fs::File;
use crate::fs::procfs::fd::ProcSelfFdDirDentry;
use crate::fs::tmpfs::inode::TempInode;
use crate::fs::vfs::{DentryInner, OpenFlags, inode::InodeMode};
use crate::task::current_process;
use alloc::format;
use alloc::sync::{Arc, Weak};

/// /proc/self 魔术目录：查找子项时动态生成当前进程相关的 proc 文件。
pub struct ProcSelfDirDentry {
    inner: DentryInner,
    self_weak: Weak<ProcSelfDirDentry>,
}

impl ProcSelfDirDentry {
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|me: &Weak<ProcSelfDirDentry>| Self {
            inner: DentryInner::new(name, parent_weak),
            self_weak: me.clone(),
        })
    }
}

impl Dentry for ProcSelfDirDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }

    fn find(&self, name: &str) -> SysResult<Arc<dyn Dentry>> {
        let me = self.self_weak.upgrade().unwrap();
        match name {
            "smaps" => {
                let dentry = crate::fs::procfs::smaps::SmapsDentry::new(
                    "smaps",
                    Some(me as Arc<dyn Dentry>),
                );
                let inode = Arc::new(crate::fs::procfs::smaps::SmapsInode::new());
                dentry.set_inode(inode);
                Ok(dentry)
            }
            "mounts" => {
                let dentry = crate::fs::procfs::mounts::MountsDentry::new(
                    "mounts",
                    Some(me as Arc<dyn Dentry>),
                );
                let inode = Arc::new(crate::fs::procfs::mounts::MountsInode::new());
                dentry.set_inode(inode);
                Ok(dentry)
            }
            "maps" => {
                let me = self.self_weak.upgrade().unwrap();
                let dentry =
                    crate::fs::procfs::maps::MapsDentry::new("maps", Some(me as Arc<dyn Dentry>));
                let inode = Arc::new(crate::fs::procfs::maps::MapsInode::new());
                dentry.set_inode(inode);
                Ok(dentry)
            }
            "pagemap" => {
                let me = self.self_weak.upgrade().unwrap();
                let dentry = crate::fs::procfs::pagemap::PagemapDentry::new(
                    "pagemap",
                    Some(me as Arc<dyn Dentry>),
                );
                let inode = Arc::new(crate::fs::procfs::pagemap::PagemapInode::new());
                dentry.set_inode(inode);
                Ok(dentry)
            }
            "status" => {
                let me = self.self_weak.upgrade().unwrap();
                let dentry = crate::fs::procfs::status::StatusDentry::new(
                    "status",
                    Some(me as Arc<dyn Dentry>),
                );
                let inode = Arc::new(crate::fs::procfs::status::StatusInode::new());
                dentry.set_inode(inode);
                Ok(dentry)
            }
            "fd" => {
                // 返回 /proc/self/fd 目录
                let me = self.self_weak.upgrade().unwrap();
                let fd_dir_dentry = ProcSelfFdDirDentry::new("fd", Some(me as Arc<dyn Dentry>));
                let fd_dir_inode = Arc::new(TempInode::new(InodeMode::DIR));
                fd_dir_dentry.set_inode(fd_dir_inode);
                Ok(fd_dir_dentry)
            }
            "fdinfo" => {
                let me = self.self_weak.upgrade().unwrap();
                let pid = current_process().getpid();
                let dentry = crate::fs::procfs::pid_dir::ProcFdinfoDirDentry::new(
                    "fdinfo",
                    Some(me as Arc<dyn Dentry>),
                    pid,
                );
                let inode = Arc::new(TempInode::new(InodeMode::DIR));
                dentry.set_inode(inode);
                Ok(dentry)
            }
            // "mounts" => {
            //     // 返回 /proc/self/mounts（与 /proc/mounts 相同）
            //     let me = self.self_weak.upgrade().unwrap();
            //     let dentry = crate::fs::procfs::mounts::MountsDentry::new(
            //         "mounts",
            //         Some(me as Arc<dyn Dentry>),
            //     );
            //     let inode = Arc::new(crate::fs::procfs::mounts::MountsInode::new());
            //     dentry.set_inode(inode);
            //     Ok(dentry)
            // }
            _ => Err(SysError::ENOENT),
        }
    }

    fn open(self: Arc<Self>, _flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Err(SysError::EISDIR)
    }
}
