use alloc::boxed::Box;
use alloc::ffi::CString;
use alloc::string::{String, ToString};
use alloc::sync::{Arc, Weak};
use alloc::{format, vec, vec::Vec};
use core::cell::RefMut;
use core::sync::atomic::{AtomicBool, Ordering};

use bitflags::*;
use lazy_static::*;
use log::{info, warn};
use spin::{rwlock::RwLock, Mutex, MutexGuard};

use virtio_drivers::device::blk::VirtIOBlk;
use virtio_drivers::transport::mmio::{MmioTransport, VirtIOHeader};
use virtio_drivers::transport::{DeviceType, Transport};

use lwext4_rust::bindings::{O_APPEND, O_RDONLY, O_RDWR, O_TRUNC, O_WRONLY, SEEK_SET};
use lwext4_rust::{InodeTypes, Lwext4File};

// use crate::config::PAGE_SIZE;
use crate::drivers::block::BLOCK_DEVICE;
use crate::error::{SysError, SysResult, SyscallResult};
use crate::mm::{frame_alloc, UserBuffer};
use polyhal::common::FrameTracker;
use polyhal::consts::PAGE_SIZE;
use polyhal::timer::current_time;

use crate::fs::vfs::{
    dcache::GLOBAL_DCACHE,
    file::{ioctl_get_fs_flags, ioctl_set_fs_flags, File, FS_IOC_GETFLAGS, FS_IOC_SETFLAGS},
    inode::{Inode, InodeMode},
    kstat::Kstat,
    path::{resolve_path, split_parent_and_name},
    Dentry, FileInner, OpenFlags,
};

use crate::fs::lwext4::{dentry::Ext4Dentry, disk::Disk, inode::Ext4Inode};

use crate::fs::get_filesystem;
use crate::fs::page::pagecache::{Page, PAGE_CACHE};

const EXT4_SEQUENTIAL_READAHEAD_PAGES: usize = 8;
const EXT4_STRIDED_READAHEAD_PAGES: usize = 4;
const EXT4_MAX_READAHEAD_STRIDE: usize = 8;
const EXT4_READAHEAD_MIN_STREAK: usize = 2;
const EXT4_HOT_PAGE_CACHE_PAGES: usize = 8;

struct ReadAheadState {
    last_page: Option<usize>,
    last_delta: isize,
    delta_streak: usize,
}

impl ReadAheadState {
    const fn new() -> Self {
        Self {
            last_page: None,
            last_delta: 0,
            delta_streak: 0,
        }
    }
}

///the Ext4File
pub struct Ext4File {
    readable: bool,
    writable: bool,
    append: bool,
    inner: Mutex<FileInner>,
    ///
    pub ext4file: Mutex<Lwext4File>,
    direct_dirty: AtomicBool,
    readahead: Mutex<ReadAheadState>,
    hot_pages: Mutex<Vec<(usize, Arc<RwLock<Page>>)>>,
}

impl Ext4File {
    /// Construct an Ext4File from a Dentry
    pub fn new(
        readable: bool,
        writable: bool,
        dentry: Arc<dyn Dentry>,
        types: InodeTypes,
        flags: OpenFlags,
    ) -> SysResult<Self> {
        let path = dentry.path();
        let mut effective_type = types;
        if effective_type == InodeTypes::EXT4_DE_UNKNOWN {
            if let Ok(c_probe) = CString::new(path.clone()) {
                if crate::fs::lwext4::ext4::dir::ExtDir::open(&c_probe).is_ok() {
                    effective_type = InodeTypes::EXT4_DE_DIR;
                } else {
                    effective_type = InodeTypes::EXT4_DE_REG_FILE;
                }
            }
        }

        let mut file = Lwext4File::new(path.as_str(), effective_type.clone());
        if effective_type != InodeTypes::EXT4_DE_DIR {
            let mut open_flags = match (readable, writable) {
                (true, true) => O_RDWR,
                (false, true) => O_WRONLY,
                _ => O_RDONLY,
            };
            if flags.contains(OpenFlags::O_TRUNC) {
                open_flags |= O_TRUNC;
            }
            if flags.contains(OpenFlags::O_APPEND) {
                open_flags |= O_APPEND;
            }
            file.file_open(path.as_str(), open_flags)
                .map_err(|_| SysError::ENOENT)?;
            // 同步 inode size 到底层 ext4 的实际大小
            if let Some(inode) = dentry.get_inode() {
                let real_size = file.file_desc.fsize as usize;
                inode.set_size(real_size);
            }
        } else {
            info!("Opening a directory: {}, skipping ext4_fopen", path);
        }
        Ok(Self {
            readable,
            writable,
            append: flags.contains(OpenFlags::O_APPEND),
            inner: Mutex::new(FileInner {
                offset: 0,
                dentry,
                flags,
            }),
            ext4file: Mutex::new(file),
            direct_dirty: AtomicBool::new(false),
            readahead: Mutex::new(ReadAheadState::new()),
            hot_pages: Mutex::new(Vec::new()),
        })
    }

