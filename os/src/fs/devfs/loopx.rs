#![allow(missing_docs)]
use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::vfs::{
    DentryInner, FileInner, OpenFlags,
    inode::{InodeInner, InodeMode, inode_alloc, make_rdev},
};
use crate::fs::{Dentry, File, Inode, String};
use crate::mm::{translated_refmut, UserBuffer};
use crate::task::{current_process, current_user_token};
use alloc::sync::{Arc, Weak};
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::{Mutex, MutexGuard};

pub struct LoopControlFile {
    inner: Mutex<FileInner>,
}

impl LoopControlFile {
    pub fn new(dentry: Arc<dyn Dentry>) -> Self {
        Self {
            inner: Mutex::new(FileInner { offset: 0, dentry }),
        }
    }
}

impl File for LoopControlFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        self.inner.lock()
    }

    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        true
    }

    fn read(&self, _buf: UserBuffer) -> SysResult<usize> {
        Ok(0)
    }

    fn write(&self, buf: UserBuffer) -> SysResult<usize> {
        Ok(buf.len())
    }

    fn ioctl(&self, request: usize, argp: usize) -> SyscallResult {
        const LOOP_CTL_GET_FREE: usize = 0x4C82;
        match request {
            LOOP_CTL_GET_FREE => {
                if argp == 0 {
                    return Err(SysError::EINVAL);
                }
                let token = current_user_token();
                let dev_no = translated_refmut(token, argp as *mut i32)?;
                // 简化实现：总是返回 loop0
                *dev_no = 0;
                Ok(0)
            }
            _ => Err(SysError::ENOTTY),
        }
    }
}

unsafe impl Send for LoopControlDentry {}
unsafe impl Sync for LoopControlDentry {}

pub struct LoopControlDentry {
    inner: DentryInner,
}

impl LoopControlDentry {
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|_me| Self {
            inner: DentryInner::new(name, parent_weak),
        })
    }
}

impl Dentry for LoopControlDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }

    fn name(&self) -> &str {
        &self.inner.name
    }

    fn open(self: Arc<Self>, _flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Ok(Arc::new(LoopControlFile::new(self)))
    }
}

pub struct LoopControlInode {
    inner: InodeInner,
}

impl LoopControlInode {
    pub fn new() -> Self {
        Self {
            inner: InodeInner::new(inode_alloc(), 0, InodeMode::CHAR, make_rdev(10, 237) as usize),
        }
    }
}

impl Inode for LoopControlInode {
    fn get_mode(&self) -> InodeMode {
        self.inner.mode
    }

    fn get_size(&self) -> usize {
        self.inner.size.load(Ordering::SeqCst)
    }

    fn set_size(&self, new_size: usize) {
        self.inner.size.store(new_size, Ordering::SeqCst);
    }

    fn get_ino(&self) -> usize {
        self.inner.ino
    }

    fn get_nlink(&self) -> usize {
        self.inner.nlink.load(Ordering::SeqCst)
    }
    fn get_rdev(&self) -> usize {
        self.inner.rdev.load(Ordering::Relaxed)
    }
    fn set_rdev(&self, rdev: usize) {
        self.inner.rdev.store(rdev, Ordering::Relaxed);
    }
    fn inc_nlink(&self) {
        self.inner.nlink.fetch_add(1, Ordering::SeqCst);
    }
    fn dec_nlink(&self) {
        self.inner.nlink.fetch_sub(1, Ordering::SeqCst);
    }
    fn get_atime(&self) -> (i64, i64) {
        (
            self.inner.atime_sec.load(Ordering::Relaxed),
            self.inner.atime_nsec.load(Ordering::Relaxed),
        )
    }
    fn set_atime(&self, sec: i64, nsec: i64) {
        self.inner.atime_sec.store(sec, Ordering::Relaxed);
        self.inner.atime_nsec.store(nsec, Ordering::Relaxed);
    }
    fn get_mtime(&self) -> (i64, i64) {
        (
            self.inner.mtime_sec.load(Ordering::Relaxed),
            self.inner.mtime_nsec.load(Ordering::Relaxed),
        )
    }
    fn set_mtime(&self, sec: i64, nsec: i64) {
        self.inner.mtime_sec.store(sec, Ordering::Relaxed);
        self.inner.mtime_nsec.store(nsec, Ordering::Relaxed);
    }
    fn get_ctime(&self) -> (i64, i64) {
        (
            self.inner.ctime_sec.load(Ordering::Relaxed),
            self.inner.ctime_nsec.load(Ordering::Relaxed),
        )
    }
    fn set_ctime(&self, sec: i64, nsec: i64) {
        self.inner.ctime_sec.store(sec, Ordering::Relaxed);
        self.inner.ctime_nsec.store(nsec, Ordering::Relaxed);
    }
}

