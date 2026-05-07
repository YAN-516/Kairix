use alloc::boxed::Box;
use alloc::ffi::CString;
use alloc::string::{String, ToString};
use alloc::sync::{Arc, Weak};
use alloc::{format, vec, vec::Vec};
use core::cell::RefMut;
use core::sync::atomic::{AtomicUsize, Ordering};

use bitflags::*;
use lazy_static::*;
use log::{info, warn};
use crate::sync::{SpinNoIrqLock, SpinMutexGuard, SpinNoIrq};

use virtio_drivers::device::blk::VirtIOBlk;
use virtio_drivers::transport::mmio::{MmioTransport, VirtIOHeader};
use virtio_drivers::transport::{DeviceType, Transport};

use lwext4_rust::bindings::{O_APPEND, O_RDONLY, O_RDWR, O_TRUNC, O_WRONLY, SEEK_SET};
use lwext4_rust::{InodeTypes, Lwext4File};

// use crate::config::PAGE_SIZE;
use crate::drivers::block::BLOCK_DEVICE;
use crate::error::{SysError, SysResult, SyscallResult};
use crate::mm::{UserBuffer, frame_alloc};
use polyhal::common::FrameTracker;
use polyhal::consts::PAGE_SIZE;

use crate::fs::vfs::{
    Dentry, FileInner, OpenFlags,
    dcache::GLOBAL_DCACHE,
    file::File,
    inode::{Inode, InodeMode},
    kstat::Kstat,
    path::{resolve_path, split_parent_and_name},
};
use crate::fs::lwext4::ext4::dir::ExtDir;
use crate::fs::lwext4::{dentry::Ext4Dentry, disk::Disk, inode::Ext4Inode};
use crate::fs::get_filesystem;
use crate::fs::page::pagecache::{PAGE_CACHE, Page};
use crate::fs::lwext4::lwext4_err_to_sys;
///the Ext4File
pub struct Ext4File {
    readable: bool,
    writable: bool,
    inner: SpinNoIrqLock<FileInner>,
    ///
    pub ext4file: SpinNoIrqLock<Lwext4File>,
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
                if ExtDir::open(&c_probe).is_ok() {
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
            inner: SpinNoIrqLock::new(FileInner { offset: 0, dentry }),
            ext4file: SpinNoIrqLock::new(file),
        })
    }

    #[allow(unused)]
    /// Truncate the inode to the given size
    fn truncate(&mut self, size: u64) -> SysResult<usize> {
        info!("truncate file to size={}", size);
        let res = self.ext4file.lock().file_truncate(size);
        if let Err(err) = res {
            return Err(crate::fs::lwext4::lwext4_err_to_sys(err));
        }
        let inner = self.inner.lock();
        if let Some(inode) = inner.dentry.get_inode() {
            inode.set_size(size as usize);
        }
        Ok(0)
    }
    /// 从磁盘加载指定页的数据到物理帧中（如果超出文件范围则清零）
    fn load_page_from_disk(&self, page_id: usize, old_size: usize) -> Arc<SpinNoIrqLock<Page>> {
        let new_frame = Arc::new(frame_alloc().unwrap());
        let page_start_offset = page_id * PAGE_SIZE;
        let bytes = new_frame.ppn.get_bytes_array();
        if page_start_offset < old_size {
            let valid_len = (old_size - page_start_offset).min(PAGE_SIZE);
            let mut ext4file = self.ext4file.lock();
            ext4file.file_seek(page_start_offset as i64, SEEK_SET).unwrap();
            let buffer = &mut bytes[..valid_len];
            let read_len = ext4file.file_read(buffer).unwrap();
            drop(ext4file);
            assert_eq!(read_len, valid_len);
            bytes[valid_len..].fill(0);
        } else {
            bytes.fill(0);
        }
        Arc::new(SpinNoIrqLock::new(Page {
            frame: new_frame,
            dirty: false,
        }))
    }
    /// 获取指定的缓存页，如果 Miss 则自动从磁盘加载并放入缓存
    fn get_or_load_cache_page(
        &self,
        ino: usize,
        page_id: usize,
        old_size: usize,
    ) -> Arc<SpinNoIrqLock<Page>> {
        if let Some(page) = PAGE_CACHE.lock().get_page(ino, page_id) {
            return page;
        }
        let mut cache_writer = PAGE_CACHE.lock();
        if let Some(page) = cache_writer.get_page(ino, page_id) {
            return page;
        }
        let new_page = self.load_page_from_disk(page_id, old_size);
        cache_writer.insert_page(ino, page_id, new_page.clone());

        // 顺序预读：加载当前页后，若下一页不在缓存中且在文件范围内，则同步预读
        let next_page_id = page_id + 1;
        let next_offset = next_page_id * PAGE_SIZE;
        if next_offset < old_size && cache_writer.get_page(ino, next_page_id).is_none() {
            let next_page = self.load_page_from_disk(next_page_id, old_size);
            cache_writer.insert_page(ino, next_page_id, next_page);
        }

        new_page
    }
}

