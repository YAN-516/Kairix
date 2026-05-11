use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::Dentry;
use crate::fs::File;
use crate::fs::Inode;
use crate::fs::page::pagecache::PAGE_CACHE;
use crate::fs::page::pagecache::Page;
use crate::fs::vfs::FileInner;
use crate::fs::vfs::kstat::Kstat;
use crate::mm::UserBuffer;
use crate::mm::frame_alloc;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use log::*;
use polyhal::consts::PAGE_SIZE;
use polyhal::common::FrameTracker;
use spin::MutexGuard;
use spin::mutex::Mutex;
use spin::rwlock::RwLock;
/// the file of tempfs
pub struct TempFile {
    inner: Mutex<FileInner>,
}

impl TempFile {
    ///
    pub fn new(dentry: Arc<dyn Dentry>) -> Self {
        Self {
            inner: Mutex::new(FileInner { offset: 0, dentry }),
        }
    }
}

impl File for TempFile {
    ///Get the FileInner
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        self.inner.lock()
    }
    /// If readable
    fn readable(&self) -> bool {
        true
    }
    /// If writable
    fn writable(&self) -> bool {
        true
    }
    fn read_all(&self) -> Vec<u8> {
        let old_offset = {
            let mut inner = self.inner.lock();
            let off = inner.offset;
            inner.offset = 0;
            off
        };
        let mut v: Vec<u8> = Vec::new();
        let mut buffer = [0u8; PAGE_SIZE];
        loop {
            let static_buf: &'static mut [u8] =
                unsafe { core::slice::from_raw_parts_mut(buffer.as_mut_ptr(), buffer.len()) };
            let user_buffer = UserBuffer::new(vec![static_buf]);
            match self.read(user_buffer) {
                Ok(0) => break,
                Ok(read_len) => v.extend_from_slice(&buffer[..read_len]),
                Err(_) => break,
            }
        }
        self.inner.lock().offset = old_offset;
        v
    }
    //read the data
    fn read(&self, mut buf: UserBuffer) -> SysResult<usize> {
        let mut inner = self.get_fileinner();
        let inode = inner.dentry.get_inode().ok_or(SysError::EIO)?;
        let ino = inode.get_ino();
        let file_size = inode.get_size();
        let mut current_offset = inner.offset;
        let mut total_read_size = 0usize;
        if current_offset >= file_size {
            return Ok(0);
        }
        for slice in buf.buffers.iter_mut() {
            let mut slice_offset = 0;
            let slice_len = slice.len();
            while slice_offset < slice_len && current_offset < file_size {
                let target_page = self.get_or_alloc_cache_page(ino, current_offset / PAGE_SIZE);
                {
                    let page_reader = target_page.read();
                    let page_offset = current_offset % PAGE_SIZE;
                    let left_in_page = PAGE_SIZE - page_offset;
                    let left_in_slice = slice_len - slice_offset;
                    let left_in_file = file_size - current_offset;
                    let read_bytes = left_in_page.min(left_in_slice).min(left_in_file);
                    let src_data = &page_reader.frame.ppn.get_bytes_array()
                        [page_offset..page_offset + read_bytes];
                    slice[slice_offset..slice_offset + read_bytes].copy_from_slice(src_data);

                    current_offset += read_bytes;
                    slice_offset += read_bytes;
                    total_read_size += read_bytes;
                }
            }
        }
        inner.offset = current_offset;
        Ok(total_read_size)
    }

    fn write(&self, buf: UserBuffer) -> SysResult<usize> {
        info!("enter VFS Write-back Cache");
        let mut inner = self.inner.lock();
        let inode = inner.dentry.get_inode().ok_or(SysError::EIO)?;
        let ino = inode.get_ino();
        // println!("[DEBUG] 当前操作的 ino: {}", ino);
        let old_size = inode.get_size();
        let mut total_write_size = 0usize;
        let mut current_offset = inner.offset;
        for slice in buf.buffers.iter() {
            let mut slice_offset = 0;
            let slice_len = slice.len();
            while slice_offset < slice_len {
                let page_id = current_offset / PAGE_SIZE;
                let page_offset = current_offset % PAGE_SIZE;
                let write_bytes = (PAGE_SIZE - page_offset).min(slice_len - slice_offset);
                // 获取缓存页
                let target_page = self.get_or_alloc_cache_page(ino, page_id);
                // 写入数据并标记脏页
                {
                    let mut page_writer = target_page.write();
                    let data_to_write = &slice[slice_offset..slice_offset + write_bytes];
                    page_writer.modify(page_offset, data_to_write);
                }
                current_offset += write_bytes;
                slice_offset += write_bytes;
                total_write_size += write_bytes;
            }
        }
        if current_offset > old_size {
            inode.set_size(current_offset);
        }
        inner.offset = current_offset;
        Ok(total_write_size)
    }

    ///get inode from the Dentry of FileInner
    fn get_inode(&self) -> Option<Arc<dyn Inode>> {
        self.get_fileinner().dentry.get_inode()
    }
    /// Do something when the node is opened.
    fn open(&self) -> SyscallResult {
        Ok(0)
    }
    /// Do something when the node is closed.
    fn release(&self) -> SyscallResult {
        Ok(0)
    }
    #[allow(unused)]
    ///chaneg the offset of file
    ///
    fn seek(&self, new_offset: usize) -> SysResult<usize> {
        self.set_offset(new_offset);
        Ok(new_offset)
    }

    fn get_offset(&self) -> usize {
        self.get_fileinner().offset
    }
    fn set_offset(&self, new_offset: usize) {
        self.get_fileinner().offset = new_offset;
    }
    fn get_dentry(&self) -> Arc<dyn Dentry> {
        self.get_fileinner().dentry.clone()
    }

    fn get_stat(&self, stat: &mut Kstat) -> SysResult<()> {
        let inner = self.get_fileinner();
        let inode = inner.dentry.get_inode().ok_or(SysError::EIO)?;
        stat.st_ino = inode.get_ino() as u64;
        stat.st_nlink = inode.get_nlink() as u32;
        stat.st_size = inode.get_size() as i64;
        stat.st_mode = inode.get_mode().bits();
        stat.st_uid = inode.get_uid() as u32;
        stat.st_gid = inode.get_gid() as u32;
        stat.st_blksize = 4096;
        stat.st_blocks = (stat.st_size as u64 + 511) / 512;
        let (atime_sec, atime_nsec) = inode.get_atime();
        let (mtime_sec, mtime_nsec) = inode.get_mtime();
        let (ctime_sec, ctime_nsec) = inode.get_ctime();
        stat.st_atime_sec = atime_sec;
        stat.st_atime_nsec = atime_nsec;
        stat.st_mtime_sec = mtime_sec;
        stat.st_mtime_nsec = mtime_nsec;
        stat.st_ctime_sec = ctime_sec;
        stat.st_ctime_nsec = ctime_nsec;
        Ok(())
    }

    fn truncate(&self, size: u64) -> SyscallResult {
        let inode = self.get_inode().ok_or(SysError::EIO)?;
        inode.set_size(size as usize);
        PAGE_CACHE.lock().remove_inode_pages(inode.get_ino());
        Ok(0)
    }

    fn get_cache_frame(&self, page_id: usize) -> Option<Arc<FrameTracker>> {
        let inner = self.inner.lock();
        let inode = inner.dentry.get_inode()?;
        let ino = inode.get_ino();
        let target_page = self.get_or_alloc_cache_page(ino, page_id);
        Some(target_page.read().frame.clone())
    }
}

impl TempFile {
    /// 获取指定的缓存页，如果 Miss则分配零页
    fn get_or_alloc_cache_page(&self, ino: usize, page_id: usize) -> Arc<RwLock<Page>> {
        {
            let cache = PAGE_CACHE.lock();
            if let Some(page) = cache.get_page(ino, page_id) {
                return page;
            }
        }
        let mut cache_writer = PAGE_CACHE.lock();
        if let Some(page) = cache_writer.get_page(ino, page_id) {
            return page;
        }

        let frame = Arc::new(frame_alloc().expect("tmpfs alloc frame failed"));
        frame.ppn.get_bytes_array().fill(0);
        let page = Arc::new(RwLock::new(Page {
            frame,
            dirty: false,
        }));
        cache_writer.insert_page(ino, page_id, page.clone());
        page
    }
}