    // /// Read all data
    // pub fn read_all(&self) -> Vec<u8> {
    //     let mut inner = self.inner.lock();
    //     let mut buffer = [0u8; 512];
    //     let mut v: Vec<u8> = Vec::new();
    //     loop {
    //         let current_offset = inner.offset;
    //         self
    //             .ext4file
    //             .lock()
    //             .file_seek(current_offset as i64, SEEK_SET)
    //             .expect("seek failed");
    //         let len = self.ext4file.lock().file_read(&mut buffer).unwrap();
    //         if len == 0 {
    //             break;
    //         }
    //         inner.offset += len;
    //         v.extend_from_slice(&buffer[..len]);
    //     }
    //     v
    // }

    #[allow(unused)]
    /// Truncate the inode to the given size
    pub fn ext4_truncate(&self, size: u64) -> SysResult<usize> {
        info!("truncate file to size={}", size);
        self.clear_hot_pages();
        let res = self.ext4file.lock().file_truncate(size);
        if let Err(err) = res {
            return Err(crate::fs::lwext4::lwext4_err_to_sys(err));
        }
        let inner = self.inner.lock();
        if let Some(inode) = inner.dentry.get_inode() {
            inode.set_size(size as usize);
            inode.clear_punched_holes();
        }
        Ok(0)
    }

    /// 从磁盘加载指定页的数据到物理帧中（如果超出文件范围则清零）
    fn load_page_from_disk(&self, page_id: usize, old_size: usize) -> SysResult<Arc<RwLock<Page>>> {
        let new_frame = Arc::new(frame_alloc().ok_or(SysError::ENOMEM)?);
        let page_start_offset = page_id * PAGE_SIZE;
        let bytes = new_frame.ppn.get_bytes_array();
        if page_start_offset < old_size {
            let valid_len = (old_size - page_start_offset).min(PAGE_SIZE);
            let mut ext4file = self.ext4file.lock();
            ext4file
                .file_seek(page_start_offset as i64, SEEK_SET)
                .map_err(crate::fs::lwext4::lwext4_err_to_sys)?;
            let buffer = &mut bytes[..valid_len];
            let read_len = ext4file
                .file_read(buffer)
                .map_err(crate::fs::lwext4::lwext4_err_to_sys)?;
            drop(ext4file);
            if read_len < valid_len {
                bytes[read_len..valid_len].fill(0);
            }
            bytes[valid_len..].fill(0);
        } else {
            bytes.fill(0);
        }
        Ok(Arc::new(RwLock::new(Page {
            frame: new_frame,
            dirty: false,
        })))
    }

    fn get_hot_page(&self, page_id: usize) -> Option<Arc<RwLock<Page>>> {
        let mut hot_pages = self.hot_pages.lock();
        let pos = hot_pages
            .iter()
            .position(|(cached_page_id, _)| *cached_page_id == page_id)?;
        let entry = hot_pages.remove(pos);
        let page = entry.1.clone();
        hot_pages.push(entry);
        Some(page)
    }

    fn remember_hot_page(&self, page_id: usize, page: Arc<RwLock<Page>>) {
        let mut hot_pages = self.hot_pages.lock();
        if let Some(pos) = hot_pages
            .iter()
            .position(|(cached_page_id, _)| *cached_page_id == page_id)
        {
            hot_pages.remove(pos);
        } else if hot_pages.len() >= EXT4_HOT_PAGE_CACHE_PAGES {
            hot_pages.remove(0);
        }
        hot_pages.push((page_id, page));
    }

    fn clear_hot_pages(&self) {
        self.hot_pages.lock().clear();
    }

