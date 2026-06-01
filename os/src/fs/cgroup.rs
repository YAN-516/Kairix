#![allow(missing_docs)]

use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::tmpfs::file::TempFile;
use crate::fs::tmpfs::inode::TempInode;
use crate::fs::vfs::dcache::GLOBAL_DCACHE;
use crate::fs::vfs::fstype::{FsType, FsTypeInner, MountFlags};
use crate::fs::vfs::inode::InodeMode;
use crate::fs::vfs::kstat::Statfs;
use crate::fs::vfs::superblock::{SuperBlock, SuperBlockInner};
use crate::fs::vfs::{Dentry, DentryInner, File, FileInner, OpenFlags};
use crate::mm::UserBuffer;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::{Mutex, MutexGuard};

use crate::devices::BlockDevice;

const CGROUP_DEFAULT_LIMIT: u64 = 1 << 30;

#[derive(Clone, Copy, PartialEq, Eq)]
enum CgroupControlKind {
    Tasks,
    CgroupProcs,
    MemoryLimitInBytes,
    MemoryMax,
    MemoryUseHierarchy,
}

impl CgroupControlKind {
    fn name(self) -> &'static str {
        match self {
            Self::Tasks => "tasks",
            Self::CgroupProcs => "cgroup.procs",
            Self::MemoryLimitInBytes => "memory.limit_in_bytes",
            Self::MemoryMax => "memory.max",
            Self::MemoryUseHierarchy => "memory.use_hierarchy",
        }
    }
}

fn is_builtin_control(name: &str) -> bool {
    matches!(
        name,
        "tasks"
            | "cgroup.procs"
            | "memory.limit_in_bytes"
            | "memory.max"
            | "memory.use_hierarchy"
    )
}

struct CgroupDirState {
    last_pid: Mutex<String>,
    memory_limit: AtomicU64,
    use_hierarchy: AtomicUsize,
}

impl CgroupDirState {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            last_pid: Mutex::new(String::new()),
            memory_limit: AtomicU64::new(CGROUP_DEFAULT_LIMIT),
            use_hierarchy: AtomicUsize::new(1),
        })
    }

    fn read_text(&self, kind: CgroupControlKind) -> String {
        match kind {
            CgroupControlKind::Tasks | CgroupControlKind::CgroupProcs => {
                let pid = self.last_pid.lock().clone();
                if pid.is_empty() {
                    String::new()
                } else {
                    format!("{}\n", pid)
                }
            }
            CgroupControlKind::MemoryLimitInBytes | CgroupControlKind::MemoryMax => {
                let limit = self.memory_limit.load(Ordering::Relaxed);
                if limit == u64::MAX {
                    "max\n".to_string()
                } else {
                    format!("{}\n", limit)
                }
            }
            CgroupControlKind::MemoryUseHierarchy => {
                format!("{}\n", self.use_hierarchy.load(Ordering::Relaxed))
            }
        }
    }

    fn write_text(&self, kind: CgroupControlKind, text: &str) -> SysResult<usize> {
        let trimmed = text.trim_matches(|c| c == '\0' || c == '\n' || c == '\r' || c == ' ' || c == '\t');
        match kind {
            CgroupControlKind::Tasks | CgroupControlKind::CgroupProcs => {
                if trimmed.is_empty() {
                    return Err(SysError::EINVAL);
                }
                *self.last_pid.lock() = trimmed.to_string();
                Ok(text.len())
            }
            CgroupControlKind::MemoryLimitInBytes | CgroupControlKind::MemoryMax => {
                let value = if trimmed == "max" {
                    u64::MAX
                } else {
                    trimmed.parse::<u64>().map_err(|_| SysError::EINVAL)?
                };
                self.memory_limit.store(value, Ordering::Relaxed);
                Ok(text.len())
            }
            CgroupControlKind::MemoryUseHierarchy => {
                let value = trimmed.parse::<usize>().map_err(|_| SysError::EINVAL)?;
                self.use_hierarchy.store(usize::from(value != 0), Ordering::Relaxed);
                Ok(text.len())
            }
        }
    }
}

fn cgroup_dir_inode() -> Arc<TempInode> {
    Arc::new(TempInode::new(
        InodeMode::DIR
            | InodeMode::OWNER_READ
            | InodeMode::OWNER_WRITE
            | InodeMode::OWNER_EXEC
            | InodeMode::GROUP_READ
            | InodeMode::GROUP_EXEC
            | InodeMode::OTHER_READ
            | InodeMode::OTHER_EXEC,
    ))
}

