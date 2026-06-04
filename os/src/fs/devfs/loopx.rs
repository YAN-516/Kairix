#![allow(missing_docs)]
use crate::devices::BlockDevice;
use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::vfs::{
    DentryInner, FileInner, OpenFlags, dcache::GLOBAL_DCACHE,
    inode::{InodeInner, InodeMode, inode_alloc, make_rdev},
};
use crate::fs::{Dentry, File, Inode, String};
use crate::mm::{translated_ref, translated_refmut, UserBuffer};
use crate::task::{current_process, current_user_token};
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use log::error;
use polyhal::consts::PAGE_SIZE;
use polyhal::timer::current_time;
use spin::{Mutex, MutexGuard};

const LOOP_BLOCK_SIZE: usize = 512;
const LOOP_DEVICE_COUNT: usize = 8;

fn loop_device_inode(id: usize) -> Option<Arc<dyn Inode>> {
    GLOBAL_DCACHE
        .get(&alloc::format!("/dev/loop{}", id))
        .and_then(|dentry| dentry.get_inode())
}

fn loop_device_is_bound(inode: &Arc<dyn Inode>) -> bool {
    inode.get_backing_fd().is_some() || inode.get_backing_file().is_some()
}

pub struct LoopBlockDevice {
    file: Arc<dyn File>,
}

impl LoopBlockDevice {
    pub fn new(file: Arc<dyn File>) -> Self {
        Self { file }
    }
}

fn drop_backing_page_cache(file: &dyn File) {
    if let Some(inode_id) = file.cache_inode_id() {
        crate::fs::page::pagecache::PAGE_CACHE
            .lock()
            .remove_inode_pages(inode_id);
    }
}

fn touch_backing_inode(inode: Arc<dyn Inode>) {
    let now_us = current_time().as_micros() as i64;
    let now_sec = now_us / 1_000_000;
    let now_nsec = (now_us % 1_000_000) * 1000;
    inode.set_mtime(now_sec, now_nsec);
    inode.set_ctime(now_sec, now_nsec);
}

fn mark_backing_zero_page(file: &dyn File, page_id: usize, extend_end: Option<usize>) -> bool {
    if !file.supports_sparse_holes() {
        return false;
    }

    let Some(inode) = file.get_inode() else {
        return false;
    };
    if let Some(end) = extend_end {
        if end > inode.get_size() {
            inode.set_size(end);
        }
    }
    touch_backing_inode(inode.clone());
    inode.add_punched_hole_page(page_id);
    if let Some(inode_id) = file.cache_inode_id() {
        crate::fs::page::pagecache::PAGE_CACHE
            .lock()
            .remove_page(inode_id, page_id);
    }
    true
}

fn mark_backing_zero_range(file: &dyn File, offset: usize, len: usize, extend: bool) -> usize {
    if len == 0 || !file.supports_sparse_holes() {
        return 0;
    }
    let Some(inode) = file.get_inode() else {
        return 0;
    };
    let Some(end) = offset.checked_add(len) else {
        return 0;
    };
    if extend && end > inode.get_size() {
        inode.set_size(end);
    }
    let file_size = inode.get_size();
    let zero_end = end.min(file_size);
    let Some(first_page_start) = align_up_page(offset) else {
        return 0;
    };
    let first_page = first_page_start / PAGE_SIZE;
    let last_page_exclusive = zero_end / PAGE_SIZE;
    if first_page >= last_page_exclusive {
        return 0;
    }
    let cache_inode_id = file.cache_inode_id();
    {
        let mut cache = crate::fs::page::pagecache::PAGE_CACHE.lock();
        for page_id in first_page..last_page_exclusive {
            inode.add_punched_hole_page(page_id);
            if let Some(inode_id) = cache_inode_id {
                cache.remove_page(inode_id, page_id);
            }
        }
    }
    touch_backing_inode(inode);
    (last_page_exclusive - first_page) * PAGE_SIZE
}

