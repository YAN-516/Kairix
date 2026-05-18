use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::sync::{Arc, Weak};
use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::vfs::Inode;
use crate::fs::cgroup2::inode::Cgroup2Inode;
use crate::fs::cgroup2::file::{CgroupProcsFile, CgroupControllersFile, CgroupSubtreeControlFile};
use crate::fs::vfs::OpenFlags;
use crate::fs::File;
use crate::fs::vfs::{
    dcache::GLOBAL_DCACHE,
    inode::InodeMode,
    Dentry,
    DentryInner
};
use crate::fs::tempfs::file::TempFile;

pub struct Cgroup2Dentry {
    inner: DentryInner,
    self_weak: Weak<Cgroup2Dentry>,
}

impl Cgroup2Dentry {
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>) -> Arc<dyn Dentry> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|me: &Weak<Cgroup2Dentry>| {
            Self {
                inner: DentryInner::new(name, parent_weak.clone()),
                self_weak: me.clone(),
            }
        })
    }
}

impl Dentry for Cgroup2Dentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }
    fn name(&self) -> &str {
        &self.inner.name
    }
    fn parent(&self) -> Option<Arc<dyn Dentry>> {
        self.inner.parent.as_ref().and_then(|p| p.upgrade())
    }
    fn path(&self) -> String {
        let Some(parent) = self.parent() else {
            return String::from("/");
        };
        let parent_path = parent.path();
        if parent_path == "/" {
            parent_path + self.name()
        } else {
            parent_path + "/" + self.name()
        }
    }
    fn find(&self, name: &str) -> SysResult<Arc<dyn Dentry>> {
        let children = self.inner.children.lock();
        children.get(name).cloned().ok_or(SysError::ENOENT)
    }
    fn create(&self, name: &str, mode: InodeMode) -> SysResult<Arc<dyn Dentry>> {
        let mut children = self.inner.children.lock();
        log::info!("[DEBUG Cgroup2Dentry::create] self.path()={}, name={}, children={:?}", self.path(), name, children.keys().collect::<Vec<_>>());
        if children.contains_key(name) {
            log::info!("[DEBUG Cgroup2Dentry::create] EEXIST");
            return Err(SysError::EEXIST);
        }
        let me = self.self_weak.upgrade().unwrap();
        let new_dentry = Cgroup2Dentry::new(name, Some(me as Arc<dyn Dentry>));
        let child_inode = Arc::new(Cgroup2Inode::new(mode));
        new_dentry.set_inode(child_inode);
        children.insert(name.to_string(), new_dentry.clone());
        let target_path = format!("{}/{}", self.path().trim_end_matches('/'), name);
        GLOBAL_DCACHE.insert(target_path.clone(), new_dentry.clone());

        // 如果创建的是目录，自动创建 cgroup 文件
        if mode.contains(InodeMode::DIR) {
            let procs = Cgroup2Dentry::new("cgroup.procs", Some(new_dentry.clone()));
            procs.set_inode(Arc::new(Cgroup2Inode::new(InodeMode::FILE)));
            new_dentry.get_dentryinner().children.lock().insert("cgroup.procs".to_string(), procs.clone());
            GLOBAL_DCACHE.insert(format!("{}/cgroup.procs", target_path), procs);

            let ctrls = Cgroup2Dentry::new("cgroup.controllers", Some(new_dentry.clone()));
            ctrls.set_inode(Arc::new(Cgroup2Inode::new(InodeMode::FILE)));
            new_dentry.get_dentryinner().children.lock().insert("cgroup.controllers".to_string(), ctrls.clone());
            GLOBAL_DCACHE.insert(format!("{}/cgroup.controllers", target_path), ctrls);

            let subtree = Cgroup2Dentry::new("cgroup.subtree_control", Some(new_dentry.clone()));
            subtree.set_inode(Arc::new(Cgroup2Inode::new(InodeMode::FILE)));
            new_dentry.get_dentryinner().children.lock().insert("cgroup.subtree_control".to_string(), subtree.clone());
            GLOBAL_DCACHE.insert(format!("{}/cgroup.subtree_control", target_path), subtree);
        }

        Ok(new_dentry)
    }
    fn unlink(&self, name: &str, flags: u32) -> SyscallResult {
        let is_rmdir = flags & crate::fs::tempfs::dentry::AT_REMOVEDIR != 0;
        let mut children = self.inner.children.lock();
        let child = match children.get(name) {
            Some(c) => c.clone(),
            None => return Err(SysError::ENOENT),
        };
        let inode = match child.get_inode() {
            Some(i) => i,
            None => return Err(SysError::ENOENT),
        };
        let is_dir = inode.get_mode().contains(InodeMode::DIR);
        if is_rmdir && !is_dir {
            return Err(SysError::ENOTDIR);
        }
        if !is_rmdir && is_dir {
            return Err(SysError::EISDIR);
        }
        if is_dir {
            let child_children = child.get_dentryinner().children.lock();
            if !child_children.is_empty() {
                return Err(SysError::ENOTEMPTY);
            }
        }
        children.remove(name);
        inode.dec_nlink();
        let target_path = format!("{}/{}", self.path().trim_end_matches('/'), name);
        GLOBAL_DCACHE.remove(&target_path);
        Ok(0)
    }
    fn link(&self, new_name: &str, old_dentry: Arc<dyn Dentry>) -> SyscallResult {
        let mut children = self.inner.children.lock();
        if children.contains_key(new_name) {
            return Err(SysError::EEXIST);
        }
        let old_inode = match old_dentry.get_inode() {
            Some(i) => i,
            None => return Err(SysError::ENOENT),
        };
        if !old_inode.get_mode().contains(InodeMode::FILE) {
            return Err(SysError::EINVAL);
        }
        let me = self.self_weak.upgrade().unwrap();
        let new_dentry = Cgroup2Dentry::new(new_name, Some(me as Arc<dyn Dentry>));
        new_dentry.set_inode(old_inode.clone());
        old_inode.inc_nlink();
        children.insert(new_name.to_string(), new_dentry.clone());
        let new_path = format!("{}/{}", self.path().trim_end_matches('/'), new_name);
        GLOBAL_DCACHE.insert(new_path, new_dentry);
        Ok(0)
    }
    fn open(self: Arc<Self>, _flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        match self.name() {
            "cgroup.procs" => Ok(Arc::new(CgroupProcsFile::new(self))),
            "cgroup.controllers" => Ok(Arc::new(CgroupControllersFile::new(self))),
            "cgroup.subtree_control" => Ok(Arc::new(CgroupSubtreeControlFile::new(self))),
            _ => Ok(Arc::new(TempFile::new(self))),
        }
    }
}
