#![allow(missing_docs)]
use crate::error::{SysError, SysResult};
use crate::fs::Dentry;
use crate::fs::File;
use crate::fs::procfs::fd::ProcSelfFdDirDentry;
use crate::fs::procfs::pid_dir::{DT_DIR, DT_LNK, DT_REG, ProcDirFile, child_entries};
use crate::fs::tmpfs::inode::TempInode;
use crate::fs::vfs::{DentryInner, OpenFlags, inode::InodeMode};
use crate::task::current_process;
use alloc::string::{String, ToString};
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;

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
            "mountinfo" => {
                let dentry = crate::fs::procfs::mounts::MountsDentry::new_mountinfo(
                    "mountinfo",
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
            "exe" => {
                let dentry =
                    crate::fs::tmpfs::dentry::TempDentry::new("exe", Some(me as Arc<dyn Dentry>));
                let inode = Arc::new(TempInode::new_symlink("/proc/version"));
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

    fn ls(&self) -> Vec<(String, u64, u8)> {
        let mut entries = child_entries(self);
        let base = current_process().getpid() as u64 * 32;
        for (name, ino, d_type) in [
            ("smaps", base + 1, DT_REG),
            ("mounts", base + 2, DT_REG),
            ("mountinfo", base + 3, DT_REG),
            ("maps", base + 4, DT_REG),
            ("pagemap", base + 5, DT_REG),
            ("status", base + 6, DT_REG),
            ("fd", base + 7, DT_DIR),
            ("fdinfo", base + 8, DT_DIR),
            ("exe", base + 9, DT_LNK),
        ] {
            if entries.iter().any(|(entry_name, _, _)| entry_name == name) {
                continue;
            }
            entries.push((name.to_string(), ino, d_type));
        }
        entries
    }

    fn open(self: Arc<Self>, flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Ok(Arc::new(ProcDirFile::new(self, flags)))
    }
}