pub struct LoopDeviceFile {
    inner: Mutex<FileInner>,
    #[allow(unused)]
    id: usize,
}

impl LoopDeviceFile {
    pub fn new(dentry: Arc<dyn Dentry>, id: usize) -> Self {
        Self {
            inner: Mutex::new(FileInner { offset: 0, dentry }),
            id,
        }
    }
}

impl File for LoopDeviceFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        self.inner.lock()
    }

    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        true
    }

    fn read(&self, _buf: UserBuffer) -> SysResult<usize> {
        Ok(0)
    }

    fn write(&self, buf: UserBuffer) -> SysResult<usize> {
        Ok(buf.len())
    }

    fn get_stat(&self, stat: &mut crate::fs::vfs::kstat::Kstat) -> SysResult<()> {
        let inode = self.get_inode().ok_or(SysError::EIO)?;
        stat.st_ino = inode.get_ino() as u64;
        stat.st_nlink = inode.get_nlink() as u32;
        stat.st_mode = inode.get_mode().bits();
        stat.st_blksize = 512;
        stat.st_rdev = inode.get_rdev() as u64;
        let (atime_sec, atime_nsec) = inode.get_atime();
        let (mtime_sec, mtime_nsec) = inode.get_mtime();
        let (ctime_sec, ctime_nsec) = inode.get_ctime();
        stat.st_atime_sec = atime_sec;
        stat.st_atime_nsec = atime_nsec;
        stat.st_mtime_sec = mtime_sec;
        stat.st_mtime_nsec = mtime_nsec;
        stat.st_ctime_sec = ctime_sec;
        stat.st_ctime_nsec = ctime_nsec;

        let fd = inode.get_backing_fd();
        let mut size = inode.get_size() as i64;
        if let Some(fd) = fd {
            let process = current_process();
            let inner = process.inner_exclusive_access();
            if let Some(file) = inner.fd_table.get(fd).and_then(|x| x.as_ref()) {
                if let Some(backing_inode) = file.get_inode() {
                    size = backing_inode.get_size() as i64;
                }
            }
        }
        stat.st_size = size;
        stat.st_blocks = (size as u64 + 511) / 512;
        Ok(())
    }

    fn ioctl(&self, request: usize, argp: usize) -> SyscallResult {
        const LOOP_SET_FD: usize = 0x4C00;
        const LOOP_CLR_FD: usize = 0x4C01;
        const LOOP_SET_STATUS: usize = 0x4C02;
        const LOOP_GET_STATUS: usize = 0x4C03;
        const LOOP_SET_STATUS64: usize = 0x4C04;
        const LOOP_GET_STATUS64: usize = 0x4C05;
        const BLKGETSIZE64: usize = 0x8008_1272;
        match request {
            LOOP_GET_STATUS | LOOP_GET_STATUS64 => {
                // 设备未绑定，返回 ENXIO 表示空闲
                Err(SysError::ENXIO)
            }
            LOOP_SET_FD => {
                if let Some(inode) = self.get_inode() {
                    inode.set_backing_fd(Some(argp));
                }
                Ok(0)
            }
            LOOP_CLR_FD => {
                if let Some(inode) = self.get_inode() {
                    if inode.get_backing_fd().is_none() {
                        return Err(SysError::ENXIO);
                    }
                    inode.set_backing_fd(None);
                }
                Ok(0)
            }
            LOOP_SET_STATUS | LOOP_SET_STATUS64 => {
                // TODO: 设置 loop 设备参数
                Ok(0)
            }
            BLKGETSIZE64 => {
                if argp == 0 {
                    return Err(SysError::EINVAL);
                }
                let token = current_user_token();
                let size_ptr = translated_refmut(token, argp as *mut u64)?;
                let fd = self.get_inode().and_then(|inode| inode.get_backing_fd());
                let mut size = 0u64;
                if let Some(fd) = fd {
                    let process = current_process();
                    let inner = process.inner_exclusive_access();
                    if let Some(file) = inner.fd_table.get(fd).and_then(|x| x.as_ref()) {
                        if let Some(inode) = file.get_inode() {
                            size = inode.get_size() as u64;
                        }
                    }
                }
                *size_ptr = size;
                Ok(0)
            }
            _ => Err(SysError::ENOTTY),
        }
    }
}

