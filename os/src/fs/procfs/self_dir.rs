#![allow(missing_docs)]
use alloc::string::{String, ToString};
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;

use crate::sync::{SpinNoIrqLock, SpinMutexGuard, SpinNoIrq};

use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::Dentry;
use crate::fs::File;
use crate::fs::Inode;
use crate::fs::procfs::smaps::{SmapsDentry, SmapsInode};
use crate::fs::vfs::{DentryInner, FileInner, OpenFlags};
use crate::fs::vfs::inode::InodeMode;
use crate::mm::UserBuffer;

/// /proc/self 魔术目录的 dentry。
/// 查找子项时动态生成当前进程相关的 proc 文件。
pub struct SelfDirDentry {
    inner: DentryInner,
    self_weak: Weak<SelfDirDentry>,
}

impl SelfDirDentry {
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|me: &Weak<SelfDirDentry>| Self {
            inner: DentryInner::new(name, parent_weak),
            self_weak: me.clone(),
        })
    }
}

impl Dentry for SelfDirDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }

    fn name(&self) -> &str {
        &self.inner.name
    }

    /// 查找子 dentry。若已缓存则直接返回，否则动态创建并缓存。
    fn find(&self, name: &str) -> SysResult<Arc<dyn Dentry>> {
        if let Some(child) = self.get_dentryinner().children.lock().get(name).cloned() {
            return Ok(child);
        }

        match name {
            "smaps" => {
                let self_arc = self.self_weak.upgrade().unwrap();
                let child = SmapsDentry::new("smaps", Some(self_arc as Arc<dyn Dentry>));
                let inode = Arc::new(SmapsInode::new());
                child.set_inode(inode);
                self.get_dentryinner()
                    .children
                    .lock()
                    .insert(name.to_string(), child.clone());
                Ok(child)
            }
            _ => Err(SysError::ENOENT),
        }
    }

    fn open(self: Arc<Self>, _flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Ok(Arc::new(ProcSelfDirFile::new(self)))
    }
}

/// /proc/self 目录对应的 File，支持 getdents64 读取目录项。
pub struct ProcSelfDirFile {
    inner: SpinNoIrqLock<FileInner>,
}

impl ProcSelfDirFile {
    /// 创建新的目录文件。
    pub fn new(dentry: Arc<dyn Dentry>) -> Self {
        Self {
            inner: SpinNoIrqLock::new(FileInner { offset: 0, dentry }),
        }
    }
}

impl File for ProcSelfDirFile {
    fn get_fileinner(&self) -> SpinMutexGuard<'_, FileInner, SpinNoIrq> {
        self.inner.lock()
    }

    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        false
    }

    fn read(&self, _buf: UserBuffer) -> SysResult<usize> {
        Err(SysError::EISDIR)
    }

    fn write(&self, _buf: UserBuffer) -> SysResult<usize> {
        Err(SysError::EISDIR)
    }

    fn open(&self) -> SyscallResult {
        Ok(0)
    }

    fn release(&self) -> SyscallResult {
        Ok(0)
    }

    fn ls(&self) -> Vec<(String, u64, u8)> {
        let inner = self.inner.lock();
        let mut entries = Vec::new();
        let children = inner.dentry.get_dentryinner().children.lock();
        for (name, child) in children.iter() {
            if let Some(inode) = child.get_inode() {
                let d_type = if inode.get_mode().contains(InodeMode::DIR) {
                    4 // DT_DIR
                } else {
                    8 // DT_REG
                };
                entries.push((name.clone(), inode.get_ino() as u64, d_type));
            }
        }
        entries
    }
}
