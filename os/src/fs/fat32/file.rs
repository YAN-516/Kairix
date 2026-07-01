use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::fat32::fat32_error_to_sys;
use crate::fs::fat32::superblock::Fat32SuperBlock;
use crate::fs::page::pagecache::{PAGE_CACHE, PAGE_CACHE_FS_FAT32, Page, tagged_inode_id};
use crate::fs::vfs::Dentry;
use crate::fs::vfs::FileInner;
use crate::fs::vfs::Inode;
use crate::fs::vfs::OpenFlags;
use crate::fs::vfs::file::{
    FS_IOC_GETFLAGS, FS_IOC_SETFLAGS, File, ioctl_get_fs_flags, ioctl_set_fs_flags,
};
use crate::fs::vfs::inode::{FS_APPEND_FL, FS_IMMUTABLE_FL, InodeMode};
use crate::fs::vfs::kstat::Kstat;
use crate::mm::UserBuffer;
use crate::mm::frame_alloc;
use alloc::string::{String, ToString};
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use fatfs::{Read, Seek, SeekFrom, Write};
use polyhal::common::FrameTracker;
use polyhal::consts::PAGE_SIZE;
use polyhal::timer::current_time;
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

fn fat32_rel_path_for_abs(sb: &Fat32SuperBlock, abs_path: &str) -> Option<String> {
    let mount = sb.mount_point.trim_end_matches('/');
    if mount.is_empty() || mount == "/" {
        return Some(abs_path.trim_start_matches('/').to_string());
    }
    if abs_path == mount {
        return Some(String::new());
    }
    abs_path
        .strip_prefix(mount)
        .and_then(|rest| rest.strip_prefix('/'))
        .map(ToString::to_string)
}

fn find_dentry_by_ino(dentry: Arc<dyn Dentry>, ino: usize) -> Option<Arc<dyn Dentry>> {
    if dentry
        .get_inode()
        .is_some_and(|inode| inode.get_ino() == ino)
    {
        return Some(dentry);
    }
    for child in dentry.children().values() {
        if let Some(found) = find_dentry_by_ino(child.clone(), ino) {
            return Some(found);
        }
    }
    None
}

fn touch_modified_inode(inode: &Arc<dyn Inode>) {
    let now_us = current_time().as_micros() as i64;
    let now_sec = now_us / 1_000_000;
    let now_nsec = (now_us % 1_000_000) * 1000;
    inode.set_mtime(now_sec, now_nsec);
    inode.set_ctime(now_sec, now_nsec);
}

impl Fat32File {
    pub fn new(
        readable: bool,
        writable: bool,
        dentry: Arc<dyn crate::fs::Dentry>,
        rel_path: String,
        superblock: Weak<Fat32SuperBlock>,
        flags: OpenFlags,
    ) -> Self {
        Self {
            readable,
            writable,
            inner: Mutex::new(FileInner {
                offset: 0,
                dentry,
                flags,
            }),
            rel_path,
            superblock,
        }
    }

    fn current_rel_path(&self, sb: &Fat32SuperBlock, ino: usize) -> String {
        let root = { sb.inner.root.lock().as_ref().cloned() };
        if let Some(root) = root {
            if let Some(dentry) = find_dentry_by_ino(root, ino) {
                if let Some(rel_path) = fat32_rel_path_for_abs(sb, &dentry.path()) {
                    return rel_path;
                }
            }
        }
        self.rel_path.clone()
    }

    fn load_page_from_disk(
        &self,
        inode_ino: usize,
        page_id: usize,
        old_size: usize,
    ) -> SysResult<Arc<RwLock<Page>>> {
        let new_frame = Arc::new(frame_alloc().ok_or(SysError::ENOMEM)?);
        let page_start_offset = page_id * PAGE_SIZE;
        let bytes = new_frame.ppn.get_bytes_array();
        if page_start_offset < old_size {
            let valid_len = (old_size - page_start_offset).min(PAGE_SIZE);
            let sb = self.superblock.upgrade().ok_or(SysError::EIO)?;
            let rel_path = self.current_rel_path(&sb, inode_ino);
            let fs = sb.fs.lock();
            let root = fs.root_dir();
            let mut fat_file = root.open_file(&rel_path).map_err(fat32_error_to_sys)?;
            fat_file
                .seek(SeekFrom::Start(page_start_offset as u64))
                .map_err(fat32_error_to_sys)?;
            let buffer = &mut bytes[..valid_len];
            let read_len = fat_file.read(buffer).map_err(fat32_error_to_sys)?;
            drop(fat_file);
            if read_len < valid_len {
                bytes[read_len..valid_len].fill(0);
            }
            bytes[valid_len..].fill(0);
        } else {
            bytes.fill(0);
        }
        Ok(Arc::new(RwLock::new(Page::new(new_frame))))
    }

