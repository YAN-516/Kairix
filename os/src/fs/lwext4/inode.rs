use core::cell::RefCell;
use core::ptr::NonNull;
use core::sync::atomic::Ordering;

use crate::fs::page::pagecache::{PAGE_CACHE, PAGE_CACHE_FS_EXT4, tagged_inode_id};
use crate::fs::vfs::inode::{
    InodeMode, XATTR_CREATE, XATTR_NAME_MAX, XATTR_REPLACE, XATTR_SIZE_MAX,
    check_user_xattr_support, check_xattr_write_allowed, note_punched_hole_inserted,
    note_punched_holes_removed,
};
use alloc::ffi::CString;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use log::*;
use spin::mutex::Mutex;

use lwext4_rust::{
    Ext4BlockWrapper, InodeTypes, KernelDevOp, Lwext4File,
    bindings::{
        O_APPEND, O_CREAT, O_RDONLY, O_RDWR, O_TRUNC, O_WRONLY, SEEK_CUR, SEEK_END, SEEK_SET,
        ext4_getxattr, ext4_inode_stat, ext4_listxattr, ext4_removexattr, ext4_setxattr,
    },
};

use virtio_drivers::{
    device::blk::VirtIOBlk,
    transport::{
        DeviceType, Transport,
        mmio::{MmioTransport, VirtIOHeader},
    },
};

use crate::config::BLOCK_SIZE;
use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::vfs::inode::{Inode, InodeInner};
use crate::fs::vfs::kstat::Kstat;
use crate::logging;

use super::disk::Disk;
use super::ext4::file::ExtFS;
use super::{Lwext4Op, with_lwext4_path_lock_op};
#[allow(unused)]
///The inode of the Ext4 filesystem
/// the InodeInner is ino
/// this_type is the InodeTypes
pub struct Ext4Inode {
    inner: Mutex<InodeInner>,
    this_type: InodeTypes,
    path: String,
    cache_inode_id: usize,
}

unsafe impl Send for Ext4Inode {}
unsafe impl Sync for Ext4Inode {}

impl Ext4Inode {
    ///
    pub fn new(ino: usize, types: InodeTypes, path: String, mount_id: usize) -> Self {
        info!("Inode new {:?} with ino {}", types, ino);
        let mode = InodeMode::from_inode_type(types.clone());
        let cache_key = ((mount_id & 0x0fff_ffff) << 32) | (ino & 0xffff_ffff);
        let cache_inode_id = tagged_inode_id(PAGE_CACHE_FS_EXT4, cache_key);

        Self {
            inner: Mutex::new(InodeInner::new(ino, 0, mode, 0)),
            this_type: types,
            path,
            cache_inode_id,
        }
    }

    /// Initialize the VFS inode cache from authoritative on-disk metadata.
    pub fn sync_from_disk_stat(&self, stat: &ext4_inode_stat) {
        let mut inner = self.inner.lock();
        inner.ino = stat.ino as usize;
        inner.size.store(stat.size as usize, Ordering::Relaxed);
        inner.nlink.store(stat.nlink as usize, Ordering::Relaxed);
        inner.mode = InodeMode::from_bits_truncate(stat.mode);
        inner.uid.store(stat.uid as usize, Ordering::Relaxed);
        inner.gid.store(stat.gid as usize, Ordering::Relaxed);
        inner.rdev.store(stat.rdev as usize, Ordering::Relaxed);
        inner.atime_sec.store(stat.atime as i64, Ordering::Relaxed);
        inner.atime_nsec.store(0, Ordering::Relaxed);
        inner.mtime_sec.store(stat.mtime as i64, Ordering::Relaxed);
        inner.mtime_nsec.store(0, Ordering::Relaxed);
        inner.ctime_sec.store(stat.ctime as i64, Ordering::Relaxed);
        inner.ctime_nsec.store(0, Ordering::Relaxed);
        inner.fs_flags.store(stat.flags as usize, Ordering::Relaxed);
    }