fn convert_zero_dirty_pages_to_holes(file: &dyn File) -> usize {
    if !file.supports_sparse_holes() {
        return 0;
    }
    let Some(inode) = file.get_inode() else {
        return 0;
    };
    let Some(cache_inode_id) = file.cache_inode_id() else {
        return 0;
    };

    let dirty_pages = {
        crate::fs::page::pagecache::PAGE_CACHE
            .lock()
            .get_inode_dirty_pages(cache_inode_id)
    };
    if dirty_pages.is_empty() {
        return 0;
    }

    let mut zero_page_ids = Vec::new();
    for (page_id, page_lock) in dirty_pages {
        let page = page_lock.read();
        if page.dirty && page.frame.ppn.get_bytes_array().iter().all(|byte| *byte == 0) {
            zero_page_ids.push(page_id);
        }
    }
    if zero_page_ids.is_empty() {
        return 0;
    }

    {
        let mut cache = crate::fs::page::pagecache::PAGE_CACHE.lock();
        for page_id in zero_page_ids.iter().copied() {
            inode.add_punched_hole_page(page_id);
            cache.remove_page(cache_inode_id, page_id);
        }
    }
    touch_backing_inode(inode);
    zero_page_ids.len()
}

fn align_up_page(value: usize) -> Option<usize> {
    value
        .checked_add(PAGE_SIZE - 1)
        .map(|value| value / PAGE_SIZE * PAGE_SIZE)
}

fn write_backing_zero_bytes(file: &dyn File, offset: usize, len: usize) -> SysResult<()> {
    let zero_page = [0u8; PAGE_SIZE];
    let mut done = 0usize;
    while done < len {
        let write_len = (len - done).min(PAGE_SIZE);
        let written = write_backing_direct(file, offset + done, &zero_page[..write_len])?;
        if written == 0 {
            return Err(SysError::EIO);
        }
        done += written;
    }
    Ok(())
}

fn zero_backing_range(file: &dyn File, offset: usize, len: usize) -> SysResult<()> {
    if len == 0 {
        return Ok(());
    }
    let end = offset.checked_add(len).ok_or(SysError::EINVAL)?;
    if !file.supports_sparse_holes() {
        return write_backing_zero_bytes(file, offset, len);
    }

    let first_full_page = align_up_page(offset).ok_or(SysError::EINVAL)?;
    let last_full_page_end = end / PAGE_SIZE * PAGE_SIZE;
    if first_full_page >= last_full_page_end {
        return write_backing_zero_bytes(file, offset, len);
    }
    if offset < first_full_page {
        write_backing_zero_bytes(file, offset, first_full_page - offset)?;
    }
    if first_full_page < last_full_page_end {
        mark_backing_zero_range(
            file,
            first_full_page,
            last_full_page_end - first_full_page,
            false,
        );
    }
    if last_full_page_end < end {
        write_backing_zero_bytes(file, last_full_page_end, end - last_full_page_end)?;
    }
    Ok(())
}

fn backing_range_from_user(argp: usize) -> SysResult<(usize, usize)> {
    if argp == 0 {
        return Err(SysError::EINVAL);
    }
    let token = current_user_token();
    let range = *translated_ref(token, argp as *const [u64; 2])?;
    let start = range[0] as usize;
    let len = range[1] as usize;
    if start as u64 != range[0] || len as u64 != range[1] {
        return Err(SysError::EINVAL);
    }
    if start % LOOP_BLOCK_SIZE != 0 || len % LOOP_BLOCK_SIZE != 0 {
        return Err(SysError::EINVAL);
    }
    Ok((start, len))
}

