#![allow(missing_docs)]

use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::notify::fanotify::fanotify_fdinfo;
use crate::fs::notify::inotify::inotify_fdinfo;
use crate::fs::tmpfs::inode::TempInode;
use crate::fs::vfs::inode::InodeMode;
use crate::fs::vfs::{Dentry, DentryInner, File, FileInner, OpenFlags};
use crate::mm::UserBuffer;
use crate::task::{all_processes, pid2process};
use alloc::string::{String, ToString};
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use alloc::{format, vec};
use spin::{Mutex, MutexGuard};

pub(crate) const DT_DIR: u8 = 4;
pub(crate) const DT_REG: u8 = 8;
pub(crate) const DT_LNK: u8 = 10;

fn parse_pid(name: &str) -> Option<usize> {
    if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    name.parse::<usize>().ok()
}

pub(crate) fn d_type_from_mode(mode: InodeMode) -> u8 {
    match mode.get_type() {
        InodeMode::DIR => DT_DIR,
        InodeMode::FILE => DT_REG,
        InodeMode::LINK => DT_LNK,
        _ => 0,
    }
}

pub(crate) fn child_entries(dentry: &dyn Dentry) -> Vec<(String, u64, u8)> {
    dentry
        .children()
        .into_iter()
        .filter_map(|(name, child)| {
            let inode = child.get_inode()?;
            Some((
                name,
                inode.get_ino() as u64,
                d_type_from_mode(inode.get_mode()),
            ))
        })
        .collect()
}

fn dir_inode() -> Arc<TempInode> {
    Arc::new(TempInode::new(InodeMode::DIR))
}

fn file_inode() -> Arc<TempInode> {
    Arc::new(TempInode::new(
        InodeMode::FILE | InodeMode::OWNER_READ | InodeMode::GROUP_READ | InodeMode::OTHER_READ,
    ))
}

pub(crate) struct ProcDirFile {
    inner: Mutex<FileInner>,
}

impl ProcDirFile {
    pub(crate) fn new(dentry: Arc<dyn Dentry>, flags: OpenFlags) -> Self {
        Self {
            inner: Mutex::new(FileInner {
                offset: 0,
                dentry,
                flags,
            }),
        }
    }
}

impl File for ProcDirFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
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

    fn ls(&self) -> Vec<(String, u64, u8)> {
        self.inner.lock().dentry.ls()
    }
}

pub struct ProcRootDentry {
    inner: DentryInner,
    self_weak: Weak<ProcRootDentry>,
}

impl ProcRootDentry {
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|me: &Weak<ProcRootDentry>| Self {
            inner: DentryInner::new(name, parent_weak),
            self_weak: me.clone(),
        })
    }
}

impl Dentry for ProcRootDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }

    fn find(&self, name: &str) -> SysResult<Arc<dyn Dentry>> {
        if let Some(child) = self.inner.children.lock().get(name).cloned() {
            return Ok(child);
        }

        let pid = parse_pid(name).ok_or(SysError::ENOENT)?;
        if pid2process(pid).is_none() {
            return Err(SysError::ENOENT);
        }
        let me = self.self_weak.upgrade().unwrap();
        let dentry = ProcPidDentry::new(name, Some(me as Arc<dyn Dentry>), pid);
        dentry.set_inode(dir_inode());
        Ok(dentry)
    }

    fn ls(&self) -> Vec<(String, u64, u8)> {
        let mut entries = child_entries(self);
        let mut pids: Vec<usize> = all_processes()
            .into_iter()
            .map(|process| process.getpid())
            .collect();
        pids.sort_unstable();

        for pid in pids {
            let name = pid.to_string();
            if entries.iter().any(|(entry_name, _, _)| entry_name == &name) {
                continue;
            }
            entries.push((name, pid as u64, DT_DIR));
        }
        entries
    }

    fn open(self: Arc<Self>, flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Ok(Arc::new(ProcDirFile::new(self, flags)))
    }
}

struct ProcPidDentry {
    inner: DentryInner,
    self_weak: Weak<ProcPidDentry>,
    pid: usize,
}

impl ProcPidDentry {
    fn new(name: &str, parent: Option<Arc<dyn Dentry>>, pid: usize) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|me: &Weak<ProcPidDentry>| Self {
            inner: DentryInner::new(name, parent_weak),
            self_weak: me.clone(),
            pid,
        })
    }
}