    fn has_xattr(&self, name: &str) -> SysResult<bool> {
        let cpath = CString::new(self.path.clone()).map_err(|_| SysError::EINVAL)?;
        let mut list_size = 0usize;
        let ret = with_lwext4_path_lock_op(&self.path, Lwext4Op::Xattr, || unsafe {
            ext4_listxattr(cpath.as_ptr(), core::ptr::null_mut(), 0, &mut list_size)
        })?;
        if ret != 0 {
            return Err(super::lwext4_err_to_sys(ret));
        }
        if list_size == 0 {
            return Ok(false);
        }

        let mut list = vec![0u8; list_size];
        let ret = with_lwext4_path_lock_op(&self.path, Lwext4Op::Xattr, || unsafe {
            ext4_listxattr(
                cpath.as_ptr(),
                list.as_mut_ptr() as *mut core::ffi::c_char,
                list.len(),
                &mut list_size,
            )
        })?;
        if ret != 0 {
            return Err(super::lwext4_err_to_sys(ret));
        }

        Ok(list[..list_size]
            .split(|byte| *byte == 0)
            .any(|entry| entry == name.as_bytes()))
    }
}

/// Combine fresh ext4 allocation metadata with mutable VFS inode state.
pub fn fill_ext4_kstat(inode: &dyn Inode, disk: &ext4_inode_stat, stat: &mut Kstat) {
    stat.st_ino = disk.ino as u64;
    stat.st_nlink = disk.nlink;
    stat.st_size = if inode.get_mode().get_type() == InodeMode::FILE {
        inode.get_size() as i64
    } else {
        disk.size as i64
    };
    stat.st_mode = inode.get_mode().bits();
    stat.st_uid = inode.get_uid() as u32;
    stat.st_gid = inode.get_gid() as u32;
    stat.st_rdev = inode.get_rdev() as u64;
    stat.st_blksize = disk.block_size as i32;
    stat.st_blocks = disk
        .blocks
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
}

impl Inode for Ext4Inode {
    /// Get the attributes of the file, such as size, permissions, etc.
    fn get_attr(&self) -> SysResult<usize> {
        unimplemented!()
    }
    /// Flush the file, synchronize the data to disk.
    fn fsync(&self) -> SysResult<usize> {
        unimplemented!()
    }
    fn truncate(&self, size: u64) -> SysResult<usize> {
        self.set_size(size as usize);
        self.truncate_punched_holes(size as usize);
        // 截断文件时清除该 inode 的页缓存，避免旧页面被后续写入/读取误用
        PAGE_CACHE.lock().remove_inode_pages(self.cache_inode_id);
        // 注意：实际的 ext4 文件截断由 Ext4File::new() 中的 O_TRUNC 标志完成，
        // 或者由 Ext4File::truncate() 方法完成。
        // 这里只更新 in-memory 状态和清除页缓存。
        Ok(0)
    }
    ///
    fn get_types(&self) -> InodeTypes {
        match self.this_type {
            InodeTypes::EXT4_DE_REG_FILE => InodeTypes::EXT4_DE_REG_FILE,
            InodeTypes::EXT4_DE_DIR => InodeTypes::EXT4_DE_DIR,
            InodeTypes::EXT4_DE_SYMLINK => InodeTypes::EXT4_DE_SYMLINK,
            InodeTypes::EXT4_DE_CHRDEV => InodeTypes::EXT4_DE_CHRDEV,
            InodeTypes::EXT4_DE_BLKDEV => InodeTypes::EXT4_DE_BLKDEV,
            InodeTypes::EXT4_DE_FIFO => InodeTypes::EXT4_DE_FIFO,
            InodeTypes::EXT4_DE_SOCK => InodeTypes::EXT4_DE_SOCK,
            _ => {
                warn!("Unsupported InodeType: {:?}", self.this_type);
                InodeTypes::EXT4_DE_UNKNOWN
            }
        }
    }

