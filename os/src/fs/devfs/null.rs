use alloc::sync::{Arc, Weak};
use alloc::string::ToString; 

use spin::{Mutex, MutexGuard};

use crate::fs::{
    vfs::{
        inode::{InodeInner, InodeMode},
        DentryInner, FileInner,
    },
    BTreeMap, Dentry, File, Inode, String,
};

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
    pub fn new(name: &str, parent: Option<Weak<dyn Dentry>>) -> Self {
        Self {
            inner: DentryInner::new(name, parent),
        }
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
            inner: InodeInner::new(0, 0, mode),
        }
    }
}

impl Inode for NullInode{

}