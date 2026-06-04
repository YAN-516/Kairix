use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::Dentry;
use crate::fs::File;
use crate::fs::Inode;
use crate::fs::page::pagecache::Page;
use crate::fs::page::pagecache::{PAGE_CACHE, PAGE_CACHE_FS_TMPFS, tagged_inode_id};
use crate::fs::tmpfs::inode::F_SEAL_WRITE;
use crate::fs::vfs::FileInner;
use crate::fs::vfs::OpenFlags;
use crate::fs::vfs::file::{
    FS_IOC_GETFLAGS, FS_IOC_SETFLAGS, ioctl_get_fs_flags, ioctl_set_fs_flags,
};
use crate::fs::vfs::kstat::Kstat;
use crate::mm::UserBuffer;
use crate::mm::frame_alloc;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use log::*;
use polyhal::common::FrameTracker;
use polyhal::consts::PAGE_SIZE;
use polyhal::timer::current_time;
use spin::MutexGuard;
use spin::mutex::Mutex;
use spin::rwlock::RwLock;
/// the file of tempfs
pub struct TempFile {
    readable: bool,
    writable: bool,
    append: bool,
    inner: Mutex<FileInner>,
}

// impl TempFile {
//     ///
//     pub fn new(dentry: Arc<dyn Dentry>) -> Self {
//         Self {
//             inner: Mutex::new(FileInner { offset: 0, dentry, flags: OpenFlags::empty() }),
//         }
//     }
// }

impl TempFile {
    ///
    pub fn new(
        readable: bool,
        writable: bool,
        append: bool,
        dentry: Arc<dyn Dentry>,
        flags: OpenFlags,
    ) -> Self {
        Self {
            readable,
            writable,
            append,
            inner: Mutex::new(FileInner {
                offset: 0,
                dentry,
                flags,
            }),
        }
    }
}

