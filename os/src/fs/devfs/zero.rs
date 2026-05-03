use crate::error::{SysError, SysResult, SyscallResult};
use alloc::sync::{Arc, Weak};
use alloc::string::ToString; 
use core::sync::atomic::Ordering::{Relaxed,SeqCst};
use spin::{Mutex, MutexGuard};
use crate::fs::vfs::inode::inode_alloc;
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
pub struct ZeroFile{
    inner: Mutex<FileInner>,
}

impl ZeroFile {
    ///
    pub fn new(dentry: Arc<dyn Dentry>) -> Self {
        Self {
            inner: Mutex::new(FileInner {
                offset: 0,
                dentry,
            }),
        }
    }
}

///
impl File for ZeroFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        self.inner.lock()
    }

    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        true
    }

    fn read(&self, buf: UserBuffer) -> SysResult<usize>{
        let mut total = 0;
        for slice in buf.buffers {
            for byte in slice.iter_mut() {
                *byte = 0;
            }
            total += slice.len();
        }
        Ok(total)
    }
    /// Write `UserBuffer` to file
    fn write(&self, buf: UserBuffer) -> SysResult<usize>{
        Ok(buf.len())
    }
}

unsafe impl Send for ZeroDentry {}
unsafe impl Sync for ZeroDentry {}
///
pub struct ZeroDentry {
    inner: DentryInner,
}
impl ZeroDentry {
    ///
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|_me: &Weak<ZeroDentry>| {
            Self {
                inner: DentryInner::new(name, parent_weak.clone()),
            }
        })
    }
}

impl Dentry for ZeroDentry {
    fn get_dentryinner(&self)->&DentryInner{
        &self.inner
    }
    ///name
    fn name(&self) -> &str{
        "zero"
    }
    
    fn open(self: Arc<Self>, _flags: OpenFlags,_mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Ok(Arc::new(ZeroFile::new(self)))
    }
}
#[allow(unused)]
///
pub struct ZeroInode {
    inner : InodeInner,
}

impl ZeroInode {
    ///
    pub fn new() -> Self {
        let mode = InodeMode::CHAR;
        Self {
            inner: InodeInner::new(inode_alloc(), 0, mode),
        }
    }
}

impl Inode for ZeroInode{
    fn get_mode(&self) -> InodeMode {
        self.inner.mode
    }
    fn set_size(&self, new_size: usize) {
        self.inner.size.store(new_size, SeqCst);
    }
    fn get_size(&self) -> usize {
        self.inner.size.load(SeqCst)
    }
    fn get_ino(&self) -> usize {
        self.inner.ino
    }
    fn get_nlink(&self) -> usize {
        self.inner.nlink.load(SeqCst)
    }
    fn inc_nlink(&self) {
        self.inner.nlink.fetch_add(1, SeqCst);
    }
    fn dec_nlink(&self) {
        self.inner.nlink.fetch_sub(1, SeqCst);
    }

    fn get_atime(&self) -> (i64, i64) {
        (
            self.inner.atime_sec.load(Relaxed),
            self.inner.atime_nsec.load(Relaxed),
        )
    }

    fn set_atime(&self, sec: i64, nsec: i64) {
        self.inner.atime_sec.store(sec, Relaxed);
        self.inner.atime_nsec.store(nsec, Relaxed);
    }

    fn get_mtime(&self) -> (i64, i64) {
        (
            self.inner.mtime_sec.load(Relaxed),
            self.inner.mtime_nsec.load(Relaxed),
        )
    }

    fn set_mtime(&self, sec: i64, nsec: i64) {
        self.inner.mtime_sec.store(sec, Relaxed);
        self.inner.mtime_nsec.store(nsec, Relaxed);
    }

    fn get_ctime(&self) -> (i64, i64) {
        (
            self.inner.ctime_sec.load(Relaxed),
            self.inner.ctime_nsec.load(Relaxed),
        )
    }

    fn set_ctime(&self, sec: i64, nsec: i64) {
        self.inner.ctime_sec.store(sec, Relaxed);
        self.inner.ctime_nsec.store(nsec, Relaxed);
    }
}