impl File for Ext4File {
    fn get_fileinner(&self) -> SpinMutexGuard<'_, FileInner, SpinNoIrq> {
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
        let inode = inner.dentry.get_inode().unwrap();
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
                    let page_reader = target_page.lock();
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
        // info!("enter VFS Write-back Cache");
        let mut inner = self.inner.lock();
        let inode = inner.dentry.get_inode().unwrap();
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
                let target_page = self.get_or_load_cache_page(ino, page_id, old_size);
                // 写入数据并标记脏页
                {
                    let mut page_writer = target_page.lock();
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
            // 同步 ext4 文件大小，失败时记录日志但不中断写入
            if let Err(e) = self.ext4file.lock().file_truncate(current_offset as u64) {
                warn!("file_truncate failed: offset={}, err={:?}", current_offset, e);
            }
        }
        inner.offset = current_offset;
        Ok(total_write_size)
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
        stat.st_blksize = 512;
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

    ///
    fn flush(&self) {
        //只读不需要写回磁盘
        if !self.writable() {
            info!("File is read-only, skipping flush.");
            return;
        }
        info!("enter VFS flush (write-back to disk)");
        let inner = self.inner.lock();
        let inode = inner.dentry.get_inode().unwrap();
        let inode_id = inode.get_ino();
        let file_size = inode.get_size();
        let max_page_id = (file_size + PAGE_SIZE - 1) / PAGE_SIZE;
        let mut cache_reader = PAGE_CACHE.lock();
        for page_id in 0..max_page_id {
            if let Some(page_lock) = cache_reader.get_page(inode_id, page_id) {
                let mut page = page_lock.lock();
                if page.dirty {
                    let offset = page_id * PAGE_SIZE;
                    let write_len = if offset + PAGE_SIZE > file_size {
                        file_size - offset
                    } else {
                        PAGE_SIZE
                    };
                    let mut ext4file = self.ext4file.lock();
                    ext4file.file_seek(offset as i64, SEEK_SET).unwrap();
                    let buffer = &page.frame.ppn.get_bytes_array()[..write_len];
                    ext4file.file_write(buffer).unwrap();
                    drop(ext4file);
                    page.dirty = false;
                }
            }
        }
        drop(cache_reader);
        if let Err(e) = self.ext4file.lock().file_cache_flush() {
            warn!("ext4 cache flush failed: {:?}", e);
        }
        info!("finish VFS flush");
    }

    fn ioctl(&self, _request: usize, _argp: usize) -> SyscallResult {
        Err(SysError::ENOTTY)
    }

    fn truncate(&self, size: u64) -> SyscallResult {
        let res = self.ext4file.lock().file_truncate(size);
        if let Err(err) = res {
            return Err(lwext4_err_to_sys(err));
        }
        let inner = self.inner.lock();
        if let Some(inode) = inner.dentry.get_inode() {
            inode.set_size(size as usize);
        }
        Ok(0)
    }

    fn get_cache_frame(&self, page_id: usize) -> Option<Arc<FrameTracker>> {
        let inner = self.inner.lock();
        let inode = inner.dentry.get_inode().unwrap();
        let ino = inode.get_ino();
        let file_size = inode.get_size();
        let target_page = self.get_or_load_cache_page(ino, page_id, file_size);
        Some(target_page.lock().frame.clone())
    }
}

impl OpenFlags {
    /// Do not check validity for simplicity
    /// Return (readable, writable)
    pub fn read_write(&self) -> (bool, bool) {
        if self.is_empty() {
            (true, false)
        } else if self.contains(Self::WRONLY) {
            (false, true)
        } else {
            (true, true)
        }
    }
    /// Convert OpenFlags to ext4 open flags (O_RDONLY, O_WRONLY, O_RDWR)
    pub fn into_ext4_flags(&self) -> u32 {
        if self.contains(Self::RDWR) {
            O_RDWR
        } else if self.contains(Self::WRONLY) {
            O_WRONLY
        } else {
            O_RDONLY
        }
    }
}