    /// 获取指定的缓存页，如果 Miss 则自动从磁盘加载并放入缓存
    fn get_or_load_cache_page(
        &self,
        ino: usize,
        page_id: usize,
        old_size: usize,
    ) -> SysResult<(Arc<RwLock<Page>>, bool)> {
        if let Some(page) = self.get_hot_page(page_id) {
            return Ok((page, false));
        }
        {
            let mut cache = PAGE_CACHE.lock();
            if let Some(page) = cache.get_page_touch(ino, page_id) {
                self.remember_hot_page(page_id, page.clone());
                return Ok((page, false));
            }
        }

        let new_page = self.load_page_from_disk(page_id, old_size)?;

        let mut cache_writer = PAGE_CACHE.lock();
        if let Some(page) = cache_writer.get_page_touch(ino, page_id) {
            self.remember_hot_page(page_id, page.clone());
            return Ok((page, false));
        }
        let under_pressure = cache_writer.insert_page(ino, page_id, new_page.clone());
        drop(cache_writer);
        self.remember_hot_page(page_id, new_page.clone());
        if under_pressure {
            crate::fs::writeback::request_writeback();
        }
        Ok((new_page, under_pressure))
    }

    fn get_or_alloc_overwrite_page(
        &self,
        ino: usize,
        page_id: usize,
    ) -> SysResult<(Arc<RwLock<Page>>, bool)> {
        if let Some(page) = self.get_hot_page(page_id) {
            return Ok((page, false));
        }
        {
            let mut cache = PAGE_CACHE.lock();
            if let Some(page) = cache.get_page_touch(ino, page_id) {
                self.remember_hot_page(page_id, page.clone());
                return Ok((page, false));
            }
        }

        let new_frame = Arc::new(frame_alloc().ok_or(SysError::ENOMEM)?);
        let new_page = Arc::new(RwLock::new(Page::new(new_frame)));

        let mut cache_writer = PAGE_CACHE.lock();
        if let Some(page) = cache_writer.get_page_touch(ino, page_id) {
            self.remember_hot_page(page_id, page.clone());
            return Ok((page, false));
        }
        let under_pressure = cache_writer.insert_page(ino, page_id, new_page.clone());
        drop(cache_writer);
        self.remember_hot_page(page_id, new_page.clone());
        if under_pressure {
            crate::fs::writeback::request_writeback();
        }
        Ok((new_page, under_pressure))
    }

    fn prefetch_page_range(
        &self,
        ino: usize,
        start_page: usize,
        file_size: usize,
        page_count: usize,
        reverse: bool,
    ) -> bool {
        if file_size == 0 || page_count == 0 {
            return false;
        }
        let max_page = (file_size + PAGE_SIZE - 1) / PAGE_SIZE;
        let mut under_pressure = false;
        if reverse {
            let mut page_id = start_page.min(max_page.saturating_sub(1));
            for _ in 0..page_count {
                if let Ok((_, pressure)) = self.get_or_load_cache_page(ino, page_id, file_size) {
                    under_pressure |= pressure;
                } else {
                    break;
                }
                if page_id == 0 {
                    break;
                }
                page_id -= 1;
            }
        } else {
            let end_page = start_page.saturating_add(page_count).min(max_page);
            for page_id in start_page..end_page {
                if let Ok((_, pressure)) = self.get_or_load_cache_page(ino, page_id, file_size) {
                    under_pressure |= pressure;
                } else {
                    break;
                }
            }
        }
        under_pressure
    }

    fn page_cached(&self, ino: usize, page_id: usize) -> bool {
        PAGE_CACHE.lock().get_page(ino, page_id).is_some()
    }

    fn prefetch_strided_pages(
        &self,
        ino: usize,
        start_page: usize,
        file_size: usize,
        page_count: usize,
        stride: isize,
    ) -> bool {
        if file_size == 0 || page_count == 0 || stride == 0 {
            return false;
        }
        let max_page = (file_size + PAGE_SIZE - 1) / PAGE_SIZE;
        let mut page_id = start_page as isize;
        let mut under_pressure = false;
        for _ in 0..page_count {
            if page_id < 0 || page_id as usize >= max_page {
                break;
            }
            if let Ok((_, pressure)) = self.get_or_load_cache_page(ino, page_id as usize, file_size)
            {
                under_pressure |= pressure;
            } else {
                break;
            }
            page_id = page_id.saturating_add(stride);
        }
        under_pressure
    }

