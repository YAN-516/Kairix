use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::fat32::fat32_error_to_sys;
use crate::fs::fat32::superblock::Fat32SuperBlock;
use crate::fs::page::pagecache::{Page, PAGE_CACHE};
use crate::fs::vfs::file::File;
use crate::fs::vfs::FileInner;
use crate::fs::vfs::OpenFlags;
use crate::mm::UserBuffer;
use crate::mm::frame_alloc;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use fatfs::{Read, Seek, SeekFrom, Write};
use polyhal::common::FrameTracker;
use polyhal::consts::PAGE_SIZE;
use spin::mutex::Mutex;
use spin::mutex::MutexGuard;
use spin::rwlock::RwLock;

pub struct Fat32File {
    readable: bool,
    writable: bool,
    inner: Mutex<FileInner>,
    rel_path: String,
    superblock: Weak<Fat32SuperBlock>,
}

impl Fat32File {
    pub fn new(
        readable: bool,
        writable: bool,
        dentry: Arc<dyn crate::fs::Dentry>,
        rel_path: String,
        superblock: Weak<Fat32SuperBlock>,
    ) -> Self {
        Self {
            readable,
            writable,
            inner: Mutex::new(FileInner { offset: 0, dentry, flags: OpenFlags::empty() }),
            rel_path,
            superblock,
        }
    }

    fn load_page_from_disk(&self, page_id: usize, old_size: usize) -> Arc<RwLock<Page>> {
        let new_frame = Arc::new(frame_alloc().unwrap());
        let page_start_offset = page_id * PAGE_SIZE;
        let bytes = new_frame.ppn.get_bytes_array();
        if page_start_offset < old_size {
            let valid_len = (old_size - page_start_offset).min(PAGE_SIZE);
            let sb = self.superblock.upgrade().expect("fat32 sb dropped");
            let fs = sb.fs.lock();
            let root = fs.root_dir();
            let mut fat_file = root.open_file(&self.rel_path).unwrap();
            fat_file
                .seek(SeekFrom::Start(page_start_offset as u64))
                .unwrap();
            let buffer = &mut bytes[..valid_len];
            let read_len = fat_file.read(buffer).unwrap();
            drop(fat_file);
            assert_eq!(read_len, valid_len);
            bytes[valid_len..].fill(0);
        } else {
            bytes.fill(0);
        }
        Arc::new(RwLock::new(Page {
            frame: new_frame,
            dirty: false,
        }))
    }

    fn get_or_load_cache_page(
        &self,
        ino: usize,
        page_id: usize,
        old_size: usize,
    ) -> Arc<RwLock<Page>> {
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
        let new_page = self.load_page_from_disk(page_id, old_size);
        cache_writer.insert_page(ino, page_id, new_page.clone());
        new_page
    }
}

impl File for Fat32File {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        self.inner.lock()
    }

    fn readable(&self) -> bool {
        self.readable
    }

    fn writable(&self) -> bool {
        self.writable
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
            let user_buffer = UserBuffer::new(alloc::vec![static_buf]);
            match self.read(user_buffer) {
                Ok(0) => break,
                Ok(read_len) => v.extend_from_slice(&buffer[..read_len]),
                Err(_) => break,
            }
        }
        self.inner.lock().offset = old_offset;
        v
    }

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
                let target_page =
                    self.get_or_load_cache_page(ino, current_offset / PAGE_SIZE, file_size);
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
        let mut inner = self.inner.lock();
        let inode = inner.dentry.get_inode().ok_or(SysError::EIO)?;
        let ino = inode.get_ino();
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
                let target_page = self.get_or_load_cache_page(ino, page_id, old_size);
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

    fn truncate(&self, size: u64) -> SyscallResult {
        let sb = self.superblock.upgrade().ok_or(SysError::EIO)?;
        {
            let fs = sb.fs.lock();
            let root = fs.root_dir();
            let mut fat_file = root.open_file(&self.rel_path).map_err(fat32_error_to_sys)?;
            fat_file
                .seek(SeekFrom::Start(size))
                .map_err(|_| SysError::EIO)?;
            fat_file.truncate().map_err(|_| SysError::EIO)?;
        }
        if let Some(inode) = self.get_inode() {
            inode.set_size(size as usize);
            PAGE_CACHE.lock().remove_inode_pages(inode.get_ino());
        }
        Ok(0)
    }

    fn flush(&self) {
        if !self.writable() {
            return;
        }
        let inner = self.inner.lock();
        let inode = inner.dentry.get_inode().unwrap();
        let inode_id = inode.get_ino();
        let file_size = inode.get_size();
        let max_page_id = (file_size + PAGE_SIZE - 1) / PAGE_SIZE;
        let mut dirty_pages: Vec<(usize, Arc<RwLock<Page>>)> = Vec::new();
        {
            let cache = PAGE_CACHE.lock();
            for page_id in 0..max_page_id {
                if let Some(page_lock) = cache.get_page(inode_id, page_id) {
                    if page_lock.read().dirty {
                        dirty_pages.push((page_id, page_lock.clone()));
                    }
                }
            }
        }
        for (page_id, page_lock) in dirty_pages {
            let mut page = page_lock.write();
            if page.dirty {
                let offset = page_id * PAGE_SIZE;
                let write_len = if offset + PAGE_SIZE > file_size {
                    file_size - offset
                } else {
                    PAGE_SIZE
                };
                let sb = self.superblock.upgrade().expect("fat32 sb dropped");
                let fs = sb.fs.lock();
                let root = fs.root_dir();
                let mut fat_file = root.open_file(&self.rel_path).unwrap();
                fat_file
                    .seek(SeekFrom::Start(offset as u64))
                    .unwrap();
                let buffer = &page.frame.ppn.get_bytes_array()[..write_len];
                fat_file.write_all(buffer).unwrap();
                drop(fat_file);
                page.dirty = false;
            }
        }
    }

    fn get_cache_frame(&self, page_id: usize) -> Option<Arc<FrameTracker>> {
        let inner = self.inner.lock();
        let inode = inner.dentry.get_inode()?;
        let ino = inode.get_ino();
        let file_size = inode.get_size();
        let target_page = self.get_or_load_cache_page(ino, page_id, file_size);
        Some(target_page.read().frame.clone())
    }

    fn ioctl(&self, _request: usize, _argp: usize) -> SyscallResult {
        Err(SysError::ENOTTY)
    }
}
