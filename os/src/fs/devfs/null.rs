use crate::error::{SysError, SysResult, SyscallResult};
use alloc::sync::{Arc, Weak};
use alloc::string::ToString; 

use spin::{Mutex, MutexGuard};
use crate::fs::vfs::inode::inode_alloc;
use crate::fs::vfs::inode::make_rdev;
use crate::fs::{
    vfs::{
        inode::{InodeInner, InodeMode},
        DentryInner, FileInner,
    },
    BTreeMap, Dentry, File, Inode, String,
};
use crate::fs::vfs::OpenFlags;
use crate::mm::UserBuffer;
///
pub struct NullFile{
    inner: Mutex<FileInner>,
}

impl NullFile {
    ///
    pub fn new(dentry: Arc<dyn Dentry>) -> Self {
        Self {
            inner: Mutex::new(FileInner {
                offset: 0,
                dentry,
                flags: OpenFlags::empty(),
            }),
        }
    }
}

///
impl File for NullFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        self.inner.lock()
    }

    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        true
    }

    fn read(&self, _buf: UserBuffer) -> SysResult<usize>{
        Ok(0)
    }
    /// Write `UserBuffer` to file
    fn write(&self, buf: UserBuffer) -> SysResult<usize>{
        Ok(buf.len())
    }
}

unsafe impl Send for NullDentry {}
unsafe impl Sync for NullDentry {}
///
pub struct NullDentry {
    inner: DentryInner,
}
impl NullDentry {
    ///
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|_me: &Weak<NullDentry>| {
            Self {
                inner: DentryInner::new(name, parent_weak.clone()),
            }
        })
    }
}

impl Dentry for NullDentry {
    fn get_dentryinner(&self)->&DentryInner{
        &self.inner
    }
    ///name
    fn name(&self) -> &str{
        "null"
    }
    
    fn open(self: Arc<Self>, _flags: OpenFlags,_mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Ok(Arc::new(NullFile::new(self)))
    }
}
#[allow(unused)]
///
pub struct NullInode {
    inner : InodeInner,
}

impl NullInode {
    ///
    pub fn new() -> Self {
        let mode = InodeMode::CHAR;
        Self {
            inner: InodeInner::new(inode_alloc(), 0, mode, make_rdev(1, 3) as usize),
        }
    }
}

impl Inode for NullInode{
    fn get_mode(&self) -> InodeMode {
        self.inner.mode
    }
    fn set_size(&self, new_size: usize) {
        self.inner.size.store(new_size, core::sync::atomic::Ordering::SeqCst);
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
    fn get_rdev(&self) -> usize {
        self.inner.rdev.load(core::sync::atomic::Ordering::Relaxed)
    }
    fn set_rdev(&self, rdev: usize) {
        self.inner.rdev.store(rdev, core::sync::atomic::Ordering::Relaxed);
    }
    fn inc_nlink(&self) {
        self.inner.nlink.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
    }
    fn dec_nlink(&self) {
        self.inner.nlink.fetch_sub(1, core::sync::atomic::Ordering::SeqCst);
    }

    fn get_atime(&self) -> (i64, i64) {
        (
            self.inner.atime_sec.load(core::sync::atomic::Ordering::Relaxed),
            self.inner.atime_nsec.load(core::sync::atomic::Ordering::Relaxed),
        )
    }

    fn set_atime(&self, sec: i64, nsec: i64) {
        self.inner.atime_sec.store(sec, core::sync::atomic::Ordering::Relaxed);
        self.inner.atime_nsec.store(nsec, core::sync::atomic::Ordering::Relaxed);
    }

    fn get_mtime(&self) -> (i64, i64) {
        (
            self.inner.mtime_sec.load(core::sync::atomic::Ordering::Relaxed),
            self.inner.mtime_nsec.load(core::sync::atomic::Ordering::Relaxed),
        )
    }

    fn set_mtime(&self, sec: i64, nsec: i64) {
        self.inner.mtime_sec.store(sec, core::sync::atomic::Ordering::Relaxed);
        self.inner.mtime_nsec.store(nsec, core::sync::atomic::Ordering::Relaxed);
    }

    fn get_ctime(&self) -> (i64, i64) {
        (
            self.inner.ctime_sec.load(core::sync::atomic::Ordering::Relaxed),
            self.inner.ctime_nsec.load(core::sync::atomic::Ordering::Relaxed),
        )
    }

    fn set_ctime(&self, sec: i64, nsec: i64) {
        self.inner.ctime_sec.store(sec, core::sync::atomic::Ordering::Relaxed);
        self.inner.ctime_nsec.store(nsec, core::sync::atomic::Ordering::Relaxed);
    }
}