    fn maybe_readahead_after_page(&self, ino: usize, page_id: usize, file_size: usize) -> bool {
        let delta = {
            let mut state = self.readahead.lock();
            let delta = match state.last_page {
                Some(last_page) if last_page == page_id => return false,
                Some(last_page) => page_id as isize - last_page as isize,
                None => {
                    state.last_page = Some(page_id);
                    return false;
                }
            };
            state.last_page = Some(page_id);
            if delta == state.last_delta {
                state.delta_streak = state.delta_streak.saturating_add(1);
            } else {
                state.last_delta = delta;
                state.delta_streak = 1;
            }
            if state.delta_streak < EXT4_READAHEAD_MIN_STREAK {
                return false;
            }
            delta
        };

        let stride = delta.unsigned_abs();
        if stride == 0 || stride > EXT4_MAX_READAHEAD_STRIDE {
            return false;
        }
        if delta == 1 {
            let start_page = page_id.saturating_add(1);
            if self.page_cached(ino, start_page) {
                return false;
            }
            self.prefetch_page_range(
                ino,
                start_page,
                file_size,
                EXT4_SEQUENTIAL_READAHEAD_PAGES,
                false,
            )
        } else if delta == -1 {
            let Some(start_page) = page_id.checked_sub(1) else {
                return false;
            };
            if self.page_cached(ino, start_page) {
                return false;
            }
            self.prefetch_page_range(
                ino,
                start_page,
                file_size,
                EXT4_SEQUENTIAL_READAHEAD_PAGES,
                true,
            )
        } else {
            let next_page = page_id as isize + delta;
            if next_page < 0 {
                return false;
            }
            let next_page = next_page as usize;
            if self.page_cached(ino, next_page) {
                return false;
            }
            self.prefetch_strided_pages(
                ino,
                next_page,
                file_size,
                EXT4_STRIDED_READAHEAD_PAGES,
                delta,
            )
        }
    }

    fn zero_gap_pages(
        &self,
        inode: &Arc<dyn Inode>,
        ino: usize,
        old_size: usize,
        end: usize,
    ) -> SysResult<bool> {
        let mut current = old_size;
        let mut should_flush_cache = false;
        let zero_page = [0u8; PAGE_SIZE];

        while current < end {
            let page_id = current / PAGE_SIZE;
            let page_offset = current % PAGE_SIZE;
            let zero_len = (PAGE_SIZE - page_offset).min(end - current);
            let overwrites_whole_page = page_offset == 0 && zero_len == PAGE_SIZE;
            let (target_page, under_pressure) = if overwrites_whole_page {
                self.get_or_alloc_overwrite_page(ino, page_id)?
            } else {
                self.get_or_load_cache_page(ino, page_id, old_size)?
            };
            should_flush_cache |= under_pressure;
            {
                let mut page_writer = target_page.write();
                page_writer.modify(page_offset, &zero_page[..zero_len]);
            }
            inode.clear_punched_hole_page(page_id);
            current += zero_len;
        }

        Ok(should_flush_cache)
    }

    fn flush_dirty_pages(&self, max_pages: Option<usize>) -> (usize, bool) {
        if !self.writable() {
            return (0, false);
        }
        let direct_dirty = self.direct_dirty.swap(false, Ordering::AcqRel);
        let inner = self.inner.lock();
        let inode = inner.dentry.get_inode().unwrap();
        let inode_id = inode.cache_inode_id().unwrap_or_else(|| inode.get_ino());
        let file_size = inode.get_size();

        let (dirty_pages, has_more) = {
            let cache = PAGE_CACHE.lock();
            match max_pages {
                Some(limit) => cache.get_inode_dirty_pages_limited(inode_id, limit),
                None => (cache.get_inode_dirty_pages(inode_id), false),
            }
        };
        let mut ext4file = self.ext4file.lock();
        if ext4file.file_desc.fsize < file_size as u64 {
            if let Err(e) = ext4file.file_truncate(file_size as u64) {
                warn!(
                    "file_truncate before flush failed: size={}, err={:?}",
                    file_size, e
                );
                self.direct_dirty.store(direct_dirty, Ordering::Release);
                return (0, has_more);
            }
        }
        if dirty_pages.is_empty() {
            if direct_dirty {
                if let Err(e) = ext4file.file_cache_flush() {
                    self.direct_dirty.store(true, Ordering::Release);
                    warn!("ext4 direct cache flush failed: {:?}", e);
                }
            }
            return (0, false);
        }

        let mut expected_offset: Option<usize> = None;
        let mut flushed = 0usize;

        for (page_id, page_lock) in dirty_pages {
            let mut page = page_lock.write();
            if !page.dirty {
                continue;
            }
            let offset = page_id * PAGE_SIZE;
            if offset >= file_size {
                page.dirty = false;
                continue;
            }
            let write_len = (file_size - offset).min(PAGE_SIZE);
            if expected_offset != Some(offset) {
                ext4file.file_seek(offset as i64, SEEK_SET).unwrap();
            }
            let buffer = &page.frame.ppn.get_bytes_array()[..write_len];
            ext4file.file_write(buffer).unwrap();
            expected_offset = Some(offset + write_len);
            page.dirty = false;
            flushed += 1;
        }

        if let Err(e) = ext4file.file_cache_flush() {
            self.direct_dirty.store(true, Ordering::Release);
            warn!("ext4 cache flush failed: {:?}", e);
        }
        (flushed, has_more)
    }
}

