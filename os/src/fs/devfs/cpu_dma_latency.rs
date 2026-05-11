use crate::error::{SysError, SysResult};
use alloc::sync::{Arc, Weak};
use spin::{Mutex, MutexGuard};
use crate::fs::vfs::inode::inode_alloc;
use crate::fs::{
    vfs::{
        inode::{InodeInner, InodeMode},
        DentryInner, FileInner,
    },
    Dentry, File, Inode,
};
use crate::fs::vfs::OpenFlags;
use crate::mm::UserBuffer;

/// Current DMA latency target (in microseconds)
static DMA_LATENCY: Mutex<Option<i32>> = Mutex::new(None);

/// cpu_dma_latency file implementation
pub struct CpuDmaLatencyFile {
    inner: Mutex<FileInner>,
}

impl CpuDmaLatencyFile {
    ///
    pub fn new(dentry: Arc<dyn Dentry>) -> Self {
        // When opened, set latency to 0 (lowest possible)
        *DMA_LATENCY.lock() = Some(0);
        Self {
            inner: Mutex::new(FileInner {
                offset: 0,
                dentry,
            }),
        }
    }
}

impl File for CpuDmaLatencyFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        self.inner.lock()
    }

    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        true
    }

    /// Read returns the current latency setting
    fn read(&self, buf: UserBuffer) -> SysResult<usize> {
        if buf.len() < core::mem::size_of::<i32>() {
            return Err(SysError::EINVAL);
        }
        let latency = *DMA_LATENCY.lock();
        let value = latency.unwrap_or(0);
        
        let mut data = [0u8; 4];
        data.copy_from_slice(&value.to_le_bytes());
        
        let mut written = 0;
        for chunk in buf.buffers.into_iter() {
            if written >= 4 {
                break;
            }
            let to_write = core::cmp::min(chunk.len(), 4 - written);
            chunk.copy_from_slice(&data[written..written + to_write]);
            written += to_write;
        }
        
        Ok(4)
    }

    /// Write sets the latency target (in microseconds)
    fn write(&self, buf: UserBuffer) -> SysResult<usize> {
        if buf.len() < core::mem::size_of::<i32>() {
            return Err(SysError::EINVAL);
        }
        
        let mut data = [0u8; 4];
        let mut read = 0;
        for chunk in buf.buffers.into_iter() {
            if read >= 4 {
                break;
            }
            let to_read = core::cmp::min(chunk.len(), 4 - read);
            data[read..read + to_read].copy_from_slice(&chunk[..to_read]);
            read += to_read;
        }
        
        let latency = i32::from_le_bytes(data);
        *DMA_LATENCY.lock() = Some(latency);
        
        Ok(read)
    }
}

unsafe impl Send for CpuDmaLatencyDentry {}
unsafe impl Sync for CpuDmaLatencyDentry {}

/// Dentry for cpu_dma_latency device
pub struct CpuDmaLatencyDentry {
    inner: DentryInner,
}

impl CpuDmaLatencyDentry {
    #[allow(unused)]
    ///
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|_me: &Weak<CpuDmaLatencyDentry>| {
            Self {
                inner: DentryInner::new(name, parent_weak.clone()),
            }
        })
    }
}

impl Dentry for CpuDmaLatencyDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }

    fn name(&self) -> &str {
        "cpu_dma_latency"
    }

    fn open(self: Arc<Self>, _flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Ok(Arc::new(CpuDmaLatencyFile::new(self)))
    }
}

/// Inode for cpu_dma_latency device
pub struct CpuDmaLatencyInode {
    inner: InodeInner,
}

impl CpuDmaLatencyInode {
    #[allow(unused)]
    ///
    pub fn new() -> Self {
        let mode = InodeMode::CHAR;
        Self {
            inner: InodeInner::new(inode_alloc(), 0, mode),
        }
    }
}

impl Inode for CpuDmaLatencyInode {
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