    fn get_or_load_cache_page(
        &self,
        inode_ino: usize,
        page_id: usize,
        old_size: usize,
    ) -> SysResult<(Arc<RwLock<Page>>, bool)> {
        let cache_inode_id = tagged_inode_id(PAGE_CACHE_FS_FAT32, inode_ino);
        {
            let mut cache = PAGE_CACHE.lock();
            if let Some(page) = cache.get_page_touch(cache_inode_id, page_id) {
                return Ok((page, false));
            }
        }

        let new_page = self.load_page_from_disk(inode_ino, page_id, old_size)?;

        let mut cache_writer = PAGE_CACHE.lock();
        if let Some(page) = cache_writer.get_page_touch(cache_inode_id, page_id) {
            return Ok((page, false));
        }
        let under_pressure = cache_writer.insert_page(cache_inode_id, page_id, new_page.clone());
        drop(cache_writer);
        if under_pressure {
            crate::fs::writeback::request_writeback();
        }
        Ok((new_page, under_pressure))
    }

    fn get_or_alloc_overwrite_page(
        &self,
        inode_ino: usize,
        page_id: usize,
    ) -> SysResult<(Arc<RwLock<Page>>, bool)> {
        let cache_inode_id = tagged_inode_id(PAGE_CACHE_FS_FAT32, inode_ino);
        {
            let mut cache = PAGE_CACHE.lock();
            if let Some(page) = cache.get_page_touch(cache_inode_id, page_id) {
                return Ok((page, false));
            }
        }

        let new_frame = Arc::new(frame_alloc().ok_or(SysError::ENOMEM)?);
        let new_page = Arc::new(RwLock::new(Page::new(new_frame)));

        let mut cache_writer = PAGE_CACHE.lock();
        if let Some(page) = cache_writer.get_page_touch(cache_inode_id, page_id) {
            return Ok((page, false));
        }
        let under_pressure = cache_writer.insert_page(cache_inode_id, page_id, new_page.clone());
        drop(cache_writer);
        if under_pressure {
            crate::fs::writeback::request_writeback();
        }
        Ok((new_page, under_pressure))
    }

    fn flush_dirty_pages(&self, max_pages: Option<usize>) -> (usize, bool) {
        if !self.writable() {
            return (0, false);
        }
        let inner = self.inner.lock();
        let Some(inode) = inner.dentry.get_inode() else {
            return (0, false);
        };
        let inode_id = tagged_inode_id(PAGE_CACHE_FS_FAT32, inode.get_ino());
        let file_size = inode.get_size();

        let (dirty_pages, has_more) = {
            let cache = PAGE_CACHE.lock();
            match max_pages {
                Some(limit) => cache.get_inode_dirty_pages_limited(inode_id, limit),
                None => (cache.get_inode_dirty_pages(inode_id), false),
            }
        };
        if dirty_pages.is_empty() {
            return (0, false);
        }

        let Some(sb) = self.superblock.upgrade() else {
            return (0, false);
        };
        let rel_path = self.current_rel_path(&sb, inode.get_ino());
        drop(inner);
        let fs = sb.fs.lock();
        let root = fs.root_dir();
        let Ok(mut fat_file) = root.open_file(&rel_path) else {
            return (0, false);
        };
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
                if fat_file.seek(SeekFrom::Start(offset as u64)).is_err() {
                    continue;
                }
            }
            let Some(frame) = page.resident_frame() else {
                continue;
            };
            let buffer = &frame.ppn.get_bytes_array()[..write_len];
            if fat_file.write_all(buffer).is_err() {
                continue;
            }
            expected_offset = Some(offset + write_len);
            page.dirty = false;
            flushed += 1;
        }
        drop(fat_file);
        (flushed, has_more)
    }
}

