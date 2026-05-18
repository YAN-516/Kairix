use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::Dentry;
use crate::fs::File;
use crate::fs::vfs::FileInner;
use crate::fs::vfs::OpenFlags;
use crate::fs::vfs::inode::InodeMode;
use crate::fs::cgroup2::CGROUP_TABLE;
use crate::fs::tempfs::file::TempFile;
use crate::mm::UserBuffer;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;
use spin::mutex::MutexGuard;

/// /cgroup.procs 特殊文件：写入 PID 加入 cgroup，读取返回 PID 列表
pub struct CgroupProcsFile {
    inner: Mutex<FileInner>,
}

impl CgroupProcsFile {
    pub fn new(dentry: Arc<dyn Dentry>) -> Self {
        Self {
            inner: Mutex::new(FileInner { offset: 0, dentry }),
        }
    }
}

impl File for CgroupProcsFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        self.inner.lock()
    }
    fn readable(&self) -> bool {
        true
    }
    fn writable(&self) -> bool {
        true
    }
    fn read(&self, mut buf: UserBuffer) -> SysResult<usize> {
        let mut inner = self.get_fileinner();
        let path = inner.dentry.path();
        let dir_path = path.trim_end_matches("/cgroup.procs").to_string();
        let table = CGROUP_TABLE.lock();
        let pids = table.get(&dir_path).cloned().unwrap_or_default();
        let mut content = String::new();
        for pid in pids {
            content.push_str(&format!("{}\n", pid));
        }
        let data = content.as_bytes();
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
        Ok(total)
    }
    fn write(&self, buf: UserBuffer) -> SysResult<usize> {
        let inner = self.get_fileinner();
        let path = inner.dentry.path();
        let dir_path = path.trim_end_matches("/cgroup.procs").to_string();

        let mut bytes = Vec::new();
        for slice in buf.buffers.iter() {
            bytes.extend_from_slice(slice);
        }
        let s = core::str::from_utf8(&bytes).map_err(|_| SysError::EINVAL)?;
        let pid: usize = s.trim().parse().map_err(|_| SysError::EINVAL)?;

        let mut table = CGROUP_TABLE.lock();
        table.entry(dir_path).or_default().push(pid);
        Ok(bytes.len())
    }
    fn open(&self) -> SyscallResult {
        Ok(0)
    }
    fn release(&self) -> SyscallResult {
        Ok(0)
    }
}

/// /cgroup.controllers 特殊文件：返回空（我们没有真正的 controller）
pub struct CgroupControllersFile {
    inner: Mutex<FileInner>,
}

impl CgroupControllersFile {
    pub fn new(dentry: Arc<dyn Dentry>) -> Self {
        Self {
            inner: Mutex::new(FileInner { offset: 0, dentry }),
        }
    }
}

impl File for CgroupControllersFile {
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
        let data = b"";
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
        Ok(total)
    }
    fn write(&self, _buf: UserBuffer) -> SysResult<usize> {
        Err(SysError::EPERM)
    }
    fn open(&self) -> SyscallResult {
        Ok(0)
    }
    fn release(&self) -> SyscallResult {
        Ok(0)
    }
}

/// /cgroup.subtree_control 特殊文件：写入成功但无实际效果，读取返回空
pub struct CgroupSubtreeControlFile {
    inner: Mutex<FileInner>,
}

impl CgroupSubtreeControlFile {
    pub fn new(dentry: Arc<dyn Dentry>) -> Self {
        Self {
            inner: Mutex::new(FileInner { offset: 0, dentry }),
        }
    }
}

impl File for CgroupSubtreeControlFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        self.inner.lock()
    }
    fn readable(&self) -> bool {
        true
    }
    fn writable(&self) -> bool {
        true
    }
    fn read(&self, mut buf: UserBuffer) -> SysResult<usize> {
        let mut inner = self.get_fileinner();
        let data = b"";
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
        Ok(total)
    }
    fn write(&self, buf: UserBuffer) -> SysResult<usize> {
        let mut total = 0usize;
        for slice in buf.buffers.iter() {
            total += slice.len();
        }
        Ok(total)
    }
    fn open(&self) -> SyscallResult {
        Ok(0)
    }
    fn release(&self) -> SyscallResult {
        Ok(0)
    }
}
