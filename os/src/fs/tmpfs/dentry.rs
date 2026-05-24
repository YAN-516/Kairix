use alloc::ffi::CString;
use alloc::format;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::AtomicU64;
use core::sync::atomic::Ordering;
use crate::alloc::string::ToString;
use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::vfs::Inode;
use log::*;
use crate::fs::tmpfs::inode::TempInode;
use crate::fs::tmpfs::file::TempFile;
use crate::fs::vfs::OpenFlags;
use crate::fs::File;
use crate::fs::vfs::{
    dcache::GLOBAL_DCACHE, 
    inode::InodeMode, 
    Dentry, 
    DentryInner
};

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

impl TempDentry {
    ///
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>) -> Arc<dyn Dentry> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|me: &Weak<TempDentry>| {
            Self {
                inner: DentryInner::new(name, parent_weak.clone()),
                self_weak: me.clone(),
            }
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
}

impl Dentry for TempDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }
    fn name(&self) -> &str{
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
            if let Ok(child) = bdentry.find(name) {
                return Ok(child);
            }
        }
        Err(SysError::ENOENT)
    }

    fn create(&self, name: &str, mode: InodeMode) -> SysResult<Arc<dyn Dentry>> {
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
        let is_dir = inode.get_mode().contains(InodeMode::DIR);
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
        if !dst_parent_inode.get_mode().contains(InodeMode::DIR) {
            return Err(SysError::ENOTDIR);
        }

        let old_inode = old_dentry.get_inode().ok_or(SysError::ENOENT)?;
        let old_is_dir = old_inode.get_mode().contains(InodeMode::DIR);
        let dst_parent_abs = dst_parent.path();
        if old_is_dir
            && (dst_parent_abs == old_abs
                || dst_parent_abs.starts_with(&format!("{}/", old_abs.trim_end_matches('/'))))
        {
            return Err(SysError::EINVAL);
        }

        if let Ok(existing) = dst_parent.find(dst_name) {
            let existing_inode = existing.get_inode().ok_or(SysError::ENOENT)?;
            let existing_is_dir = existing_inode.get_mode().contains(InodeMode::DIR);
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
        let (readable, writable) = flags.read_write();
        let append = flags.contains(OpenFlags::O_APPEND);
        Ok(Arc::new(TempFile::new(readable, writable, append, self)))
    }
}