fn cgroup_file_inode() -> Arc<TempInode> {
    Arc::new(TempInode::new(
        InodeMode::FILE
            | InodeMode::OWNER_READ
            | InodeMode::OWNER_WRITE
            | InodeMode::GROUP_READ
            | InodeMode::OTHER_READ,
    ))
}

fn add_control_file(
    parent: Arc<dyn Dentry>,
    state: Arc<CgroupDirState>,
    kind: CgroupControlKind,
) {
    let dentry = CgroupControlDentry::new(kind.name(), Some(parent.clone()), state, kind);
    dentry.set_inode(cgroup_file_inode());
    parent.add_child(dentry.clone());
    GLOBAL_DCACHE.insert(dentry.path(), dentry);
}

fn populate_control_files(dir: Arc<dyn Dentry>, state: Arc<CgroupDirState>) {
    add_control_file(dir.clone(), state.clone(), CgroupControlKind::Tasks);
    add_control_file(dir.clone(), state.clone(), CgroupControlKind::CgroupProcs);
    add_control_file(
        dir.clone(),
        state.clone(),
        CgroupControlKind::MemoryLimitInBytes,
    );
    add_control_file(dir.clone(), state.clone(), CgroupControlKind::MemoryMax);
    add_control_file(dir, state, CgroupControlKind::MemoryUseHierarchy);
}

pub struct CgroupFsType {
    inner: FsTypeInner,
}

impl CgroupFsType {
    pub fn new(name: &str) -> Arc<Self> {
        Arc::new(Self {
            inner: FsTypeInner::new(name),
        })
    }
}

impl FsType for CgroupFsType {
    fn inner(&self) -> &FsTypeInner {
        &self.inner
    }

    fn mount(
        &self,
        _name: &str,
        _parent: Option<Arc<dyn Dentry>>,
        _flags: MountFlags,
        _dev: Option<Arc<dyn BlockDevice>>,
    ) -> Option<Arc<dyn Dentry>> {
        None
    }

    fn mount_with_data(
        &self,
        name: &str,
        parent: Option<Arc<dyn Dentry>>,
        flags: MountFlags,
        dev: Option<Arc<dyn BlockDevice>>,
        data: Option<&str>,
    ) -> Option<Arc<dyn Dentry>> {
        let data = data?;
        if !data.split(',').any(|part| part == "memory") {
            return None;
        }

        let state = CgroupDirState::new();
        let root = CgroupDirDentry::new(name, parent, state.clone());
        root.set_inode(cgroup_dir_inode());
        populate_control_files(root.clone(), state);
        let superblock = Arc::new(CgroupSuperBlock::new(SuperBlockInner::new(
            dev,
            Some(root.clone()),
            flags,
        )));
        GLOBAL_DCACHE.insert(root.path(), root.clone());
        GLOBAL_DCACHE.pin(root.path());
        self.add_sb(&root.path(), superblock);
        Some(root)
    }

    fn kill_sb(&self) -> isize {
        todo!()
    }
}

struct CgroupSuperBlock {
    inner: SuperBlockInner,
}

unsafe impl Sync for CgroupSuperBlock {}
unsafe impl Send for CgroupSuperBlock {}

impl CgroupSuperBlock {
    fn new(inner: SuperBlockInner) -> Self {
        Self { inner }
    }
}

impl SuperBlock for CgroupSuperBlock {
    fn inner(&self) -> &SuperBlockInner {
        &self.inner
    }

    fn statfs(&self) -> Statfs {
        let mut stat = Statfs::new();
        stat.f_type = 0x0027_e0eb;
        stat.f_bsize = 4096;
        stat.f_blocks = 1;
        stat.f_bfree = 0;
        stat.f_bavail = 0;
        stat.f_files = 1024;
        stat.f_ffree = 1000;
        stat.f_frsize = 4096;
        stat
    }
}

struct CgroupDirDentry {
    inner: DentryInner,
    self_weak: Weak<CgroupDirDentry>,
}

impl CgroupDirDentry {
    fn new(name: &str, parent: Option<Arc<dyn Dentry>>, _state: Arc<CgroupDirState>) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|me: &Weak<CgroupDirDentry>| Self {
            inner: DentryInner::new(name, parent_weak),
            self_weak: me.clone(),
        })
    }
}