fn write_backing_direct(file: &dyn File, offset: usize, buf: &[u8]) -> SysResult<usize> {
    let mut done = 0usize;
    while done < buf.len() {
        let pos = offset + done;
        let page_left = PAGE_SIZE - (pos % PAGE_SIZE);
        let write_len = page_left.min(buf.len() - done);
        let page_id = pos / PAGE_SIZE;
        let write_buf = &buf[done..done + write_len];
        let all_zero = write_buf.iter().all(|byte| *byte == 0);
        if all_zero {
            if pos % PAGE_SIZE == 0
                && write_len == PAGE_SIZE
                && mark_backing_zero_page(file, page_id, Some(pos + write_len))
            {
                done += write_len;
                continue;
            }
            if let Some(inode) = file.get_inode() {
                if inode.is_punched_hole_page(page_id) {
                    let end = pos + write_len;
                    if end > inode.get_size() {
                        inode.set_size(end);
                    }
                    touch_backing_inode(inode);
                    done += write_len;
                    continue;
                }
            }
        }
        match file.write_at_direct(pos, write_buf) {
            Ok(0) => break,
            Ok(n) => done += n,
            Err(err) => {
                if done > 0 {
                    return Ok(done);
                }
                return Err(err);
            }
        }
    }
    Ok(done)
}

impl BlockDevice for LoopBlockDevice {
    fn size(&self) -> u64 {
        self.file
            .get_inode()
            .map(|inode| inode.get_size() as u64)
            .unwrap_or(0)
    }

    fn block_size(&self) -> usize {
        LOOP_BLOCK_SIZE
    }

    fn read_block(&self, block_id: usize, buf: &mut [u8]) {
        let mut done = 0usize;
        while done < buf.len() {
            let offset = block_id * LOOP_BLOCK_SIZE + done;
            match self.file.read_at_direct(offset, &mut buf[done..]) {
                Ok(0) => break,
                Ok(n) => done += n,
                Err(err) => {
                    error!("loop block read failed: {:?}", err);
                    break;
                }
            }
        }
        if done < buf.len() {
            buf[done..].fill(0);
        }
    }

    fn write_block(&self, block_id: usize, buf: &[u8]) {
        if let Err(err) = write_backing_direct(self.file.as_ref(), block_id * LOOP_BLOCK_SIZE, buf) {
            error!("loop block write failed: {:?}", err);
        }
        crate::fs::writeback::queue_file_lazy(self.file.clone());
    }
}

pub fn loop_block_device_from_inode(inode: Arc<dyn Inode>) -> Option<Arc<dyn BlockDevice>> {
    inode
        .get_backing_file()
        .map(|file| Arc::new(LoopBlockDevice::new(file)) as Arc<dyn BlockDevice>)
}

pub struct LoopControlFile {
    inner: Mutex<FileInner>,
}

impl LoopControlFile {
    pub fn new(dentry: Arc<dyn Dentry>) -> Self {
        Self {
            inner: Mutex::new(FileInner { offset: 0, dentry , flags: OpenFlags::empty() }),
        }
    }
}

impl File for LoopControlFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        self.inner.lock()
    }

    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        true
    }

    fn read(&self, buf: UserBuffer) -> SysResult<usize> {
        let mut inner = self.get_fileinner();
        let inode = inner.dentry.get_inode().ok_or(SysError::EIO)?;
        let backing = inode.get_backing_file().ok_or(SysError::ENXIO)?;

        let mut total = 0usize;
        for slice in buf.buffers {
            if slice.is_empty() {
                continue;
            }
            match backing.read_at_direct(inner.offset + total, slice) {
                Ok(0) => break,
                Ok(n) => {
                    total += n;
                    if n < slice.len() {
                        break;
                    }
                }
                Err(err) => return Err(err),
            }
        }
        let ret = Ok(total);
        if let Ok(n) = ret {
            inner.offset += n;
        }
        ret
    }

    fn write(&self, buf: UserBuffer) -> SysResult<usize> {
        let mut inner = self.get_fileinner();
        let inode = inner.dentry.get_inode().ok_or(SysError::EIO)?;
        let backing = inode.get_backing_file().ok_or(SysError::ENXIO)?;

        let mut total = 0usize;
        for slice in buf.buffers {
            if slice.is_empty() {
                continue;
            }
            match write_backing_direct(backing.as_ref(), inner.offset + total, slice) {
                Ok(0) => break,
                Ok(n) => {
                    total += n;
                    if n < slice.len() {
                        break;
                    }
                }
                Err(err) => return Err(err),
            }
        }
        let ret = Ok(total);
        if let Ok(n) = ret {
            inner.offset += n;
            if n > 0 {
                crate::fs::writeback::queue_file_lazy(backing.clone());
            }
        }
        ret
    }

    fn flush(&self) {
        if let Some(backing) = self.get_inode().and_then(|inode| inode.get_backing_file()) {
            backing.flush();
        }
    }

    fn ioctl(&self, request: usize, _argp: usize) -> SyscallResult {
        const LOOP_CTL_GET_FREE: usize = 0x4C82;
        match request {
            LOOP_CTL_GET_FREE => {
                for id in 0..LOOP_DEVICE_COUNT {
                    let Some(inode) = loop_device_inode(id) else {
                        continue;
                    };
                    if !loop_device_is_bound(&inode) {
                        return Ok(id);
                    }
                }
                Err(SysError::ENOSPC)
            }
            _ => Err(SysError::ENOTTY),
        }
    }
}

