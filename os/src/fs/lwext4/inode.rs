use core::cell::RefCell;
use core::ptr::NonNull;
use core::sync::atomic::Ordering;

use crate::fs::page::pagecache::PAGE_CACHE;
use crate::fs::vfs::inode::InodeMode;
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
        ext4_setxattr, ext4_getxattr, ext4_listxattr, ext4_removexattr,
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
use crate::logging;

use super::disk::Disk;
use super::ext4::file::ExtFS;
#[allow(unused)]
///The inode of the Ext4 filesystem
/// the InodeInner is ino
/// this_type is the InodeTypes
pub struct Ext4Inode {
    inner: Mutex<InodeInner>,
    this_type: InodeTypes,
    path: String,
}

unsafe impl Send for Ext4Inode {}
unsafe impl Sync for Ext4Inode {}

impl Ext4Inode {
    ///
    pub fn new(ino: usize, types: InodeTypes, path: String) -> Self {
        info!("Inode new {:?} with ino {}", types, ino);
        let mode = InodeMode::from_inode_type(types.clone());

        Self {
            inner: Mutex::new(InodeInner::new(ino, 0, mode, 0)),
            this_type: types,
            path,
        }
    }
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
        // 截断文件时清除该 inode 的页缓存，避免旧页面被后续写入/读取误用
        PAGE_CACHE.lock().remove_inode_pages(self.get_ino());
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
            _ => panic!("Unsupported InodeType: {:?}", self.this_type),
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
        Some(self.get_ino())
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
        const XATTR_NAME_MAX: usize = 255;
        const XATTR_SIZE_MAX: usize = 65536;
        const XATTR_CREATE: i32 = 1;
        const XATTR_REPLACE: i32 = 2;

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

        let cpath = CString::new(self.path.clone()).map_err(|_| SysError::EINVAL)?;
        let cname = CString::new(name).map_err(|_| SysError::EINVAL)?;

        match flags {
            XATTR_CREATE => {
                let mut dummy = [0u8; 1];
                let mut data_size = 0usize;
                let ret = unsafe {
                    ext4_getxattr(
                        cpath.as_ptr(),
                        cname.as_ptr(),
                        name.len(),
                        dummy.as_mut_ptr() as *mut core::ffi::c_void,
                        0,
                        &mut data_size,
                    )
                };
                if ret == 0 {
                    return Err(SysError::EEXIST);
                }
            }
            XATTR_REPLACE => {
                let mut dummy = [0u8; 1];
                let mut data_size = 0usize;
                let ret = unsafe {
                    ext4_getxattr(
                        cpath.as_ptr(),
                        cname.as_ptr(),
                        name.len(),
                        dummy.as_mut_ptr() as *mut core::ffi::c_void,
                        0,
                        &mut data_size,
                    )
                };
                if ret != 0 {
                    let err = super::lwext4_err_to_sys(ret);
                    if err == SysError::ENODATA {
                        return Err(SysError::ENODATA);
                    }
                    // If some other error, fall through to setxattr which will also fail
                }
            }
            _ => {}
        }

        let ret = unsafe {
            ext4_setxattr(
                cpath.as_ptr(),
                cname.as_ptr(),
                name.len(),
                value.as_ptr() as *const core::ffi::c_void,
                value.len(),
            )
        };
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
        let ret = unsafe {
            ext4_getxattr(
                cpath.as_ptr(),
                cname.as_ptr(),
                name.len(),
                buf.as_mut_ptr() as *mut core::ffi::c_void,
                buf.len(),
                &mut data_size,
            )
        };
        if ret != 0 {
            return Err(super::lwext4_err_to_sys(ret));
        }
        Ok(data_size as isize as usize)
    }

    fn listxattr(&self, buf: &mut [u8]) -> SyscallResult {
        let cpath = CString::new(self.path.clone()).map_err(|_| SysError::EINVAL)?;
        let mut ret_size = 0usize;
        let ret = unsafe {
            ext4_listxattr(
                cpath.as_ptr(),
                buf.as_mut_ptr() as *mut core::ffi::c_char,
                buf.len(),
                &mut ret_size,
            )
        };
        if ret != 0 {
            return Err(super::lwext4_err_to_sys(ret));
        }
        Ok(ret_size)
    }

    fn removexattr(&self, name: &str) -> SyscallResult {
        if name.is_empty() {
            return Err(SysError::ERANGE);
        }
        let cpath = CString::new(self.path.clone()).map_err(|_| SysError::EINVAL)?;
        let cname = CString::new(name).map_err(|_| SysError::EINVAL)?;
        let ret = unsafe {
            ext4_removexattr(cpath.as_ptr(), cname.as_ptr(), name.len())
        };
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
    /// Get the type bits of the InodeMode, masking out the permission bits.
    pub fn get_type(self) -> Self {
        self.intersection(InodeMode::TYPE_MASK)
    }
}