impl File for TempFile {
    ///Get the FileInner
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        self.inner.lock()
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
    fn ls(&self) -> Vec<(String, u64, u8)> {
        let inner = self.inner.lock();
        let dentry = &inner.dentry;
        let children = dentry.get_dentryinner().children.lock();
        let mut entries = Vec::new();
        for (name, child) in children.iter() {
            if let Some(inode) = child.get_inode() {
                let ino = inode.get_ino() as u64;
                let d_type = if inode
                    .get_mode()
                    .contains(crate::fs::vfs::inode::InodeMode::DIR)
                {
                    4 // DT_DIR
                } else if inode
                    .get_mode()
                    .contains(crate::fs::vfs::inode::InodeMode::FILE)
                {
                    8 // DT_REG
                } else if inode
                    .get_mode()
                    .contains(crate::fs::vfs::inode::InodeMode::LINK)
                {
                    10 // DT_LNK
                } else {
                    0 // DT_UNKNOWN
                };
                entries.push((name.clone(), ino, d_type));
            }
        }
        entries
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
        let should_update_atime = !inner.flags.contains(OpenFlags::O_NOATIME)
            && buf.buffers.iter().any(|slice| !slice.is_empty());
        let path = inner.dentry.path();
        let ino = tagged_inode_id(PAGE_CACHE_FS_TMPFS, inode.get_ino());
        let file_size = inode.get_size();
        let mut current_offset = inner.offset;
        let mut total_read_size = 0usize;
        if current_offset >= file_size {
            if should_update_atime {
                crate::syscall::maybe_update_atime(&path, &inode, false);
            }
            return Ok(0);
        }
        for slice in buf.buffers.iter_mut() {
            let mut slice_offset = 0;
            let slice_len = slice.len();
            while slice_offset < slice_len && current_offset < file_size {
                let (target_page, _) =
                    self.get_or_alloc_cache_page(ino, current_offset / PAGE_SIZE)?;
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
        if should_update_atime {
            crate::syscall::maybe_update_atime(&path, &inode, false);
        }
        Ok(total_read_size)
    }

    fn write(&self, buf: UserBuffer) -> SysResult<usize> {
        info!("enter VFS Write-back Cache");
        let mut inner = self.inner.lock();
        let inode = inner.dentry.get_inode().ok_or(SysError::EIO)?;
        if inode.get_fs_flags()
            & (crate::fs::vfs::inode::FS_IMMUTABLE_FL | crate::fs::vfs::inode::FS_APPEND_FL)
            != 0
        {
            return Err(SysError::EPERM);
        }
        let ino = tagged_inode_id(PAGE_CACHE_FS_TMPFS, inode.get_ino());

        // 检查 F_SEAL_WRITE seal
        if inode.get_seals() & F_SEAL_WRITE != 0 {
            return Err(SysError::EPERM);
        }

        // let ino = inode.get_ino();
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
                inode.clear_punched_hole_page(page_id);
                // 获取缓存页
                let (target_page, _) = self.get_or_alloc_cache_page(ino, page_id)?;
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
        let now_us = current_time().as_micros() as i64;
        let now_sec = now_us / 1_000_000;
        let now_nsec = (now_us % 1_000_000) * 1000;
        inode.set_mtime(now_sec, now_nsec);
        inode.set_ctime(now_sec, now_nsec);
        inner.offset = current_offset;
        Ok(total_write_size)
    }

    ///get inode from the Dentry of FileInner
    fn get_inode(&self) -> Option<Arc<dyn Inode>> {
        self.get_fileinner().dentry.get_inode()
    }
    fn cache_inode_id(&self) -> Option<usize> {
        self.get_inode()
            .map(|inode| tagged_inode_id(PAGE_CACHE_FS_TMPFS, inode.get_ino()))
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
        stat.st_rdev = inode.get_rdev() as u64;
        stat.st_blksize = 4096;
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

    fn truncate(&self, size: u64) -> SyscallResult {
        let inode = self.get_inode().ok_or(SysError::EIO)?;
        if inode.get_fs_flags()
            & (crate::fs::vfs::inode::FS_IMMUTABLE_FL | crate::fs::vfs::inode::FS_APPEND_FL)
            != 0
        {
            return Err(SysError::EPERM);
        }
        inode.set_size(size as usize);
        inode.clear_punched_holes();
        PAGE_CACHE
            .lock()
            .remove_inode_pages(tagged_inode_id(PAGE_CACHE_FS_TMPFS, inode.get_ino()));
        Ok(0)
    }

    fn ioctl(&self, request: usize, argp: usize) -> SyscallResult {
        let inode = self.get_inode().ok_or(SysError::EIO)?;
        match request {
            FS_IOC_GETFLAGS => ioctl_get_fs_flags(inode, argp),
            FS_IOC_SETFLAGS => ioctl_set_fs_flags(inode, argp),
            _ => Err(SysError::ENOTTY),
        }
    }

    fn get_cache_frame(&self, page_id: usize) -> Option<Arc<FrameTracker>> {
        let inner = self.inner.lock();
        let inode = inner.dentry.get_inode()?;
        let ino = tagged_inode_id(PAGE_CACHE_FS_TMPFS, inode.get_ino());
        let (target_page, _) = self.get_or_alloc_cache_page(ino, page_id).ok()?;
        Some(target_page.read().frame.clone())
    }
}

impl TempFile {
    /// 获取指定的缓存页，如果 Miss则分配零页
    fn get_or_alloc_cache_page(
        &self,
        ino: usize,
        page_id: usize,
    ) -> SysResult<(Arc<RwLock<Page>>, bool)> {
        {
            let mut cache = PAGE_CACHE.lock();
            if let Some(page) = cache.get_page_touch(ino, page_id) {
                return Ok((page, false));
            }
        }
        let mut cache_writer = PAGE_CACHE.lock();
        if let Some(page) = cache_writer.get_page_touch(ino, page_id) {
            return Ok((page, false));
        }

        let frame = Arc::new(frame_alloc().ok_or(SysError::ENOMEM)?);
        frame.ppn.get_bytes_array().fill(0);
        let page = Arc::new(RwLock::new(Page {
            frame,
            dirty: false,
        }));
        let under_pressure = cache_writer.insert_page(ino, page_id, page.clone());
        drop(cache_writer);
        Ok((page, under_pressure))
    }

    // pub fn new_with_flags(dentry: Arc<dyn Dentry>, flags: OpenFlags) -> Self {
    //     Self {
    //         inner: Mutex::new(FileInner {
    //             offset: 0,
    //             dentry,
    //             flags,
    //         }),
    //     }
    // }
}