unsafe impl Send for LoopControlDentry {}
unsafe impl Sync for LoopControlDentry {}

pub struct LoopControlDentry {
    inner: DentryInner,
}

impl LoopControlDentry {
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|_me| Self {
            inner: DentryInner::new(name, parent_weak),
        })
    }
}

impl Dentry for LoopControlDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }

    fn name(&self) -> &str {
        &self.inner.name
    }

    fn open(self: Arc<Self>, _flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Ok(Arc::new(LoopControlFile::new(self)))
    }
}

pub struct LoopControlInode {
    inner: InodeInner,
}

impl LoopControlInode {
    pub fn new() -> Self {
        Self {
            inner: InodeInner::new(inode_alloc(), 0, InodeMode::CHAR, make_rdev(10, 237) as usize),
        }
    }
}

impl Inode for LoopControlInode {
    fn get_mode(&self) -> InodeMode {
        self.inner.mode
    }

    fn get_size(&self) -> usize {
        self.inner.size.load(Ordering::SeqCst)
    }

    fn set_size(&self, new_size: usize) {
        self.inner.size.store(new_size, Ordering::SeqCst);
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
    fn get_atime(&self) -> (i64, i64) {
        (
            self.inner.atime_sec.load(Ordering::Relaxed),
            self.inner.atime_nsec.load(Ordering::Relaxed),
        )
    }
    fn set_atime(&self, sec: i64, nsec: i64) {
        self.inner.atime_sec.store(sec, Ordering::Relaxed);
        self.inner.atime_nsec.store(nsec, Ordering::Relaxed);
    }
    fn get_mtime(&self) -> (i64, i64) {
        (
            self.inner.mtime_sec.load(Ordering::Relaxed),
            self.inner.mtime_nsec.load(Ordering::Relaxed),
        )
    }
    fn set_mtime(&self, sec: i64, nsec: i64) {
        self.inner.mtime_sec.store(sec, Ordering::Relaxed);
        self.inner.mtime_nsec.store(nsec, Ordering::Relaxed);
    }
    fn get_ctime(&self) -> (i64, i64) {
        (
            self.inner.ctime_sec.load(Ordering::Relaxed),
            self.inner.ctime_nsec.load(Ordering::Relaxed),
        )
    }
    fn set_ctime(&self, sec: i64, nsec: i64) {
        self.inner.ctime_sec.store(sec, Ordering::Relaxed);
        self.inner.ctime_nsec.store(nsec, Ordering::Relaxed);
    }
}

pub struct LoopDeviceFile {
    inner: Mutex<FileInner>,
    #[allow(unused)]
    id: usize,
}

impl LoopDeviceFile {
    pub fn new(dentry: Arc<dyn Dentry>, id: usize) -> Self {
        Self {
            inner: Mutex::new(FileInner { offset: 0, dentry , flags: OpenFlags::empty() }),
            id,
        }
    }
}

impl File for LoopDeviceFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        self.inner.lock()
    }

    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        true
    }

    fn read(&self, buf: UserBuffer) -> SysResult<usize> {
        let mut inner = self.get_fileinner();
        let inode = inner.dentry.get_inode().ok_or(SysError::EIO)?;
        let backing = inode.get_backing_file().ok_or(SysError::ENXIO)?;

        let mut total = 0usize;
        for slice in buf.buffers {
            if slice.is_empty() {
                continue;
            }
            match backing.read_at_direct(inner.offset + total, slice) {
                Ok(0) => break,
                Ok(n) => {
                    total += n;
                    if n < slice.len() {
                        break;
                    }
                }
                Err(err) => return Err(err),
            }
        }
        let ret = Ok(total);
        if let Ok(n) = ret {
            inner.offset += n;
        }
        ret
    }

    fn write(&self, buf: UserBuffer) -> SysResult<usize> {
        let mut inner = self.get_fileinner();
        let inode = inner.dentry.get_inode().ok_or(SysError::EIO)?;
        let backing = inode.get_backing_file().ok_or(SysError::ENXIO)?;

        let mut total = 0usize;
        for slice in buf.buffers {
            if slice.is_empty() {
                continue;
            }
            match write_backing_direct(backing.as_ref(), inner.offset + total, slice) {
                Ok(0) => break,
                Ok(n) => {
                    total += n;
                    if n < slice.len() {
                        break;
                    }
                }
                Err(err) => return Err(err),
            }
        }
        let ret = Ok(total);
        if let Ok(n) = ret {
            inner.offset += n;
            if n > 0 {
                crate::fs::writeback::queue_file_lazy(backing.clone());
            }
        }
        ret
    }

    fn flush(&self) {
        if let Some(backing) = self.get_inode().and_then(|inode| inode.get_backing_file()) {
            backing.flush();
        }
    }

    fn get_stat(&self, stat: &mut crate::fs::vfs::kstat::Kstat) -> SysResult<()> {
        let inode = self.get_inode().ok_or(SysError::EIO)?;
        stat.st_ino = inode.get_ino() as u64;
        stat.st_nlink = inode.get_nlink() as u32;
        stat.st_mode = inode.get_mode().bits();
        stat.st_blksize = 512;
        stat.st_rdev = inode.get_rdev() as u64;
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

        let mut size = inode.get_size() as i64;
        if let Some(backing_file) = inode.get_backing_file() {
            if let Some(backing_inode) = backing_file.get_inode() {
                size = backing_inode.get_size() as i64;
            }
        }
        stat.st_size = size;
        stat.st_blocks = (size as u64 + 511) / 512;
        Ok(())
    }

    fn ioctl(&self, request: usize, argp: usize) -> SyscallResult {
        const LOOP_SET_FD: usize = 0x4C00;
        const LOOP_CLR_FD: usize = 0x4C01;
        const LOOP_SET_STATUS: usize = 0x4C02;
        const LOOP_GET_STATUS: usize = 0x4C03;
        const LOOP_SET_STATUS64: usize = 0x4C04;
        const LOOP_GET_STATUS64: usize = 0x4C05;
        const BLKGETSIZE: usize = 0x1260;
        const BLKGETSIZE64: usize = 0x8008_1272;
        const BLKSSZGET: usize = 0x1268;
        const BLKBSZGET: usize = 0x8008_1270;
        const BLKIOMIN: usize = 0x1278;
        const BLKIOOPT: usize = 0x1279;
        const BLKALIGNOFF: usize = 0x127a;
        const BLKPBSZGET: usize = 0x127b;
        const BLKDISCARDZEROES: usize = 0x127c;
        const BLKROTATIONAL: usize = 0x127e;
        const BLKDISCARD: usize = 0x1277;
        const BLKSECDISCARD: usize = 0x127d;
        const BLKZEROOUT: usize = 0x127f;
        match request {
            LOOP_GET_STATUS | LOOP_GET_STATUS64 => {
                let inode = self.get_inode().ok_or(SysError::EIO)?;
                if loop_device_is_bound(&inode) {
                    Ok(0)
                } else {
                    // 设备未绑定，返回 ENXIO 表示空闲
                    Err(SysError::ENXIO)
                }
            }
            LOOP_SET_FD => {
                if let Some(inode) = self.get_inode() {
                    if loop_device_is_bound(&inode) {
                        return Err(SysError::EBUSY);
                    }
                    let process = current_process();
                    let inner = process.inner_exclusive_access();
                    let Some(file) = inner.fd_table.get(argp).and_then(|x| x.as_ref()).cloned()
                    else {
                        return Err(SysError::EBADF);
                    };
                    drop(inner);
                    convert_zero_dirty_pages_to_holes(file.as_ref());
                    file.flush();
                    drop_backing_page_cache(file.as_ref());
                    if let Some(backing_inode) = file.get_inode() {
                        inode.set_size(backing_inode.get_size());
                    }
                    inode.set_backing_file(Some(file));
                    inode.set_backing_fd(Some(argp));
                }
                Ok(0)
            }
            LOOP_CLR_FD => {
                if let Some(inode) = self.get_inode() {
                    if inode.get_backing_fd().is_none() && inode.get_backing_file().is_none() {
                        return Err(SysError::ENXIO);
                    }
                    if let Some(backing) = inode.get_backing_file() {
                        backing.flush();
                        drop_backing_page_cache(backing.as_ref());
                    }
                    inode.set_backing_fd(None);
                    inode.set_backing_file(None);
                }
                Ok(0)
            }
            LOOP_SET_STATUS | LOOP_SET_STATUS64 => {
                // TODO: 设置 loop 设备参数
                Ok(0)
            }
            BLKGETSIZE => {
                if argp == 0 {
                    return Err(SysError::EINVAL);
                }
                let token = current_user_token();
                let size_ptr = translated_refmut(token, argp as *mut u64)?;
                let mut size = 0u64;
                if let Some(backing_file) =
                    self.get_inode().and_then(|inode| inode.get_backing_file())
                {
                    if let Some(inode) = backing_file.get_inode() {
                        size = (inode.get_size() / LOOP_BLOCK_SIZE) as u64;
                    }
                }
                *size_ptr = size;
                Ok(0)
            }
            BLKGETSIZE64 => {
                if argp == 0 {
                    return Err(SysError::EINVAL);
                }
                let token = current_user_token();
                let size_ptr = translated_refmut(token, argp as *mut u64)?;
                let mut size = 0u64;
                if let Some(backing_file) =
                    self.get_inode().and_then(|inode| inode.get_backing_file())
                {
                    if let Some(inode) = backing_file.get_inode() {
                        size = inode.get_size() as u64;
                    }
                }
                *size_ptr = size;
                Ok(0)
            }
            #[allow(non_snake_case)]
            BLKBSZGET => {
                if argp == 0 {
                    return Err(SysError::EINVAL);
                }
                let token = current_user_token();
                let sz_ptr = translated_refmut(token, argp as *mut usize)?;
                *sz_ptr = LOOP_BLOCK_SIZE;
                Ok(0)
            }
            BLKSSZGET | BLKIOMIN | BLKIOOPT | BLKPBSZGET => {
                if argp == 0 {
                    return Err(SysError::EINVAL);
                }
                let token = current_user_token();
                let sz_ptr = translated_refmut(token, argp as *mut i32)?;
                *sz_ptr = LOOP_BLOCK_SIZE as i32;
                Ok(0)
            }
            BLKALIGNOFF | BLKROTATIONAL => {
                if argp == 0 {
                    return Err(SysError::EINVAL);
                }
                let token = current_user_token();
                let value_ptr = translated_refmut(token, argp as *mut i32)?;
                *value_ptr = 0;
                Ok(0)
            }
            BLKDISCARDZEROES => {
                if argp == 0 {
                    return Err(SysError::EINVAL);
                }
                let token = current_user_token();
                let value_ptr = translated_refmut(token, argp as *mut i32)?;
                *value_ptr = 1;
                Ok(0)
            }
            BLKDISCARD | BLKSECDISCARD | BLKZEROOUT => {
                let (start, len) = backing_range_from_user(argp)?;
                let Some(backing) = self.get_inode().and_then(|inode| inode.get_backing_file()) else {
                    return Err(SysError::ENXIO);
                };
                let size = backing
                    .get_inode()
                    .map(|inode| inode.get_size())
                    .ok_or(SysError::EIO)?;
                let end = start.checked_add(len).ok_or(SysError::EINVAL)?;
                if end > size {
                    return Err(SysError::EINVAL);
                }
                zero_backing_range(backing.as_ref(), start, len)?;
                if len > 0 {
                    crate::fs::writeback::queue_file_lazy(backing);
                }
                Ok(0)
            }
            _ => Err(SysError::ENOTTY),
        }
    }
}

