use crate::alloc::string::ToString;
use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::File;
use crate::fs::tmpfs::file::TempFile;
use crate::fs::tmpfs::inode::TempInode;
use crate::fs::vfs::FileInner;
use crate::fs::vfs::Inode;
use crate::fs::vfs::OpenFlags;
use crate::fs::vfs::kstat::Kstat;
use crate::fs::vfs::{Dentry, DentryInner, dcache::GLOBAL_DCACHE, inode::InodeMode};
use alloc::ffi::CString;
use alloc::format;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::AtomicU64;
use core::sync::atomic::Ordering;
use log::*;
use polyhal::common::FrameTracker;
use spin::{Mutex, MutexGuard};

use crate::fs::{Ext4Inode, InodeTypes};

///remove the dentry with the name, if the flag has AT_REMOVEDIR, then remove the directory, otherwise remove the file
pub const AT_REMOVEDIR: u32 = 0x200;
///
pub const DT_UNKNOWN: u8 = 0;
///
pub const DT_DIR: u8 = 4;
///
pub const DT_REG: u8 = 8;

#[allow(unused)]
///
pub struct TempDentry {
    inner: DentryInner,
    /// The self_weak field is designed to allow a Dentry to correctly set the parent reference
    /// when creating child Dentry instances
    self_weak: Weak<TempDentry>,
}

impl Drop for TempDentry {
    fn drop(&mut self) {
        let Some(inode) = self.inner.inode.lock().clone() else {
            return;
        };
        if inode.get_nlink() != 0 {
            return;
        }
        if let Some(cache_inode_id) = inode.cache_inode_id() {
            crate::fs::page::pagecache::PAGE_CACHE
                .lock()
                .remove_inode_pages(cache_inode_id);
        }
    }
}

struct BindMountFile {
    inner: Mutex<FileInner>,
    source: Arc<dyn File>,
}

impl BindMountFile {
    fn new(dentry: Arc<dyn Dentry>, source: Arc<dyn File>, flags: OpenFlags) -> Self {
        Self {
            inner: Mutex::new(FileInner {
                offset: source.get_offset(),
                dentry,
                flags,
            }),
            source,
        }
    }
}

impl File for BindMountFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        self.inner.lock()
    }

    fn readable(&self) -> bool {
        self.source.readable()
    }

    fn writable(&self) -> bool {
        self.source.writable()
    }

    fn read(&self, buf: crate::mm::UserBuffer) -> SysResult<usize> {
        let mut inner = self.inner.lock();
        self.source.set_offset(inner.offset);
        let ret = self.source.read(buf);
        inner.offset = self.source.get_offset();
        ret
    }

    fn write(&self, buf: crate::mm::UserBuffer) -> SysResult<usize> {
        let mut inner = self.inner.lock();
        self.source.set_offset(inner.offset);
        let ret = self.source.write(buf);
        inner.offset = self.source.get_offset();
        ret
    }

    fn write_at(&self, offset: usize, buf: crate::mm::UserBuffer) -> SysResult<usize> {
        self.source.write_at(offset, buf)
    }

    fn read_at_direct(&self, offset: usize, buf: &mut [u8]) -> SysResult<usize> {
        self.source.read_at_direct(offset, buf)
    }

    fn write_at_direct(&self, offset: usize, buf: &[u8]) -> SysResult<usize> {
        self.source.write_at_direct(offset, buf)
    }

    fn open(&self) -> SyscallResult {
        self.source.open()
    }

    fn release(&self) -> SyscallResult {
        self.source.release()
    }

    fn seek(&self, new_offset: usize) -> SysResult<usize> {
        self.source.seek(new_offset)?;
        self.set_offset(new_offset);
        Ok(new_offset)
    }

    fn ls(&self) -> Vec<(String, u64, u8)> {
        self.source.ls()
    }

    fn is_append(&self) -> bool {
        self.source.is_append()
    }

    fn set_status_flags(&self, flags: u32) {
        self.source.set_status_flags(flags);
        let mut inner = self.inner.lock();
        let access_mode = inner.flags.bits() & 0o3;
        let settable = OpenFlags::O_APPEND | OpenFlags::O_NONBLOCK | OpenFlags::O_NOATIME;
        inner.flags = OpenFlags::from_bits_truncate(access_mode | (flags & settable.bits()));
    }

    fn get_offset(&self) -> usize {
        self.inner.lock().offset
    }

    fn set_offset(&self, new_offset: usize) {
        self.inner.lock().offset = new_offset;
        self.source.set_offset(new_offset);
    }

    fn get_stat(&self, stat: &mut Kstat) -> SysResult<()> {
        self.source.get_stat(stat)
    }

    fn flush(&self) {
        self.source.flush();
    }

    fn flush_pages(&self, max_pages: usize) -> (usize, bool) {
        self.source.flush_pages(max_pages)
    }

    fn get_cache_frame(&self, page_id: usize) -> Option<Arc<FrameTracker>> {
        self.source.get_cache_frame(page_id)
    }

    fn read_all(&self) -> Vec<u8> {
        self.source.read_all()
    }

    fn ioctl(&self, request: usize, argp: usize) -> SyscallResult {
        self.source.ioctl(request, argp)
    }

    fn truncate(&self, size: u64) -> SyscallResult {
        self.source.truncate(size)
    }
}

