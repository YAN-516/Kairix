#![allow(missing_docs)]
use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::Dentry;
use crate::fs::File;
use crate::fs::vfs::{DentryInner, OpenFlags, inode::InodeMode};
use crate::task::current_process;
use alloc::sync::{Arc, Weak};

/// /proc/self/fd 目录：动态显示当前进程打开的文件描述符
pub struct ProcSelfFdDirDentry {
    inner: DentryInner,
    _self_weak: Weak<ProcSelfFdDirDentry>,
}

impl ProcSelfFdDirDentry {
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|me: &Weak<ProcSelfFdDirDentry>| Self {
            inner: DentryInner::new(name, parent_weak),
            _self_weak: me.clone(),
        })
    }
}

impl Dentry for ProcSelfFdDirDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }

    fn find(&self, name: &str) -> SysResult<Arc<dyn Dentry>> {
        // 尝试将 name 解析为数字 fd
        match name.parse::<usize>() {
            Ok(fd) => {
                // 获取当前进程的文件描述符表
                let process = current_process();
                let inner = process.inner_exclusive_access();

                // 检查 fd 是否有效
                if fd >= inner.fd_table.len() {
                    return Err(SysError::ENOENT);
                }

                let file = match &inner.fd_table[fd] {
                    Some(f) => f.clone(),
                    None => return Err(SysError::ENOENT),
                };
                drop(inner);

                // 直接返回原始文件的 dentry，而不是创建符号链接
                // 这样打开时会直接操作原始文件，避免路径解析问题
                Ok(file.get_dentry())
            }
            Err(_) => Err(SysError::ENOENT),
        }
    }

    fn open(self: Arc<Self>, _flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Err(SysError::EISDIR)
    }
}