    fn readlink(&self) -> Result<String, i32> {
        if self.this_type != InodeTypes::EXT4_DE_SYMLINK {
            return Err(-22);
        }
        let cpath = CString::new(self.path.clone()).map_err(|_| -22)?;
        let mut buf = vec![0u8; 4096];
        match ExtFS::readlink(&cpath, &mut buf) {
            Ok(len) => {
                buf.truncate(len);
                Ok(String::from_utf8_lossy(&buf).into_owned())
            }
            Err(e) => Err(e.code() as i32),
        }
    }
    fn get_ino(&self) -> usize {
        self.inner.lock().ino
    }

    fn cache_inode_id(&self) -> Option<usize> {
        Some(self.cache_inode_id)
    }

    fn get_punched_hole_pages(&self) -> usize {
        self.inner.lock().punched_hole_pages.len()
    }

    fn is_punched_hole_page(&self, page_id: usize) -> bool {
        self.inner.lock().punched_hole_pages.contains(&page_id)
    }

    fn add_punched_hole_page(&self, page_id: usize) {
        if self.inner.lock().punched_hole_pages.insert(page_id) {
            note_punched_hole_inserted();
        }
    }

    fn clear_punched_hole_page(&self, page_id: usize) {
        if self.inner.lock().punched_hole_pages.remove(&page_id) {
            note_punched_holes_removed(1);
        }
    }

    fn clear_punched_holes(&self) {
        let mut inner = self.inner.lock();
        let removed = inner.punched_hole_pages.len();
        inner.punched_hole_pages.clear();
        note_punched_holes_removed(removed);
    }

    fn truncate_punched_holes(&self, size: usize) {
        let first_invalid_page = size.div_ceil(polyhal::consts::PAGE_SIZE);
        let mut inner = self.inner.lock();
        let removed = inner.punched_hole_pages.split_off(&first_invalid_page);
        note_punched_holes_removed(removed.len());
    }

    fn get_size(&self) -> usize {
        self.inner.lock().size.load(Ordering::Relaxed)
    }

    fn set_size(&self, new_size: usize) {
        self.inner.lock().size.store(new_size, Ordering::Relaxed);
    }

    fn get_nlink(&self) -> usize {
        self.inner.lock().nlink.load(Ordering::Relaxed)
    }
    fn get_rdev(&self) -> usize {
        self.inner.lock().rdev.load(Ordering::Relaxed)
    }
    fn set_rdev(&self, rdev: usize) {
        self.inner.lock().rdev.store(rdev, Ordering::Relaxed);
    }
    fn get_fs_flags(&self) -> u32 {
        self.inner.lock().fs_flags.load(Ordering::Relaxed) as u32
    }
    fn set_fs_flags(&self, flags: u32) {
        self.inner
            .lock()
            .fs_flags
            .store(flags as usize, Ordering::Relaxed);
    }

    fn get_mode(&self) -> InodeMode {
        self.inner.lock().mode
    }
    fn set_mode(&self, mode: InodeMode) {
        self.inner.lock().mode = mode;
    }
    fn get_uid(&self) -> usize {
        self.inner.lock().uid.load(Ordering::Relaxed)
    }
    fn set_uid(&self, uid: usize) {
        self.inner.lock().uid.store(uid, Ordering::Relaxed);
    }
    fn get_gid(&self) -> usize {
        self.inner.lock().gid.load(Ordering::Relaxed)
    }
    fn set_gid(&self, gid: usize) {
        self.inner.lock().gid.store(gid, Ordering::Relaxed);
    }
    fn inc_nlink(&self) {
        self.inner.lock().nlink.fetch_add(1, Ordering::SeqCst);
    }

    fn dec_nlink(&self) {
        self.inner.lock().nlink.fetch_sub(1, Ordering::SeqCst);
    }

    fn get_atime(&self) -> (i64, i64) {
        let inner = self.inner.lock();
        (
            inner.atime_sec.load(Ordering::Relaxed),
            inner.atime_nsec.load(Ordering::Relaxed),
        )
    }

    fn set_atime(&self, sec: i64, nsec: i64) {
        let inner = self.inner.lock();
        inner.atime_sec.store(sec, Ordering::Relaxed);
        inner.atime_nsec.store(nsec, Ordering::Relaxed);
    }