impl TempDentry {
    ///
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>) -> Arc<dyn Dentry> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|me: &Weak<TempDentry>| Self {
            inner: DentryInner::new(name, parent_weak.clone()),
            self_weak: me.clone(),
        })
    }

    fn clone_subtree(
        name: &str,
        parent: Arc<dyn Dentry>,
        source: Arc<dyn Dentry>,
    ) -> SysResult<Arc<dyn Dentry>> {
        let new_dentry = TempDentry::new(name, Some(parent));
        let inode = source.get_inode().ok_or(SysError::ENOENT)?;
        new_dentry.set_inode(inode);

        for (child_name, child) in source.children() {
            let new_child = Self::clone_subtree(&child_name, new_dentry.clone(), child)?;
            let child_path = new_child.path();
            new_dentry.add_child(new_child.clone());
            GLOBAL_DCACHE.insert(child_path, new_child);
        }

        Ok(new_dentry)
    }

    fn proxy_child(&self, name: &str, source_child: Arc<dyn Dentry>) -> SysResult<Arc<dyn Dentry>> {
        let my_arc = self.self_weak.upgrade().ok_or(SysError::ENOENT)?;
        let child = TempDentry::new(name, Some(my_arc as Arc<dyn Dentry>));
        let inode = source_child.get_inode().ok_or(SysError::ENOENT)?;
        child.set_inode(inode);
        child.bind_mount_dentry(source_child);
        self.inner
            .children
            .lock()
            .insert(name.to_string(), child.clone());
        GLOBAL_DCACHE.insert(child.path(), child.clone());
        Ok(child)
    }
}

