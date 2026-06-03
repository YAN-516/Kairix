// /workspace/os/src/fs/procfs/pagemap.rs

use crate::error::SysError;
use crate::error::SysResult;
use crate::fs::InodeMode;
use crate::fs::vfs::inode::{InodeInner, inode_alloc};
use crate::fs::vfs::{Dentry, DentryInner, File, FileInner, Inode, OpenFlags};
use crate::mm::UserBuffer;
use crate::task::current_process;
use alloc::sync::Arc;
use core::sync::atomic::Ordering;
use polyhal::utils::addr::VirtPageNum;
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
pub struct PagemapInode {
    inner: InodeInner,
}

impl PagemapInode {
    ///
    pub fn new() -> Self {
        Self {
            inner: InodeInner::new(inode_alloc(), 0, InodeMode::FILE, 0),
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
        let mut offset = inner.offset;
        let process = current_process();
        let proc_inner = process.inner_exclusive_access();

        let mut total = 0usize;
        for slice in buf.buffers.iter_mut() {
            for byte in slice.iter_mut() {
                let entry_offset = offset / core::mem::size_of::<u64>();
                let byte_offset = offset % core::mem::size_of::<u64>();
                let vpn = VirtPageNum(entry_offset);
                let entry = if proc_inner.vm_set.page_table.translate(vpn).is_some() {
                    1u64 << 63
                } else {
                    0
                };
                *byte = entry.to_ne_bytes()[byte_offset];
                offset += 1;
                total += 1;
            }
        }
        drop(proc_inner);

        inner.offset = offset;
        Ok(total)
    }

    // 添加 write 方法（pagemap 是只读的，返回错误）
    fn write(&self, _buf: UserBuffer) -> SysResult<usize> {
        Err(SysError::EPERM)
    }
}