unsafe impl Send for LoopDeviceDentry {}
unsafe impl Sync for LoopDeviceDentry {}

pub struct LoopDeviceDentry {
    inner: DentryInner,
}

impl LoopDeviceDentry {
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|_me| Self {
            inner: DentryInner::new(name, parent_weak),
        })
    }
}

impl Dentry for LoopDeviceDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }

    fn name(&self) -> &str {
        &self.inner.name
    }

    fn open(self: Arc<Self>, _flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        let name = self.name();
        let id = name
            .strip_prefix("loop")
            .unwrap_or(name)
            .parse::<usize>()
            .unwrap_or(0);
        Ok(Arc::new(LoopDeviceFile::new(self, id)))
    }
}

pub struct LoopDeviceInode {
    inner: InodeInner,
    backing_fd: AtomicUsize,
    backing_file: Mutex<Option<Arc<dyn File>>>,
}

impl LoopDeviceInode {
    pub fn new(id: usize) -> Self {
        Self {
            inner: InodeInner::new(inode_alloc(), 0, InodeMode::BLOCK, make_rdev(7, id as u32) as usize),
            backing_fd: AtomicUsize::new(usize::MAX),
            backing_file: Mutex::new(None),
        }
    }
}

impl Inode for LoopDeviceInode {
    fn get_mode(&self) -> InodeMode {
        self.inner.mode
    }