    fn get_mtime(&self) -> (i64, i64) {
        let inner = self.inner.lock();
        (
            inner.mtime_sec.load(Ordering::Relaxed),
            inner.mtime_nsec.load(Ordering::Relaxed),
        )
    }

    fn set_mtime(&self, sec: i64, nsec: i64) {
        let inner = self.inner.lock();
        inner.mtime_sec.store(sec, Ordering::Relaxed);
        inner.mtime_nsec.store(nsec, Ordering::Relaxed);
    }

    fn get_ctime(&self) -> (i64, i64) {
        let inner = self.inner.lock();
        (
            inner.ctime_sec.load(Ordering::Relaxed),
            inner.ctime_nsec.load(Ordering::Relaxed),
        )
    }

    fn set_ctime(&self, sec: i64, nsec: i64) {
        let inner = self.inner.lock();
        inner.ctime_sec.store(sec, Ordering::Relaxed);
        inner.ctime_nsec.store(nsec, Ordering::Relaxed);
    }

    fn setxattr(&self, name: &str, value: &[u8], flags: i32) -> SyscallResult {
        if flags & !(XATTR_CREATE | XATTR_REPLACE) != 0 {
            return Err(SysError::EINVAL);
        }
        if name.is_empty() {
            return Err(SysError::ERANGE);
        }
        if name.len() > XATTR_NAME_MAX {
            return Err(SysError::ERANGE);
        }
        if value.len() > XATTR_SIZE_MAX {
            return Err(SysError::E2BIG);
        }
        check_xattr_write_allowed(self.get_fs_flags())?;
        if name.starts_with("user.") {
            check_user_xattr_support(self.get_mode())?;
        }

        let cpath = CString::new(self.path.clone()).map_err(|_| SysError::EINVAL)?;
        let cname = CString::new(name).map_err(|_| SysError::EINVAL)?;

        match flags {
            XATTR_CREATE => {
                if self.has_xattr(name)? {
                    return Err(SysError::EEXIST);
                }
            }
            XATTR_REPLACE => {
                if !self.has_xattr(name)? {
                    return Err(SysError::ENODATA);
                }
            }
            _ => {}
        }

        let ret = with_lwext4_path_lock_op(&self.path, Lwext4Op::Xattr, || unsafe {
            ext4_setxattr(
                cpath.as_ptr(),
                cname.as_ptr(),
                name.len(),
                value.as_ptr() as *const core::ffi::c_void,
                value.len(),
            )
        })?;
        if ret != 0 {
            return Err(super::lwext4_err_to_sys(ret));
        }
        Ok(0)
    }

    fn getxattr(&self, name: &str, buf: &mut [u8]) -> SyscallResult {
        if name.is_empty() {
            return Err(SysError::ERANGE);
        }
        let cpath = CString::new(self.path.clone()).map_err(|_| SysError::EINVAL)?;
        let cname = CString::new(name).map_err(|_| SysError::EINVAL)?;
        let mut data_size = 0usize;

        if !buf.is_empty() {
            let mut required_size = 0usize;
            let ret = with_lwext4_path_lock_op(&self.path, Lwext4Op::Xattr, || unsafe {
                ext4_getxattr(
                    cpath.as_ptr(),
                    cname.as_ptr(),
                    name.len(),
                    core::ptr::null_mut(),
                    0,
                    &mut required_size,
                )
            })?;
            if ret != 0 {
                return Err(super::lwext4_err_to_sys(ret));
            }
            if buf.len() < required_size {
                return Err(SysError::ERANGE);
            }
        }

        let ret = with_lwext4_path_lock_op(&self.path, Lwext4Op::Xattr, || unsafe {
            ext4_getxattr(
                cpath.as_ptr(),
                cname.as_ptr(),
                name.len(),
                buf.as_mut_ptr() as *mut core::ffi::c_void,
                buf.len(),
                &mut data_size,
            )
        })?;
        if ret != 0 {
            return Err(super::lwext4_err_to_sys(ret));
        }
        Ok(data_size as isize as usize)
    }

