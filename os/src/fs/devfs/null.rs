use alloc::sync::{Arc, Weak};
use alloc::string::ToString; 

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

    fn read(&self, _buf: UserBuffer) -> usize{
        0
    }
    /// Write `UserBuffer` to file
    fn write(&self, buf: UserBuffer) -> usize{
        buf.len()
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
    
    fn open(self: Arc<Self>, _flags: OpenFlags,_mode: InodeMode) -> Option<Arc<dyn File>> {
        Some(Arc::new(NullFile::new(self)))
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
            inner: InodeInner::new(inode_alloc(), 0, mode),
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
    fn inc_nlink(&self) {
        self.inner.nlink.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
    }
    fn dec_nlink(&self) {
        self.inner.nlink.fetch_sub(1, core::sync::atomic::Ordering::SeqCst);
    }
}