impl File for Ext4File {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        self.inner.lock()
    }
    fn seek_position(&self, offset: isize, whence: i32) -> SysResult<usize> {
        const SEEK_SET: i32 = 0;
        const SEEK_CUR: i32 = 1;
        const SEEK_END: i32 = 2;

        let mut inner = self.inner.lock();
        let inode = inner.dentry.get_inode().ok_or(SysError::ESPIPE)?;
        let new_off = match whence {
            SEEK_SET => offset,
            SEEK_CUR => (inner.offset as isize).saturating_add(offset),
            SEEK_END => {
                if inode.get_mode().get_type() == InodeMode::DIR {
                    return Err(SysError::EINVAL);
                }
                (inode.get_size() as isize).saturating_add(offset)
            }
            _ => return Err(SysError::EINVAL),
        };
        if new_off < 0 {
            return Err(SysError::EINVAL);
        }
        inner.offset = new_off as usize;
        Ok(new_off as usize)
    }
    fn readable(&self) -> bool {
        self.readable
    }
    fn writable(&self) -> bool {
        self.writable
    }
    fn is_append(&self) -> bool {
        self.append
    }
    fn supports_sparse_holes(&self) -> bool {
        true
    }
    fn read_all(&self) -> Vec<u8> {
        let size = self
            .inner
            .lock()
            .dentry
            .get_inode()
            .map(|inode| inode.get_size())
            .unwrap_or(0);
        let mut data = vec![0u8; size];
        if size == 0 {
            return data;
        }

        let mut ext4file = self.ext4file.lock();
        if ext4file.file_seek(0, SEEK_SET).is_err() {
            return Vec::new();
        }
        let mut offset = 0usize;
        while offset < size {
            match ext4file.file_read(&mut data[offset..]) {
                Ok(0) => break,
                Ok(n) => offset += n,
                Err(_) => break,
            }
        }
        data.truncate(offset);
        data
    }
    //read the data
    fn read(&self, mut buf: UserBuffer) -> SysResult<usize> {
        let mut inner = self.get_fileinner();
        let inode = inner.dentry.get_inode().unwrap();
        let should_update_atime = !inner.flags.contains(OpenFlags::O_NOATIME)
            && buf.buffers.iter().any(|slice| !slice.is_empty());
        let ino = inode.cache_inode_id().unwrap_or_else(|| inode.get_ino());
        // 使用 inode 中缓存的大小，而不是 ext4 文件描述符中的大小
        // 因为 ext4 文件描述符的 fsize 可能没有及时更新
        let file_size = inode.get_size();
        let mut current_offset = inner.offset;
        let mut total_read_size = 0usize;
        let mut should_flush_cache = false;
        if current_offset >= file_size {
            return Ok(0);
        }
        for slice in buf.buffers.iter_mut() {
            let mut slice_offset = 0;
            let slice_len = slice.len();
            while slice_offset < slice_len && current_offset < file_size {
                let page_id = current_offset / PAGE_SIZE;
                let page_offset = current_offset % PAGE_SIZE;
                let left_in_page = PAGE_SIZE - page_offset;
                let left_in_slice = slice_len - slice_offset;
                let left_in_file = file_size - current_offset;
                let read_bytes = left_in_page.min(left_in_slice).min(left_in_file);
                if inode.is_punched_hole_page(page_id) {
                    slice[slice_offset..slice_offset + read_bytes].fill(0);
                    current_offset += read_bytes;
                    slice_offset += read_bytes;
                    total_read_size += read_bytes;
                    continue;
                }
                let (target_page, under_pressure) =
                    self.get_or_load_cache_page(ino, page_id, file_size)?;
                should_flush_cache |= under_pressure && self.writable();
                {
                    let page_reader = target_page.read();
                    let src_data = &page_reader.frame.ppn.get_bytes_array()
                        [page_offset..page_offset + read_bytes];
                    slice[slice_offset..slice_offset + read_bytes].copy_from_slice(src_data);

                    current_offset += read_bytes;
                    slice_offset += read_bytes;
                    total_read_size += read_bytes;
                }
                should_flush_cache |=
                    self.maybe_readahead_after_page(ino, page_id, file_size) && self.writable();
            }
        }
        inner.offset = current_offset;
        if should_update_atime && total_read_size > 0 {
            crate::syscall::maybe_update_atime_for_dentry(&inner.dentry, &inode, false);
        }
        drop(inner);
        if should_flush_cache {
            crate::fs::writeback::request_writeback();
        }
        Ok(total_read_size)
    }

    fn read_at(&self, offset: usize, mut buf: UserBuffer) -> SysResult<usize> {
        let (inode, should_update_atime, dentry) = {
            let inner = self.get_fileinner();
            let inode = inner.dentry.get_inode().unwrap();
            let should_update_atime = !inner.flags.contains(OpenFlags::O_NOATIME)
                && buf.buffers.iter().any(|slice| !slice.is_empty());
            let dentry = if should_update_atime {
                Some(inner.dentry.clone())
            } else {
                None
            };
            (inode, should_update_atime, dentry)
        };
        let ino = inode.cache_inode_id().unwrap_or_else(|| inode.get_ino());
        let file_size = inode.get_size();
        let mut current_offset = offset;
        let mut total_read_size = 0usize;
        let mut should_flush_cache = false;
        if current_offset >= file_size {
            return Ok(0);
        }
        for slice in buf.buffers.iter_mut() {
            let mut slice_offset = 0;
            let slice_len = slice.len();
            while slice_offset < slice_len && current_offset < file_size {
                let page_id = current_offset / PAGE_SIZE;
                let page_offset = current_offset % PAGE_SIZE;
                let left_in_page = PAGE_SIZE - page_offset;
                let left_in_slice = slice_len - slice_offset;
                let left_in_file = file_size - current_offset;
                let read_bytes = left_in_page.min(left_in_slice).min(left_in_file);
                if inode.is_punched_hole_page(page_id) {
                    slice[slice_offset..slice_offset + read_bytes].fill(0);
                    current_offset += read_bytes;
                    slice_offset += read_bytes;
                    total_read_size += read_bytes;
                    continue;
                }
                let (target_page, under_pressure) =
                    self.get_or_load_cache_page(ino, page_id, file_size)?;
                should_flush_cache |= under_pressure && self.writable();
                {
                    let page_reader = target_page.read();
                    let src_data = &page_reader.frame.ppn.get_bytes_array()
                        [page_offset..page_offset + read_bytes];
                    slice[slice_offset..slice_offset + read_bytes].copy_from_slice(src_data);

                    current_offset += read_bytes;
                    slice_offset += read_bytes;
                    total_read_size += read_bytes;
                }
                should_flush_cache |=
                    self.maybe_readahead_after_page(ino, page_id, file_size) && self.writable();
            }
        }
        if should_update_atime && total_read_size > 0 {
            if let Some(dentry) = dentry {
                crate::syscall::maybe_update_atime_for_dentry(&dentry, &inode, false);
            }
        }
        if should_flush_cache {
            crate::fs::writeback::request_writeback();
        }
        Ok(total_read_size)
    }

    fn read_at_direct(&self, offset: usize, buf: &mut [u8]) -> SysResult<usize> {
        if !self.readable() {
            return Err(SysError::EBADF);
        }
        if buf.is_empty() {
            return Ok(0);
        }
        let inode = self.get_inode().ok_or(SysError::EIO)?;
        let file_size = inode.get_size();
        if offset >= file_size {
            return Ok(0);
        }
        let mut done = 0usize;
        let total_len = (file_size - offset).min(buf.len());
        while done < total_len {
            let pos = offset + done;
            let page_id = pos / PAGE_SIZE;
            let page_offset = pos % PAGE_SIZE;
            let read_len = (PAGE_SIZE - page_offset).min(total_len - done);
            if inode.is_punched_hole_page(page_id) {
                buf[done..done + read_len].fill(0);
                done += read_len;
                continue;
            }
            let mut ext4file = self.ext4file.lock();
            ext4file
                .file_seek(pos as i64, SEEK_SET)
                .map_err(crate::fs::lwext4::lwext4_err_to_sys)?;
            let n = ext4file
                .file_read(&mut buf[done..done + read_len])
                .map_err(crate::fs::lwext4::lwext4_err_to_sys)?;
            drop(ext4file);
            if n == 0 {
                break;
            }
            done += n;
            if n < read_len {
                break;
            }
        }
        Ok(done)
    }

    fn write(&self, buf: UserBuffer) -> SysResult<usize> {
        // info!("enter VFS Write-back Cache");
        let mut inner = self.inner.lock();
        let inode = inner.dentry.get_inode().unwrap();
        if inode.get_fs_flags()
            & (crate::fs::vfs::inode::FS_IMMUTABLE_FL | crate::fs::vfs::inode::FS_APPEND_FL)
            != 0
        {
            return Err(SysError::EPERM);
        }
        let ino = inode.cache_inode_id().unwrap_or_else(|| inode.get_ino());
        // println!("[DEBUG] 当前操作的 ino: {}", ino);
        let old_size = inode.get_size();
        let mut total_write_size = 0usize;
        let mut current_offset = inner.offset;
        let mut should_flush_cache = false;
        if current_offset > old_size {
            should_flush_cache |= self.zero_gap_pages(&inode, ino, old_size, current_offset)?;
        }
        for slice in buf.buffers.iter() {
            let mut slice_offset = 0;
            let slice_len = slice.len();
            while slice_offset < slice_len {
                let page_id = current_offset / PAGE_SIZE;
                let page_offset = current_offset % PAGE_SIZE;
                let write_bytes = (PAGE_SIZE - page_offset).min(slice_len - slice_offset);
                let overwrites_whole_page = page_offset == 0 && write_bytes == PAGE_SIZE;
                let page_was_hole = inode.is_punched_hole_page(page_id);
                let (target_page, under_pressure) = if overwrites_whole_page || page_was_hole {
                    self.get_or_alloc_overwrite_page(ino, page_id)?
                } else {
                    self.get_or_load_cache_page(ino, page_id, old_size)?
                };
                inode.clear_punched_hole_page(page_id);
                should_flush_cache |= under_pressure;
                // 写入数据并标记脏页
                {
                    let mut page_writer = target_page.write();
                    if page_was_hole && !overwrites_whole_page {
                        page_writer.frame.ppn.get_bytes_array().fill(0);
                    }
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
        let now_us = current_time().as_micros() as i64;
        let now_sec = now_us / 1_000_000;
        let now_nsec = (now_us % 1_000_000) * 1000;
        inode.set_mtime(now_sec, now_nsec);
        inode.set_ctime(now_sec, now_nsec);
        inner.offset = current_offset;
        drop(inner);
        if should_flush_cache {
            crate::fs::writeback::request_writeback();
        }
        Ok(total_write_size)
    }

    fn write_at_direct(&self, offset: usize, buf: &[u8]) -> SysResult<usize> {
        if !self.writable() {
            return Err(SysError::EBADF);
        }
        if buf.is_empty() {
            return Ok(0);
        }
        self.clear_hot_pages();
        let inode = self.get_inode().ok_or(SysError::EIO)?;
        if inode.get_fs_flags()
            & (crate::fs::vfs::inode::FS_IMMUTABLE_FL | crate::fs::vfs::inode::FS_APPEND_FL)
            != 0
        {
            return Err(SysError::EPERM);
        }
        let mut written = 0usize;
        while written < buf.len() {
            let pos = offset + written;
            let page_id = pos / PAGE_SIZE;
            let page_offset = pos % PAGE_SIZE;
            let write_len = (PAGE_SIZE - page_offset).min(buf.len() - written);
            let overwrites_whole_page = page_offset == 0 && write_len == PAGE_SIZE;
            let page_was_hole = inode.is_punched_hole_page(page_id);

            if page_was_hole && !overwrites_whole_page {
                let mut page = [0u8; PAGE_SIZE];
                page[page_offset..page_offset + write_len]
                    .copy_from_slice(&buf[written..written + write_len]);
                let mut ext4file = self.ext4file.lock();
                ext4file
                    .file_seek((page_id * PAGE_SIZE) as i64, SEEK_SET)
                    .map_err(crate::fs::lwext4::lwext4_err_to_sys)?;
                let n = ext4file
                    .file_write(&page)
                    .map_err(crate::fs::lwext4::lwext4_err_to_sys)?;
                drop(ext4file);
                if n != PAGE_SIZE {
                    if written > 0 {
                        return Ok(written);
                    }
                    return Err(SysError::EIO);
                }
                inode.clear_punched_hole_page(page_id);
                written += write_len;
            } else {
                let mut ext4file = self.ext4file.lock();
                ext4file
                    .file_seek(pos as i64, SEEK_SET)
                    .map_err(crate::fs::lwext4::lwext4_err_to_sys)?;
                let n = ext4file
                    .file_write(&buf[written..written + write_len])
                    .map_err(crate::fs::lwext4::lwext4_err_to_sys)?;
                drop(ext4file);
                if n == 0 {
                    break;
                }
                inode.clear_punched_hole_page(page_id);
                written += n;
            }
        }
        if written > 0 {
            let end = offset + written;
            if end > inode.get_size() {
                inode.set_size(end);
            }
            let now_us = current_time().as_micros() as i64;
            let now_sec = now_us / 1_000_000;
            let now_nsec = (now_us % 1_000_000) * 1000;
            inode.set_mtime(now_sec, now_nsec);
            inode.set_ctime(now_sec, now_nsec);
            self.direct_dirty.store(true, Ordering::Release);
        }
        Ok(written)
    }

    fn ls(&self) -> Vec<(String, u64, u8)> {
        self.get_fileinner().dentry.ls()
    }

    fn get_stat(&self, stat: &mut Kstat) -> SysResult<()> {
        let inner_lock = self.inner.lock();
        let inode = inner_lock.dentry.get_inode().unwrap();

        stat.st_ino = inode.get_ino() as u64;
        stat.st_nlink = inode.get_nlink() as u32;
        stat.st_size = inode.get_size() as i64;
        stat.st_mode = inode.get_mode().bits();
        stat.st_uid = inode.get_uid() as u32;
        stat.st_gid = inode.get_gid() as u32;
        stat.st_rdev = inode.get_rdev() as u64;
        stat.st_blksize = 512;
        stat.st_blocks = ((stat.st_size as u64 + 511) / 512)
            .saturating_sub(inode.get_punched_hole_pages() as u64 * 8);
        stat.st_fs_flags = inode.get_fs_flags();

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

    ///
    fn flush(&self) {
        info!("enter VFS flush (write-back to disk)");
        self.flush_dirty_pages(None);
        info!("finish VFS flush");
    }

    fn has_private_writeback_state(&self) -> bool {
        self.direct_dirty.load(Ordering::Acquire)
    }

    fn flush_pages(&self, max_pages: usize) -> (usize, bool) {
        self.flush_dirty_pages(Some(max_pages))
    }

    fn ioctl(&self, request: usize, argp: usize) -> SyscallResult {
        let inode = self.get_inode().ok_or(SysError::EIO)?;
        match request {
            FS_IOC_GETFLAGS => ioctl_get_fs_flags(inode, argp),
            FS_IOC_SETFLAGS => ioctl_set_fs_flags(inode, argp),
            _ => Err(SysError::ENOTTY),
        }
    }

    fn truncate(&self, size: u64) -> SyscallResult {
        self.clear_hot_pages();
        if let Some(inode) = self.get_inode() {
            if inode.get_fs_flags()
                & (crate::fs::vfs::inode::FS_IMMUTABLE_FL | crate::fs::vfs::inode::FS_APPEND_FL)
                != 0
            {
                return Err(SysError::EPERM);
            }
        }
        let res = self.ext4file.lock().file_truncate(size);
        if let Err(err) = res {
            return Err(crate::fs::lwext4::lwext4_err_to_sys(err));
        }
        let inner = self.inner.lock();
        if let Some(inode) = inner.dentry.get_inode() {
            inode.set_size(size as usize);
            inode.clear_punched_holes();
        }
        Ok(0)
    }

    fn get_cache_frame(&self, page_id: usize) -> Option<Arc<FrameTracker>> {
        let inner = self.inner.lock();
        let inode = inner.dentry.get_inode().unwrap();
        let ino = inode.cache_inode_id().unwrap_or_else(|| inode.get_ino());
        let file_size = inode.get_size();
        let (target_page, under_pressure) =
            self.get_or_load_cache_page(ino, page_id, file_size).ok()?;
        drop(inner);
        if under_pressure && self.writable() {
            crate::fs::writeback::request_writeback();
        }
        Some(target_page.read().frame.clone())
    }

    fn populate_page_cache(&self, offset: usize, len: usize) -> SysResult<usize> {
        if len == 0 {
            return Ok(0);
        }
        let inode = self.get_inode().ok_or(SysError::EIO)?;
        let file_size = inode.get_size();
        if offset >= file_size {
            return Ok(0);
        }
        let ino = inode.cache_inode_id().unwrap_or_else(|| inode.get_ino());
        let end = offset.saturating_add(len).min(file_size);
        let start_page = offset / PAGE_SIZE;
        let end_page = (end + PAGE_SIZE - 1) / PAGE_SIZE;
        let mut should_flush_cache = false;
        for page_id in start_page..end_page {
            if inode.is_punched_hole_page(page_id) {
                continue;
            }
            let (_, under_pressure) = self.get_or_load_cache_page(ino, page_id, file_size)?;
            should_flush_cache |= under_pressure && self.writable();
        }
        if should_flush_cache {
            crate::fs::writeback::request_writeback();
        }
        Ok(end - offset)
    }
}

impl OpenFlags {
    /// Convert OpenFlags to ext4 open flags (O_RDONLY, O_WRONLY, O_RDWR)
    pub fn into_ext4_flags(&self) -> u32 {
        match self.bits() & 0o3 {
            0o1 => O_WRONLY,
            0o2 => O_RDWR,
            _ => O_RDONLY,
        }
    }
}