    fn get_size(&self) -> usize {
        self.inner.size.load(Ordering::SeqCst)
    }

    fn set_size(&self, new_size: usize) {
        self.inner.size.store(new_size, Ordering::SeqCst);
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
    fn get_atime(&self) -> (i64, i64) {
        (
            self.inner.atime_sec.load(Ordering::Relaxed),
            self.inner.atime_nsec.load(Ordering::Relaxed),
        )
    }

    fn set_atime(&self, sec: i64, nsec: i64) {
        self.inner.atime_sec.store(sec, Ordering::Relaxed);
        self.inner.atime_nsec.store(nsec, Ordering::Relaxed);
    }

    fn get_mtime(&self) -> (i64, i64) {
        (
            self.inner.mtime_sec.load(Ordering::Relaxed),
            self.inner.mtime_nsec.load(Ordering::Relaxed),
        )
    }

    fn set_mtime(&self, sec: i64, nsec: i64) {
        self.inner.mtime_sec.store(sec, Ordering::Relaxed);
        self.inner.mtime_nsec.store(nsec, Ordering::Relaxed);
    }

    fn get_ctime(&self) -> (i64, i64) {
        (
            self.inner.ctime_sec.load(Ordering::Relaxed),
            self.inner.ctime_nsec.load(Ordering::Relaxed),
        )
    }

    fn set_ctime(&self, sec: i64, nsec: i64) {
        self.inner.ctime_sec.store(sec, Ordering::Relaxed);
        self.inner.ctime_nsec.store(nsec, Ordering::Relaxed);
    }

    fn get_backing_fd(&self) -> Option<usize> {
        let fd = self.backing_fd.load(Ordering::Relaxed);
        if fd == usize::MAX { None } else { Some(fd) }
    }

    fn set_backing_fd(&self, fd: Option<usize>) {
        self.backing_fd.store(fd.unwrap_or(usize::MAX), Ordering::Relaxed);
    }

    fn get_backing_file(&self) -> Option<Arc<dyn File>> {
        self.backing_file.lock().clone()
    }

    fn set_backing_file(&self, file: Option<Arc<dyn File>>) {
        *self.backing_file.lock() = file;
    }
}