impl Dentry for ProcPidDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }

    fn find(&self, name: &str) -> SysResult<Arc<dyn Dentry>> {
        if pid2process(self.pid).is_none() {
            return Err(SysError::ENOENT);
        }
        let me = self.self_weak.upgrade().unwrap();
        match name {
            "fdinfo" => {
                let dentry = ProcFdinfoDirDentry::new(name, Some(me as Arc<dyn Dentry>), self.pid);
                dentry.set_inode(dir_inode());
                Ok(dentry)
            }
            "stat" => {
                let dentry = crate::fs::procfs::pid_stat::PidStatDentry::new(
                    name,
                    Some(me as Arc<dyn Dentry>),
                    self.pid,
                );
                let inode = Arc::new(crate::fs::procfs::pid_stat::PidStatInode::new());
                dentry.set_inode(inode);
                Ok(dentry)
            }
            _ => Err(SysError::ENOENT),
        }
    }

    fn ls(&self) -> Vec<(String, u64, u8)> {
        if pid2process(self.pid).is_none() {
            return Vec::new();
        }
        let base = self.pid as u64 * 16;
        vec![
            ("fdinfo".to_string(), base + 1, DT_DIR),
            ("stat".to_string(), base + 2, DT_REG),
        ]
    }

    fn open(self: Arc<Self>, flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Ok(Arc::new(ProcDirFile::new(self, flags)))
    }
}

pub struct ProcFdinfoDirDentry {
    inner: DentryInner,
    self_weak: Weak<ProcFdinfoDirDentry>,
    pid: usize,
}

impl ProcFdinfoDirDentry {
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>, pid: usize) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|me: &Weak<ProcFdinfoDirDentry>| Self {
            inner: DentryInner::new(name, parent_weak),
            self_weak: me.clone(),
            pid,
        })
    }
}

impl Dentry for ProcFdinfoDirDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }

    fn find(&self, name: &str) -> SysResult<Arc<dyn Dentry>> {
        let fd = parse_pid(name).ok_or(SysError::ENOENT)?;
        let process = pid2process(self.pid).ok_or(SysError::ENOENT)?;
        let inner = process.inner_exclusive_access();
        let exists = fd < inner.fd_table.len() && inner.fd_table[fd].is_some();
        drop(inner);
        if !exists {
            return Err(SysError::ENOENT);
        }

        let me = self.self_weak.upgrade().unwrap();
        let dentry = ProcFdinfoDentry::new(name, Some(me as Arc<dyn Dentry>), self.pid, fd);
        dentry.set_inode(file_inode());
        Ok(dentry)
    }

    fn ls(&self) -> Vec<(String, u64, u8)> {
        let process = match pid2process(self.pid) {
            Some(process) => process,
            None => return Vec::new(),
        };
        let inner = process.inner_exclusive_access();
        inner
            .fd_table
            .iter()
            .enumerate()
            .filter_map(|(fd, file)| {
                if file.is_some() {
                    Some((fd.to_string(), fd as u64, DT_REG))
                } else {
                    None
                }
            })
            .collect()
    }

    fn open(self: Arc<Self>, flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Ok(Arc::new(ProcDirFile::new(self, flags)))
    }
}

struct ProcFdinfoDentry {
    inner: DentryInner,
    pid: usize,
    fd: usize,
}

impl ProcFdinfoDentry {
    fn new(name: &str, parent: Option<Arc<dyn Dentry>>, pid: usize, fd: usize) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new(Self {
            inner: DentryInner::new(name, parent_weak),
            pid,
            fd,
        })
    }
}

impl Dentry for ProcFdinfoDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }

    fn open(self: Arc<Self>, _flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Ok(Arc::new(ProcFdinfoFile::new(self)))
    }
}

struct ProcFdinfoFile {
    inner: Mutex<FileInner>,
    pid: usize,
    fd: usize,
}

impl ProcFdinfoFile {
    fn new(dentry: Arc<ProcFdinfoDentry>) -> Self {
        let pid = dentry.pid;
        let fd = dentry.fd;
        Self {
            inner: Mutex::new(FileInner {
                offset: 0,
                dentry,
                flags: OpenFlags::empty(),
            }),
            pid,
            fd,
        }
    }

    fn render(&self) -> SysResult<String> {
        let process = pid2process(self.pid).ok_or(SysError::ENOENT)?;
        let inner = process.inner_exclusive_access();
        let file = if self.fd < inner.fd_table.len() {
            inner.fd_table[self.fd].clone()
        } else {
            None
        }
        .ok_or(SysError::ENOENT)?;
        let flags = file.status_flags();
        let pos = file.get_offset();
        drop(inner);

        let mut info = format!("pos:\t{}\nflags:\t{:o}\n", pos, flags);
        info.push_str("mnt_id:\t1\n");
        if let Some(pid) = file.pidfd_pid() {
            info.push_str(&format!("Pid:\t{}\nNSpid:\t{}\n", pid, pid));
        }
        if let Some(inotify_info) = inotify_fdinfo(&file) {
            info.push_str(&inotify_info);
        }
        if let Some(fanotify_info) = fanotify_fdinfo(&file) {
            info.push_str(&fanotify_info);
        }
        Ok(info)
    }
}

impl File for ProcFdinfoFile {
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
        let info = self.render()?;
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
        Err(SysError::EINVAL)
    }

    fn open(&self) -> SyscallResult {
        Ok(0)
    }
}
