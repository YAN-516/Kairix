#![allow(missing_docs)]
use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::Dentry;
use crate::fs::File;
use crate::fs::Inode;
use crate::fs::vfs::DentryInner;
use crate::fs::vfs::FileInner;
use crate::fs::vfs::OpenFlags;
use crate::fs::vfs::inode::InodeInner;
use crate::fs::vfs::inode::InodeMode;
use crate::fs::vfs::inode::inode_alloc;
use crate::mm::UserBuffer;
use crate::task::pid2process;
use alloc::format;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use spin::Mutex;
use spin::MutexGuard;

/// /proc/[pid]/stat 文件内容生成
pub struct PidStatFile {
    inner: Mutex<FileInner>,
    pid: usize,
}

impl PidStatFile {
    pub fn new(dentry: Arc<dyn Dentry>, pid: usize) -> Self {
        Self {
            inner: Mutex::new(FileInner {
                offset: 0,
                dentry,
                flags: OpenFlags::empty(),
            }),
            pid,
        }
    }

    fn generate_content(&self) -> String {
        let pid = self.pid;
        let process_opt = pid2process(pid);
        if let Some(process) = process_opt {
            let inner = process.inner_exclusive_access();
            let ppid = inner
                .parent
                .as_ref()
                .and_then(|p| p.upgrade())
                .map_or(0, |p| p.getpid());
            let pgid = inner.pgid.0;
            let comm = "(init)";
            let state = match inner.term_status {
                crate::task::process::TermStatus::Running => 'R',
                crate::task::process::TermStatus::Exited(_) => 'Z',
                crate::task::process::TermStatus::Signaled(_, _) => 'Z',
                crate::task::process::TermStatus::Stopped(_) => 'T',
            };
            // 简化格式，只提供前 5 个必要字段，后面用 0 填充
            // pid (comm) state ppid pgrp ...
            format!(
                "{} {} {} {} {} 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n",
                pid, comm, state, ppid, pgid
            )
        } else {
            // 进程不存在，返回空内容
            String::new()
        }
    }
}

impl File for PidStatFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        self.inner.lock()
    }

    fn readable(&self) -> bool {
        true
    }
    fn writable(&self) -> bool {
        false
    }

    fn read(&self, mut buf: UserBuffer) -> SysResult<usize> {
        let mut inner = self.get_fileinner();
        let info = self.generate_content();
        let data = info.as_bytes();
        let offset = inner.offset;
        if offset >= data.len() {
            return Ok(0);
        }
        let remaining = &data[offset..];
        let mut total = 0usize;
        for slice in buf.buffers.iter_mut() {
            let len = slice.len().min(remaining.len() - total);
            if len == 0 {
                break;
            }
            slice[..len].copy_from_slice(&remaining[total..total + len]);
            total += len;
        }
        inner.offset = offset + total;
        if let Some(inode) = inner.dentry.get_inode() {
            inode.set_size(data.len());
        }
        Ok(total)
    }

    fn write(&self, _buf: UserBuffer) -> SysResult<usize> {
        Ok(0)
    }

    fn open(&self) -> SyscallResult {
        Ok(0)
    }
    fn release(&self) -> SyscallResult {
        Ok(0)
    }
}

pub struct PidStatDentry {
    inner: DentryInner,
    pid: usize,
}

impl PidStatDentry {
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>, pid: usize) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|_me: &Weak<PidStatDentry>| Self {
            inner: DentryInner::new(name, parent_weak),
            pid,
        })
    }
}

impl Dentry for PidStatDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }
    fn name(&self) -> &str {
        &self.inner.name
    }
    fn open(self: Arc<Self>, _flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        let pid = self.pid;
        Ok(Arc::new(PidStatFile::new(self, pid)))
    }
}

pub struct PidStatInode {
    inner: InodeInner,
}

impl PidStatInode {
    pub fn new() -> Self {
        Self {
            inner: InodeInner::new(inode_alloc(), 0, InodeMode::FILE, 0),
        }
    }
}

impl Inode for PidStatInode {
    fn get_mode(&self) -> InodeMode {
        self.inner.mode
    }
    fn set_size(&self, new_size: usize) {
        self.inner
            .size
            .store(new_size, core::sync::atomic::Ordering::SeqCst);
    }
    fn get_size(&self) -> usize {
        self.inner.size.load(core::sync::atomic::Ordering::SeqCst)
    }
    fn get_ino(&self) -> usize {
        self.inner.ino
    }
    fn get_nlink(&self) -> usize {
        self.inner.nlink.load(core::sync::atomic::Ordering::SeqCst)
    }
    fn inc_nlink(&self) {
        self.inner
            .nlink
            .fetch_add(1, core::sync::atomic::Ordering::SeqCst);
    }
    fn dec_nlink(&self) {
        self.inner
            .nlink
            .fetch_sub(1, core::sync::atomic::Ordering::SeqCst);
    }
    fn get_atime(&self) -> (i64, i64) {
        (0, 0)
    }
    fn set_atime(&self, _sec: i64, _nsec: i64) {}
    fn get_mtime(&self) -> (i64, i64) {
        (0, 0)
    }
    fn set_mtime(&self, _sec: i64, _nsec: i64) {}
    fn get_ctime(&self) -> (i64, i64) {
        (0, 0)
    }
    fn set_ctime(&self, _sec: i64, _nsec: i64) {}
}
