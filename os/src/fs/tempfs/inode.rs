
use crate::fs::Inode;
use crate::fs::vfs::inode::{InodeMode,InodeInner};
use log::info;
use spin::mutex::Mutex;
use core::sync::atomic::Ordering;

#[allow(unused)]
/// the inode of tempfs
pub struct TempInode {
    inner : Mutex<InodeInner>,
    this_mode: InodeMode,
}

impl TempInode {
    ///
    pub fn new(ino:usize, mode: InodeMode) -> Self {
        info!("Inode new {:?} with ino {}", mode, ino);
        Self{
            inner: Mutex::new(InodeInner::new(ino,0,mode)),
            this_mode: mode
        }
    }
}

impl Inode for TempInode{
    /// Get the attributes of the file, such as size, permissions, etc.
    fn get_attr(&self) -> Result<usize, i32> {
        unimplemented!()
    }
    /// Flush the file, synchronize the data to disk.
    fn fsync(&self) -> Result<usize, i32> {
        unimplemented!()
    }
    ///
    fn get_ino(&self) -> usize {
        self.inner.lock().ino
    }
    
    fn get_size(&self) -> usize {
        self.inner.lock().size.load(Ordering::Relaxed)
    }

    fn set_size(&self, new_size: usize) {
        self.inner.lock().size.store(new_size, Ordering::Relaxed);
    }

    fn get_nlink(&self) -> usize {
        self.inner.lock().nlink.load(Ordering::Relaxed)
    }

    fn get_mode(&self) -> InodeMode {
        self.inner.lock().mode
    }
    fn inc_nlink(&self) {
        self.inner.lock().nlink.fetch_add(1, Ordering::SeqCst);
    }
    
    fn dec_nlink(&self) {
        self.inner.lock().nlink.fetch_sub(1, Ordering::SeqCst);
    }
}