impl Dentry for CgroupDirDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }

    fn find(&self, name: &str) -> SysResult<Arc<dyn Dentry>> {
        let children = self.inner.children.lock();
        if let Some(child) = children.get(name).cloned() {
            return Ok(child);
        }
        drop(children);
        if let Some(bdentry) = self.inner.bdentry.lock().clone() {
            if let Ok(child) = bdentry.find(name) {
                return Ok(child);
            }
        }
        Err(SysError::ENOENT)
    }

    fn create(&self, name: &str, mode: InodeMode) -> SysResult<Arc<dyn Dentry>> {
        if name.is_empty() || is_builtin_control(name) {
            return Err(SysError::EINVAL);
        }
        if mode.get_type() != InodeMode::DIR {
            return Err(SysError::EPERM);
        }
        let mut children = self.inner.children.lock();
        if children.contains_key(name) {
            return Err(SysError::EEXIST);
        }
        let me = self.self_weak.upgrade().unwrap();
        let state = CgroupDirState::new();
        let new_dir = CgroupDirDentry::new(name, Some(me as Arc<dyn Dentry>), state.clone());
        new_dir.set_inode(cgroup_dir_inode());
        populate_control_files(new_dir.clone(), state);
        children.insert(name.to_string(), new_dir.clone());
        GLOBAL_DCACHE.insert(new_dir.path(), new_dir.clone());
        Ok(new_dir)
    }

    fn unlink(&self, name: &str, flags: u32) -> SyscallResult {
        let is_rmdir = flags & crate::fs::tmpfs::dentry::AT_REMOVEDIR != 0;
        let mut children = self.inner.children.lock();
        let child = children.get(name).cloned().ok_or(SysError::ENOENT)?;
        let inode = child.get_inode().ok_or(SysError::ENOENT)?;
        let is_dir = inode.get_mode().get_type() == InodeMode::DIR;

        if is_builtin_control(name) {
            return Err(SysError::EPERM);
        }
        if is_rmdir && !is_dir {
            return Err(SysError::ENOTDIR);
        }
        if !is_rmdir && is_dir {
            return Err(SysError::EISDIR);
        }
        if is_dir {
            let non_builtin_children = child
                .children()
                .into_iter()
                .filter(|(child_name, child_dentry)| {
                    !is_builtin_control(child_name)
                        || child_dentry
                            .get_inode()
                            .is_some_and(|inode| inode.get_mode().get_type() == InodeMode::DIR)
                })
                .count();
            if non_builtin_children != 0 {
                return Err(SysError::ENOTEMPTY);
            }
        }
        children.remove(name);
        inode.dec_nlink();
        GLOBAL_DCACHE.remove_subtree(&child.path());
        Ok(0)
    }

    fn open(self: Arc<Self>, flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        let (readable, writable) = flags.read_write();
        Ok(Arc::new(TempFile::new(
            readable,
            writable,
            flags.contains(OpenFlags::O_APPEND),
            self,
            flags,
        )))
    }
}

struct CgroupControlDentry {
    inner: DentryInner,
    state: Arc<CgroupDirState>,
    kind: CgroupControlKind,
}

impl CgroupControlDentry {
    fn new(
        name: &str,
        parent: Option<Arc<dyn Dentry>>,
        state: Arc<CgroupDirState>,
        kind: CgroupControlKind,
    ) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new(Self {
            inner: DentryInner::new(name, parent_weak),
            state,
            kind,
        })
    }
}

impl Dentry for CgroupControlDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }

    fn open(self: Arc<Self>, flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Ok(Arc::new(CgroupControlFile::new(
            self.clone() as Arc<dyn Dentry>,
            self.state.clone(),
            self.kind,
            flags,
        )))
    }
}

struct CgroupControlFile {
    inner: Mutex<FileInner>,
    state: Arc<CgroupDirState>,
    kind: CgroupControlKind,
}

impl CgroupControlFile {
    fn new(
        dentry: Arc<dyn Dentry>,
        state: Arc<CgroupDirState>,
        kind: CgroupControlKind,
        flags: OpenFlags,
    ) -> Self {
        Self {
            inner: Mutex::new(FileInner {
                offset: 0,
                dentry,
                flags,
            }),
            state,
            kind,
        }
    }
}

impl File for CgroupControlFile {
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
        let info = self.state.read_text(self.kind);
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

    fn write(&self, buf: UserBuffer) -> SysResult<usize> {
        let mut bytes = Vec::new();
        for slice in buf.buffers.iter() {
            bytes.extend_from_slice(slice);
        }
        let text = core::str::from_utf8(&bytes).map_err(|_| SysError::EINVAL)?;
        let written = self.state.write_text(self.kind, text)?;
        if let Some(inode) = self.get_fileinner().dentry.get_inode() {
            inode.set_size(self.state.read_text(self.kind).len());
        }
        Ok(written)
    }

    fn open(&self) -> SyscallResult {
        Ok(0)
    }

    fn release(&self) -> SyscallResult {
        Ok(0)
    }
}
