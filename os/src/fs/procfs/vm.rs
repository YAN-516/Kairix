#![allow(missing_docs)]

use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::vfs::inode::{InodeInner, InodeMode, inode_alloc};
use crate::fs::vfs::{DentryInner, FileInner, OpenFlags};
use crate::fs::{Dentry, File, Inode};
use crate::mm::UserBuffer;
use alloc::format;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::str;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::{Mutex, MutexGuard};

static VFS_CACHE_PRESSURE: AtomicUsize = AtomicUsize::new(100);

#[derive(Clone, Copy)]
pub enum VmSysctlKind {
    DropCaches,
    VfsCachePressure,
}

pub struct VmSysctlFile {
    inner: Mutex<FileInner>,
    kind: VmSysctlKind,
}

impl VmSysctlFile {
    pub fn new(dentry: Arc<dyn Dentry>, kind: VmSysctlKind) -> Self {
        Self {
            inner: Mutex::new(FileInner {
                offset: 0,
                dentry,
                flags: OpenFlags::empty(),
            }),
            kind,
        }
    }
}

impl File for VmSysctlFile {
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
        let value = match self.kind {
            VmSysctlKind::DropCaches => 0,
            VmSysctlKind::VfsCachePressure => VFS_CACHE_PRESSURE.load(Ordering::Relaxed),
        };
        let info = format!("{}\n", value);
        let data = info.as_bytes();
        let offset = inner.offset;
        if offset >= data.len() {
            return Ok(0);
        }

        let mut total = 0usize;
        for slice in buf.buffers.iter_mut() {
            let len = slice.len().min(data.len() - offset - total);
            if len == 0 {
                break;
            }
            slice[..len].copy_from_slice(&data[offset + total..offset + total + len]);
            total += len;
        }

        inner.offset = offset + total;
        if let Some(inode) = inner.dentry.get_inode() {
            inode.set_size(data.len());
        }
        Ok(total)
    }

    fn write(&self, buf: UserBuffer) -> SysResult<usize> {
        let len = buf.len();
        let value = parse_sysctl_value(&buf)?;
        match self.kind {
            VmSysctlKind::DropCaches => {
                let _ = value;
                crate::syscall::fanotify::fanotify_drop_evictable_marks();
            }
            VmSysctlKind::VfsCachePressure => {
                VFS_CACHE_PRESSURE.store(value, Ordering::Relaxed);
            }
        }
        if let Some(inode) = self.get_fileinner().dentry.get_inode() {
            inode.set_size(format!("{}\n", value).len());
        }
        Ok(len)
    }

    fn open(&self) -> SyscallResult {
        Ok(0)
    }

    fn release(&self) -> SyscallResult {
        Ok(0)
    }
}

fn parse_sysctl_value(buf: &UserBuffer) -> SysResult<usize> {
    let mut bytes = Vec::new();
    for slice in buf.buffers.iter() {
        bytes.extend_from_slice(slice);
    }
    let text = str::from_utf8(&bytes).map_err(|_| SysError::EINVAL)?.trim();
    if text.is_empty() {
        return Err(SysError::EINVAL);
    }

    let mut value = 0usize;
    for byte in text.bytes() {
        if !byte.is_ascii_digit() {
            return Err(SysError::EINVAL);
        }
        value = value
            .checked_mul(10)
            .and_then(|value| value.checked_add((byte - b'0') as usize))
            .ok_or(SysError::EINVAL)?;
    }
    Ok(value)
}

pub struct VmSysctlDentry {
    inner: DentryInner,
    kind: VmSysctlKind,
}

impl VmSysctlDentry {
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>, kind: VmSysctlKind) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|_me: &Weak<VmSysctlDentry>| Self {
            inner: DentryInner::new(name, parent_weak),
            kind,
        })
    }
}

impl Dentry for VmSysctlDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }

    fn name(&self) -> &str {
        &self.inner.name
    }

    fn open(self: Arc<Self>, _flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Ok(Arc::new(VmSysctlFile::new(self.clone(), self.kind)))
    }
}

pub struct VmSysctlInode {
    inner: InodeInner,
}

impl VmSysctlInode {
    pub fn new() -> Self {
        Self {
            inner: InodeInner::new(
                inode_alloc(),
                0,
                InodeMode::FILE
                    | InodeMode::OWNER_READ
                    | InodeMode::OWNER_WRITE
                    | InodeMode::GROUP_READ
                    | InodeMode::OTHER_READ,
                0,
            ),
        }
    }
}

impl Inode for VmSysctlInode {
    fn get_ino(&self) -> usize {
        self.inner.ino
    }

    fn get_size(&self) -> usize {
        self.inner.size.load(Ordering::Relaxed)
    }

    fn set_size(&self, new_size: usize) {
        self.inner.size.store(new_size, Ordering::Relaxed);
    }

    fn get_nlink(&self) -> usize {
        self.inner.nlink.load(Ordering::Relaxed)
    }

    fn get_mode(&self) -> InodeMode {
        self.inner.mode
    }

    fn inc_nlink(&self) {
        self.inner.nlink.fetch_add(1, Ordering::Relaxed);
    }

    fn dec_nlink(&self) {
        self.inner.nlink.fetch_sub(1, Ordering::Relaxed);
    }
}