impl File for Fat32File {
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
        self.inner.lock().flags.contains(OpenFlags::O_APPEND)
    }

    fn ls(&self) -> Vec<(String, u64, u8)> {
        self.inner.lock().dentry.ls()
    }

    fn read_all(&self) -> Vec<u8> {
        let Some(inode) = self.inner.lock().dentry.get_inode() else {
            return Vec::new();
        };
        let size = inode.get_size();
        let mut data = alloc::vec![0u8; size];
        if size == 0 {
            return data;
        }

        let Some(sb) = self.superblock.upgrade() else {
            return Vec::new();
        };
        let rel_path = self.current_rel_path(&sb, inode.get_ino());
        let fs = sb.fs.lock();
        let root = fs.root_dir();
        let Ok(mut fat_file) = root.open_file(&rel_path) else {
            return Vec::new();
        };
        if fat_file.seek(SeekFrom::Start(0)).is_err() {
            return Vec::new();
        }
        let mut offset = 0usize;
        while offset < size {
            match fat_file.read(&mut data[offset..]) {
                Ok(0) => break,
                Ok(n) => offset += n,
                Err(_) => break,
            }
        }
        data.truncate(offset);
        data
    }

    fn read(&self, mut buf: UserBuffer) -> SysResult<usize> {
        let mut inner = self.get_fileinner();
        let inode = inner.dentry.get_inode().ok_or(SysError::EIO)?;
        let should_update_atime = !inner.flags.contains(OpenFlags::O_NOATIME)
            && buf.buffers.iter().any(|slice| !slice.is_empty());
        let ino = inode.get_ino();
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
                let (target_page, under_pressure) =
                    self.get_or_load_cache_page(ino, current_offset / PAGE_SIZE, file_size)?;
                should_flush_cache |= under_pressure && self.writable();
                {
                    let page_reader = target_page.read();
                    let page_offset = current_offset % PAGE_SIZE;
                    let left_in_page = PAGE_SIZE - page_offset;
                    let left_in_slice = slice_len - slice_offset;
                    let left_in_file = file_size - current_offset;
                    let read_bytes = left_in_page.min(left_in_slice).min(left_in_file);
                    let frame = page_reader.resident_frame().ok_or(SysError::EIO)?;
                    let src_data =
                        &frame.ppn.get_bytes_array()[page_offset..page_offset + read_bytes];
                    slice[slice_offset..slice_offset + read_bytes].copy_from_slice(src_data);

                    current_offset += read_bytes;
                    slice_offset += read_bytes;
                    total_read_size += read_bytes;
                }
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

    fn write(&self, buf: UserBuffer) -> SysResult<usize> {
        let mut inner = self.inner.lock();
        let inode = inner.dentry.get_inode().ok_or(SysError::EIO)?;
        if inode.get_fs_flags() & (FS_IMMUTABLE_FL | FS_APPEND_FL) != 0 {
            return Err(SysError::EPERM);
        }
        let ino = inode.get_ino();
        let old_size = inode.get_size();
        let mut total_write_size = 0usize;
        let mut current_offset = if inner.flags.contains(OpenFlags::O_APPEND) {
            old_size
        } else {
            inner.offset
        };
        let mut should_flush_cache = false;
        for slice in buf.buffers.iter() {
            let mut slice_offset = 0;
            let slice_len = slice.len();
            while slice_offset < slice_len {
                let page_id = current_offset / PAGE_SIZE;
                let page_offset = current_offset % PAGE_SIZE;
                let write_bytes = (PAGE_SIZE - page_offset).min(slice_len - slice_offset);
                inode.clear_punched_hole_page(page_id);
                let overwrites_whole_page = page_offset == 0 && write_bytes == PAGE_SIZE;
                let (target_page, under_pressure) = if overwrites_whole_page {
                    self.get_or_alloc_overwrite_page(ino, page_id)?
                } else {
                    self.get_or_load_cache_page(ino, page_id, old_size)?
                };
                should_flush_cache |= under_pressure;
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
        if total_write_size > 0 {
            touch_modified_inode(&inode);
        }
        inner.offset = current_offset;
        drop(inner);
        if should_flush_cache {
            crate::fs::writeback::request_writeback();
        }
        Ok(total_write_size)
    }

    fn truncate(&self, size: u64) -> SyscallResult {
        let sb = self.superblock.upgrade().ok_or(SysError::EIO)?;
        let inode = self.get_inode().ok_or(SysError::EIO)?;
        if inode.get_fs_flags() & (FS_IMMUTABLE_FL | FS_APPEND_FL) != 0 {
            return Err(SysError::EPERM);
        }
        let rel_path = self.current_rel_path(&sb, inode.get_ino());
        {
            let fs = sb.fs.lock();
            let root = fs.root_dir();
            let mut fat_file = root.open_file(&rel_path).map_err(fat32_error_to_sys)?;
            fat_file
                .seek(SeekFrom::Start(size))
                .map_err(|_| SysError::EIO)?;
            fat_file.truncate().map_err(|_| SysError::EIO)?;
        }
        inode.set_size(size as usize);
        inode.clear_punched_holes();
        touch_modified_inode(&inode);
        PAGE_CACHE
            .lock()
            .remove_inode_pages(tagged_inode_id(PAGE_CACHE_FS_FAT32, inode.get_ino()));
        Ok(0)
    }

    fn read_at_direct(&self, offset: usize, buf: &mut [u8]) -> SysResult<usize> {
        if !self.readable() {
            return Err(SysError::EBADF);
        }
        if buf.is_empty() {
            return Ok(0);
        }
        self.flush_dirty_pages(None);
        let inode = self.get_inode().ok_or(SysError::EIO)?;
        let file_size = inode.get_size();
        if offset >= file_size {
            return Ok(0);
        }
        let read_len = (file_size - offset).min(buf.len());
        let sb = self.superblock.upgrade().ok_or(SysError::EIO)?;
        let rel_path = self.current_rel_path(&sb, inode.get_ino());
        let fs = sb.fs.lock();
        let root = fs.root_dir();
        let mut fat_file = root.open_file(&rel_path).map_err(fat32_error_to_sys)?;
        fat_file
            .seek(SeekFrom::Start(offset as u64))
            .map_err(fat32_error_to_sys)?;
        fat_file
            .read(&mut buf[..read_len])
            .map_err(fat32_error_to_sys)
    }

    fn write_at_direct(&self, offset: usize, buf: &[u8]) -> SysResult<usize> {
        if !self.writable() {
            return Err(SysError::EBADF);
        }
        if buf.is_empty() {
            return Ok(0);
        }
        let inode = self.get_inode().ok_or(SysError::EIO)?;
        if inode.get_fs_flags() & (FS_IMMUTABLE_FL | FS_APPEND_FL) != 0 {
            return Err(SysError::EPERM);
        }
        self.flush_dirty_pages(None);
        let sb = self.superblock.upgrade().ok_or(SysError::EIO)?;
        let rel_path = self.current_rel_path(&sb, inode.get_ino());
        let fs = sb.fs.lock();
        let root = fs.root_dir();
        let mut fat_file = root.open_file(&rel_path).map_err(fat32_error_to_sys)?;
        fat_file
            .seek(SeekFrom::Start(offset as u64))
            .map_err(fat32_error_to_sys)?;
        let written = fat_file.write(buf).map_err(fat32_error_to_sys)?;
        if written > 0 {
            let end = offset + written;
            if end > inode.get_size() {
                inode.set_size(end);
            }
            touch_modified_inode(&inode);
            PAGE_CACHE
                .lock()
                .remove_inode_pages(tagged_inode_id(PAGE_CACHE_FS_FAT32, inode.get_ino()));
        }
        Ok(written)
    }

    fn cache_inode_id(&self) -> Option<usize> {
        self.get_inode()
            .map(|inode| tagged_inode_id(PAGE_CACHE_FS_FAT32, inode.get_ino()))
    }

    fn flush(&self) {
        self.flush_dirty_pages(None);
    }

    fn flush_pages(&self, max_pages: usize) -> (usize, bool) {
        self.flush_dirty_pages(Some(max_pages))
    }

    fn get_cache_frame(&self, page_id: usize) -> Option<Arc<FrameTracker>> {
        let inner = self.inner.lock();
        let inode = inner.dentry.get_inode()?;
        let ino = inode.get_ino();
        let file_size = inode.get_size();
        let Ok((target_page, under_pressure)) =
            self.get_or_load_cache_page(ino, page_id, file_size)
        else {
            return None;
        };
        drop(inner);
        if under_pressure && self.writable() {
            crate::fs::writeback::request_writeback();
        }
        target_page.read().resident_frame()
    }

    fn get_stat(&self, stat: &mut Kstat) -> SysResult<()> {
        let inner = self.inner.lock();
        let inode = inner.dentry.get_inode().ok_or(SysError::EIO)?;
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

    fn ioctl(&self, request: usize, argp: usize) -> SyscallResult {
        let inode = self.get_inode().ok_or(SysError::EIO)?;
        match request {
            FS_IOC_GETFLAGS => ioctl_get_fs_flags(inode, argp),
            FS_IOC_SETFLAGS => ioctl_set_fs_flags(inode, argp),
            _ => Err(SysError::ENOTTY),
        }
    }
}
