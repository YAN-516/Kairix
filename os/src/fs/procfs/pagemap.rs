// /workspace/os/src/fs/procfs/pagemap.rs

use crate::error::SysError;
use crate::error::SysResult;
use crate::fs::vfs::{Dentry, DentryInner, File, FileInner, Inode, OpenFlags};
use crate::fs::InodeMode;
use crate::mm::UserBuffer;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

///
pub struct PagemapDentry {
    inner: DentryInner,
}

impl PagemapDentry {
    ///
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new(Self {
            inner: DentryInner::new(name, parent_weak),
        })
    }
}

impl Dentry for PagemapDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }

    fn open(self: Arc<Self>, _flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Ok(Arc::new(PagemapFile::new(self)))
    }
}
///
pub struct PagemapInodeInner {
    mode: InodeMode,
    size: AtomicUsize,
}
///
pub struct PagemapInode {
    inner: PagemapInodeInner,
}

impl PagemapInode {
    ///
    pub fn new() -> Self {
        Self {
            inner: PagemapInodeInner {
                mode: InodeMode::FILE,
                size: AtomicUsize::new(0),
            },
        }
    }
}

impl Inode for PagemapInode {
    fn get_mode(&self) -> InodeMode {
        self.inner.mode
    }

    fn set_size(&self, new_size: usize) {
        self.inner.size.store(new_size, Ordering::SeqCst);
    }

    fn get_size(&self) -> usize {
        self.inner.size.load(Ordering::SeqCst)
    }
}
///
pub struct PagemapFile {
    inner: Mutex<FileInner>,
}

impl PagemapFile {
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

impl File for PagemapFile {
    fn get_fileinner(&self) -> spin::MutexGuard<'_, FileInner> {
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
        let _offset = inner.offset;

        let entry: u64 = 0x1; // 页面存在但没有实际物理地址

        let mut total = 0usize;
        let entry_bytes = entry.to_ne_bytes();

        for slice in buf.buffers.iter_mut() {
            let len = slice.len().min(entry_bytes.len());
            if len == 0 {
                break;
            }
            slice[..len].copy_from_slice(&entry_bytes[..len]);
            total += len;
        }

        inner.offset += total;
        Ok(total)
    }

    // 添加 write 方法（pagemap 是只读的，返回错误）
    fn write(&self, _buf: UserBuffer) -> SysResult<usize> {
        Err(SysError::EPERM)
    }
}