    fn listxattr(&self, buf: &mut [u8]) -> SyscallResult {
        let cpath = CString::new(self.path.clone()).map_err(|_| SysError::EINVAL)?;
        let mut ret_size = 0usize;
        let ret = with_lwext4_path_lock_op(&self.path, Lwext4Op::Xattr, || unsafe {
            ext4_listxattr(cpath.as_ptr(), core::ptr::null_mut(), 0, &mut ret_size)
        })?;
        if ret != 0 {
            return Err(super::lwext4_err_to_sys(ret));
        }
        if buf.is_empty() {
            return Ok(ret_size);
        }
        if buf.len() < ret_size {
            return Err(SysError::ERANGE);
        }

        let ret = with_lwext4_path_lock_op(&self.path, Lwext4Op::Xattr, || unsafe {
            ext4_listxattr(
                cpath.as_ptr(),
                buf.as_mut_ptr() as *mut core::ffi::c_char,
                buf.len(),
                &mut ret_size,
            )
        })?;
        if ret != 0 {
            return Err(super::lwext4_err_to_sys(ret));
        }
        if !buf.is_empty() && buf.len() < ret_size {
            return Err(SysError::ERANGE);
        }
        Ok(ret_size)
    }

    fn removexattr(&self, name: &str) -> SyscallResult {
        if name.is_empty() {
            return Err(SysError::ERANGE);
        }
        let cpath = CString::new(self.path.clone()).map_err(|_| SysError::EINVAL)?;
        let cname = CString::new(name).map_err(|_| SysError::EINVAL)?;
        let ret = with_lwext4_path_lock_op(&self.path, Lwext4Op::Xattr, || unsafe {
            ext4_removexattr(cpath.as_ptr(), cname.as_ptr(), name.len())
        })?;
        if ret != 0 {
            return Err(super::lwext4_err_to_sys(ret));
        }
        Ok(0)
    }
}

/// translate between InodeTypes and InodeMode
impl InodeMode {
    /// Convert an InodeTypes to an InodeMode, setting the type bits and permission bits.
    pub fn from_inode_type(itype: InodeTypes) -> Self {
        let perm_mode = InodeMode::OWNER_MASK | InodeMode::GROUP_MASK | InodeMode::OTHER_MASK;
        let file_mode = match itype {
            InodeTypes::EXT4_DE_DIR => InodeMode::DIR,
            InodeTypes::EXT4_DE_REG_FILE => InodeMode::FILE,
            InodeTypes::EXT4_DE_CHRDEV => InodeMode::CHAR,
            InodeTypes::EXT4_DE_FIFO => InodeMode::FIFO,
            InodeTypes::EXT4_DE_BLKDEV => InodeMode::BLOCK,
            InodeTypes::EXT4_DE_SOCK => InodeMode::SOCKET,
            InodeTypes::EXT4_DE_SYMLINK => InodeMode::LINK,
            _ => InodeMode::TYPE_MASK,
        };
        file_mode | perm_mode
    }
    /// Convert an InodeMode to an InodeTypes, extracting the type bits and ignoring the permission bits.
    pub fn to_inode_type(self) -> InodeTypes {
        match self.get_type() {
            InodeMode::DIR => InodeTypes::EXT4_DE_DIR,
            InodeMode::FILE => InodeTypes::EXT4_DE_REG_FILE,
            InodeMode::CHAR => InodeTypes::EXT4_DE_CHRDEV,
            InodeMode::FIFO => InodeTypes::EXT4_DE_FIFO,
            InodeMode::BLOCK => InodeTypes::EXT4_DE_BLKDEV,
            InodeMode::SOCKET => InodeTypes::EXT4_DE_SOCK,
            InodeMode::LINK => InodeTypes::EXT4_DE_SYMLINK,
            _ => InodeTypes::EXT4_DE_UNKNOWN,
        }
    }
}