unsafe impl Send for LoopDeviceDentry {}
unsafe impl Sync for LoopDeviceDentry {}

pub struct LoopDeviceDentry {
    inner: DentryInner,
}

impl LoopDeviceDentry {
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|_me| Self {
            inner: DentryInner::new(name, parent_weak),
        })
    }
}

impl Dentry for LoopDeviceDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }

    fn name(&self) -> &str {
        &self.inner.name
    }

    fn open(self: Arc<Self>, _flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        let name = self.name();
        let id = name
            .strip_prefix("loop")
            .unwrap_or(name)
            .parse::<usize>()
            .unwrap_or(0);
        Ok(Arc::new(LoopDeviceFile::new(self, id)))
    }
}

pub struct LoopDeviceInode {
    inner: InodeInner,
    backing_fd: AtomicUsize,
}

impl LoopDeviceInode {
    pub fn new(id: usize) -> Self {
        Self {
            inner: InodeInner::new(inode_alloc(), 0, InodeMode::BLOCK, make_rdev(7, id as u32) as usize),
            backing_fd: AtomicUsize::new(usize::MAX),
        }
    }
}

impl Inode for LoopDeviceInode {
    fn get_mode(&self) -> InodeMode {
        self.inner.mode
    }

    fn get_size(&self) -> usize {
        self.inner.size.load(Ordering::SeqCst)
    }

    fn set_size(&self, new_size: usize) {
        self.inner.size.store(new_size, Ordering::SeqCst);
    }

    fn get_ino(&self) -> usize {
        self.inner.ino
    }

    fn get_nlink(&self) -> usize {
        self.inner.nlink.load(Ordering::SeqCst)
    }
    fn get_rdev(&self) -> usize {
        self.inner.rdev.load(Ordering::Relaxed)
    }
    fn set_rdev(&self, rdev: usize) {
        self.inner.rdev.store(rdev, Ordering::Relaxed);
    }
    fn inc_nlink(&self) {
        self.inner.nlink.fetch_add(1, Ordering::SeqCst);
    }
    fn dec_nlink(&self) {
        self.inner.nlink.fetch_sub(1, Ordering::SeqCst);
    }
    fn get_atime(&self) -> (i64, i64) {
        (
            self.inner.atime_sec.load(Ordering::Relaxed),
            self.inner.atime_nsec.load(Ordering::Relaxed),
        )
    }

    fn set_atime(&self, sec: i64, nsec: i64) {
        self.inner.atime_sec.store(sec, Ordering::Relaxed);
        self.inner.atime_nsec.store(nsec, Ordering::Relaxed);
    }

    fn get_mtime(&self) -> (i64, i64) {
        (
            self.inner.mtime_sec.load(Ordering::Relaxed),
            self.inner.mtime_nsec.load(Ordering::Relaxed),
        )
    }

    fn set_mtime(&self, sec: i64, nsec: i64) {
        self.inner.mtime_sec.store(sec, Ordering::Relaxed);
        self.inner.mtime_nsec.store(nsec, Ordering::Relaxed);
    }

    fn get_ctime(&self) -> (i64, i64) {
        (
            self.inner.ctime_sec.load(Ordering::Relaxed),
            self.inner.ctime_nsec.load(Ordering::Relaxed),
        )
    }

    fn set_ctime(&self, sec: i64, nsec: i64) {
        self.inner.ctime_sec.store(sec, Ordering::Relaxed);
        self.inner.ctime_nsec.store(nsec, Ordering::Relaxed);
    }

    fn get_backing_fd(&self) -> Option<usize> {
        let fd = self.backing_fd.load(Ordering::Relaxed);
        if fd == usize::MAX { None } else { Some(fd) }
    }

    fn set_backing_fd(&self, fd: Option<usize>) {
        self.backing_fd.store(fd.unwrap_or(usize::MAX), Ordering::Relaxed);
    }
}
