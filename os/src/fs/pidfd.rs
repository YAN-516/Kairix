use crate::error::{SysError, SysResult};
use crate::fs::TempDentry;
use crate::fs::TempInode;
use crate::fs::vfs::FileInner;
use crate::fs::vfs::OpenFlags;
use crate::fs::vfs::file::File;
use crate::fs::vfs::inode::InodeMode;
use crate::mm::UserBuffer;
use alloc::sync::Arc;
use spin::MutexGuard;
use spin::mutex::Mutex;
/// PidFd 文件，代表一个进程的文件描述符
pub struct PidFdFile {
    pid: usize,
    inner: Mutex<FileInner>,
}

impl PidFdFile {
    /// Create a new PidFdFile for the given pid
    pub fn new(pid: usize) -> Self {
        let _dummy_inode = Arc::new(TempInode::new(InodeMode::FILE));
        let dummy_dentry = TempDentry::new("pidfd", None);
        // TempDentry::new returns Arc<dyn Dentry>, we need to get the inner dentry
        // Actually, let's use a simpler approach: just create a minimal dentry
        Self {
            pid,
            inner: Mutex::new(FileInner {
                offset: 0,
                dentry: dummy_dentry,
                flags: OpenFlags::empty(),
            }),
        }
    }
}

impl File for PidFdFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        self.inner.lock()
    }

    fn readable(&self) -> bool {
        false
    }

    fn writable(&self) -> bool {
        false
    }

    fn supports_epoll(&self) -> bool {
        true
    }

    fn read(&self, _buf: UserBuffer) -> SysResult<usize> {
        Err(SysError::EINVAL)
    }

    fn write(&self, _buf: UserBuffer) -> SysResult<usize> {
        Err(SysError::EINVAL)
    }

    fn is_pidfd(&self) -> bool {
        true
    }

    fn pidfd_pid(&self) -> Option<usize> {
        Some(self.pid)
    }
}