impl Dentry for TempDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }
    fn name(&self) -> &str {
        &self.inner.name
    }
    fn parent(&self) -> Option<Arc<dyn Dentry>> {
        self.inner.parent.as_ref().and_then(|p| p.upgrade())
    }

    fn path(&self) -> String {
        let Some(parent) = self.parent() else {
            return String::from("/");
        };

        let parent_path = parent.path();
        if parent_path == "/" {
            parent_path + self.name()
        } else {
            parent_path + "/" + self.name()
        }
    }

    /// find the child dentry by the name, return Err(SysError::ENOENT) if not found
    fn find(&self, name: &str) -> SysResult<Arc<dyn Dentry>> {
        let children = self.inner.children.lock();
        if let Some(child) = children.get(name).cloned() {
            return Ok(child);
        }
        drop(children);
        if let Some(bdentry) = self.inner.bdentry.lock().clone() {
            if let Ok(source_child) = bdentry.find(name) {
                return self.proxy_child(name, source_child);
            }
        }
        Err(SysError::ENOENT)
    }

    fn create(&self, name: &str, mode: InodeMode) -> SysResult<Arc<dyn Dentry>> {
        if let Some(source_dentry) = self.inner.bdentry.lock().clone() {
            let source_child = source_dentry.create(name, mode)?;
            return self.proxy_child(name, source_child);
        }

        let mut children = self.inner.children.lock();
        if children.contains_key(name) {
            return Err(SysError::EEXIST);
        }
        let my_arc = self.self_weak.upgrade().unwrap();
        let new_dentry = TempDentry::new(name, Some(my_arc as Arc<dyn Dentry>));
        let child_inode = Arc::new(TempInode::new(mode));
        new_dentry.set_inode(child_inode);
        children.insert(name.to_string(), new_dentry.clone());
        let target_path = format!("{}/{}", self.path().trim_end_matches('/'), name);
        GLOBAL_DCACHE.insert(target_path, new_dentry.clone());
        Ok(new_dentry)
    }

    /// list all the children of the current dentry
    /// return name and ino and type
    // fn ls(&self) -> Vec<(String, usize, InodeMode)> {
    //     let children = self.inner.children.lock();
    //     let mut entries = Vec::new();

    //     for (name, child_dentry) in children.iter() {
    //         let inode = child_dentry.get_inode().unwrap();
    //         // 获取你存在 TmpfsInode 里的信息
    //         let ino = inode.get_ino();
    //         let dt_mode = inode.get_mode(); // 这里返回 DT_DIR 或 DT_REG

    //         entries.push((name.clone(), ino, dt_mode));
    //     }
    //     entries
    // }

    fn unlink(&self, name: &str, flags: u32) -> SyscallResult {
        if let Some(source_dentry) = self.inner.bdentry.lock().clone() {
            let child = self.find(name)?;
            let target_path = child.path();
            source_dentry.unlink(name, flags)?;
            self.inner.children.lock().remove(name);
            GLOBAL_DCACHE.remove_subtree(&target_path);
            return Ok(0);
        }

        let is_rmdir = flags & AT_REMOVEDIR != 0;
        let mut children = self.inner.children.lock();
        let child = match children.get(name) {
            Some(c) => c.clone(),
            None => return Err(SysError::ENOENT),
        };
        let inode = match child.get_inode() {
            Some(i) => i,
            None => return Err(SysError::ENOENT),
        };
        let is_dir = inode.get_mode().get_type() == InodeMode::DIR;
        if is_rmdir && !is_dir {
            return Err(SysError::ENOTDIR);
        }
        if !is_rmdir && is_dir {
            return Err(SysError::EISDIR);
        }
        if is_dir {
            let child_children = child.get_dentryinner().children.lock();
            if !child_children.is_empty() {
                return Err(SysError::ENOTEMPTY);
            }
        }
        children.remove(name);
        inode.dec_nlink();
        let target_path = format!("{}/{}", self.path().trim_end_matches('/'), name);
        GLOBAL_DCACHE.remove(&target_path);
        Ok(0)
    }

    fn rename(
        &self,
        src_name: &str,
        dst_parent: Arc<dyn Dentry>,
        dst_name: &str,
    ) -> SysResult<usize> {
        let source_parent = self.inner.bdentry.lock().clone();
        if let Some(source_parent) = source_parent {
            if src_name.is_empty()
                || dst_name.is_empty()
                || src_name == "."
                || src_name == ".."
                || dst_name == "."
                || dst_name == ".."
            {
                return Err(SysError::EINVAL);
            }

            let old_dentry = self.find(src_name)?;
            let old_abs = old_dentry.path();
            let new_abs = if dst_parent.path() == "/" {
                format!("/{}", dst_name)
            } else {
                format!("{}/{}", dst_parent.path(), dst_name)
            };
            if old_abs == new_abs {
                return Ok(0);
            }

            let source_dst_parent = dst_parent
                .get_bind_dentry()
                .unwrap_or_else(|| dst_parent.clone());
            source_parent.rename(src_name, source_dst_parent.clone(), dst_name)?;
            self.inner.children.lock().remove(src_name);
            dst_parent.remove_child(dst_name);
            GLOBAL_DCACHE.remove_subtree(&old_abs);
            GLOBAL_DCACHE.remove_subtree(&new_abs);
            if dst_parent.get_bind_dentry().is_some() {
                if let Ok(source_child) = source_dst_parent.find(dst_name) {
                    let child = TempDentry::new(dst_name, Some(dst_parent.clone()));
                    let inode = source_child.get_inode().ok_or(SysError::ENOENT)?;
                    child.set_inode(inode);
                    child.bind_mount_dentry(source_child);
                    dst_parent.add_child(child.clone());
                    GLOBAL_DCACHE.insert(new_abs, child);
                }
            }
            return Ok(0);
        }

        if src_name.is_empty()
            || dst_name.is_empty()
            || src_name == "."
            || src_name == ".."
            || dst_name == "."
            || dst_name == ".."
        {
            return Err(SysError::EINVAL);
        }

        let old_dentry = {
            let children = self.inner.children.lock();
            children.get(src_name).cloned().ok_or(SysError::ENOENT)?
        };
        let old_abs = old_dentry.path();
        let new_abs = if dst_parent.path() == "/" {
            format!("/{}", dst_name)
        } else {
            format!("{}/{}", dst_parent.path(), dst_name)
        };
        if old_abs == new_abs {
            return Ok(0);
        }

        let dst_parent_inode = dst_parent.get_inode().ok_or(SysError::ENOENT)?;
        if dst_parent_inode.get_mode().get_type() != InodeMode::DIR {
            return Err(SysError::ENOTDIR);
        }

        let old_inode = old_dentry.get_inode().ok_or(SysError::ENOENT)?;
        let old_is_dir = old_inode.get_mode().get_type() == InodeMode::DIR;
        let dst_parent_abs = dst_parent.path();
        if old_is_dir
            && (dst_parent_abs == old_abs
                || dst_parent_abs.starts_with(&format!("{}/", old_abs.trim_end_matches('/'))))
        {
            return Err(SysError::EINVAL);
        }

        if let Ok(existing) = dst_parent.find(dst_name) {
            let existing_inode = existing.get_inode().ok_or(SysError::ENOENT)?;
            let existing_is_dir = existing_inode.get_mode().get_type() == InodeMode::DIR;
            if old_is_dir && !existing_is_dir {
                return Err(SysError::ENOTDIR);
            }
            if !old_is_dir && existing_is_dir {
                return Err(SysError::EISDIR);
            }
            if existing_is_dir && !existing.children().is_empty() {
                return Err(SysError::ENOTEMPTY);
            }
            dst_parent.remove_child(dst_name);
            existing_inode.dec_nlink();
            GLOBAL_DCACHE.remove_subtree(&new_abs);
        }

        let new_dentry = Self::clone_subtree(dst_name, dst_parent.clone(), old_dentry)?;
        self.inner.children.lock().remove(src_name);
        dst_parent.add_child(new_dentry.clone());
        GLOBAL_DCACHE.remove_subtree(&old_abs);
        GLOBAL_DCACHE.remove_subtree(&new_abs);
        GLOBAL_DCACHE.insert(new_abs, new_dentry);
        Ok(0)
    }

    fn link(&self, new_name: &str, old_dentry: Arc<dyn Dentry>) -> SyscallResult {
        let mut children = self.inner.children.lock();
        if children.contains_key(new_name) {
            return Err(SysError::EEXIST);
        }
        let old_inode = match old_dentry.get_inode() {
            Some(i) => i,
            None => return Err(SysError::ENOENT),
        };
        if !old_inode.get_mode().contains(InodeMode::FILE) {
            return Err(SysError::EINVAL);
        }
        let my_arc = self.self_weak.upgrade().unwrap();
        let new_dentry = TempDentry::new(new_name, Some(my_arc as Arc<dyn Dentry>));
        new_dentry.set_inode(old_inode.clone());
        old_inode.inc_nlink();
        children.insert(new_name.to_string(), new_dentry.clone());
        let new_path = format!("{}/{}", self.path().trim_end_matches('/'), new_name);
        GLOBAL_DCACHE.insert(new_path, new_dentry);
        Ok(0)
    }

    fn symlink(&self, name: &str, target: &str) -> SyscallResult {
        let mut children = self.inner.children.lock();
        if children.contains_key(name) {
            return Err(SysError::EEXIST);
        }
        let my_arc = self.self_weak.upgrade().unwrap();
        let new_dentry = TempDentry::new(name, Some(my_arc as Arc<dyn Dentry>));
        let symlink_inode = Arc::new(TempInode::new_symlink(target));
        new_dentry.set_inode(symlink_inode);
        children.insert(name.to_string(), new_dentry.clone());
        let new_path = format!("{}/{}", self.path().trim_end_matches('/'), name);
        GLOBAL_DCACHE.insert(new_path, new_dentry);
        Ok(0)
    }

    fn mknod(&self, name: &str, mode: InodeMode, dev: u32) -> SyscallResult {
        if let Some(source_dentry) = self.inner.bdentry.lock().clone() {
            let source_child = if mode.get_type() == InodeMode::FILE {
                source_dentry.create(name, mode)?
            } else {
                source_dentry.mknod(name, mode, dev)?;
                source_dentry.find(name)?
            };
            self.proxy_child(name, source_child)?;
            return Ok(0);
        }

        let mut children = self.inner.children.lock();
        if children.contains_key(name) {
            return Err(SysError::EEXIST);
        }
        let my_arc = self.self_weak.upgrade().unwrap();
        let new_dentry = TempDentry::new(name, Some(my_arc as Arc<dyn Dentry>));
        let child_inode = Arc::new(TempInode::new_dev(mode, dev as usize));
        new_dentry.set_inode(child_inode);
        children.insert(name.to_string(), new_dentry.clone());
        let target_path = format!("{}/{}", self.path().trim_end_matches('/'), name);
        GLOBAL_DCACHE.insert(target_path, new_dentry);
        Ok(0)
    }

    fn open(self: Arc<Self>, flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        let source_dentry = self.inner.bdentry.lock().clone();
        if let Some(source_dentry) = source_dentry {
            let inode = source_dentry.get_inode().ok_or(SysError::ENOENT)?;
            let flags_bits = flags.bits();
            let source_file = source_dentry.open(flags, inode.get_mode())?;
            return Ok(Arc::new(BindMountFile::new(
                self,
                source_file,
                OpenFlags::from_bits_truncate(flags_bits),
            )));
        }
        let (readable, writable) = flags.read_write();
        let append = flags.contains(OpenFlags::O_APPEND);
        Ok(Arc::new(TempFile::new(
            readable, writable, append, self, flags,
        )))
    }
}
