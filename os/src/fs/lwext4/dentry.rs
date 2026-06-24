use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::Ext4File;
use crate::fs::File;
use crate::fs::vfs::OpenFlags;
use alloc::ffi::CString;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use log::*;

use crate::fs::vfs::{Dentry, DentryInner, dcache::GLOBAL_DCACHE, inode::InodeMode};

use crate::fs::lwext4::ext4::{dir::ExtDir, file::ExtFS};
use crate::fs::lwext4::{lwext4_err_to_sys, with_lwext4_lock};

use crate::fs::vfs::inode::Inode;
use crate::fs::{Ext4Inode, InodeTypes};
use lwext4_rust::{Lwext4File, bindings::O_RDONLY};

///remove the dentry with the name, if the flag has AT_REMOVEDIR, then remove the directory, otherwise remove the file
pub const AT_REMOVEDIR: u32 = 0x200;
///
pub const DT_UNKNOWN: u8 = 0;
///
pub const DT_DIR: u8 = 4;
///
pub const DT_REG: u8 = 8;
///
pub const DT_LNK: u8 = 10;
///
pub struct Ext4Dentry {
    inner: DentryInner,
    /// The self_weak field is designed to allow a Dentry to correctly set the parent reference
    /// when creating child Dentry instances
    self_weak: Weak<Ext4Dentry>,
    mount_id: usize,
}

impl Ext4Dentry {
    ///
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>, mount_id: usize) -> Arc<dyn Dentry> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|me: &Weak<Ext4Dentry>| Self {
            inner: DentryInner::new(name, parent_weak.clone()),
            self_weak: me.clone(),
            mount_id,
        })
    }
}

impl Dentry for Ext4Dentry {
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
    /// find the child dentry by the name, return None if not found
    /// the name was not the absolute path
    /// use the lwext4 dir operations to find the child dentry, and then create a new dentry for it
    /// so the path will with the '/0' at the end
    fn find(&self, name: &str) -> SysResult<Arc<dyn Dentry>> {
        let clean_target = name.trim_matches(|c| c == '\0' || c == ' ');
        if let Some(child) = self.inner.children.lock().get(clean_target).cloned() {
            return Ok(child);
        }

        let current_dir_path = self.path();
        trace!(
            "lookup ext4 dir [{}] for [{}]",
            current_dir_path, clean_target
        );
        let path = match CString::new(current_dir_path.clone()) {
            Ok(path) => path,
            Err(_) => {
                warn!("invalid directory path contains NUL: {}", current_dir_path);
                return Err(SysError::ENOENT);
            }
        };
        let mut dir = match ExtDir::open(&path) {
            Ok(dir) => dir,
            Err(err) => {
                warn!(
                    "failed to open parent dir for find: path={}, err={:?}",
                    current_dir_path, err
                );
                return Err(SysError::ENOENT);
            }
        };
        while let Some(entry) = dir.next() {
            let entry_name = match entry.name() {
                Ok(name) => name,
                Err(_) => continue,
            };
            if entry_name == clean_target {
                let ino = entry.ino() as usize;
                let mut file_type = entry.file_type();
                let file_path = format!(
                    "{}/{}",
                    current_dir_path.trim_end_matches('/'),
                    clean_target
                );
                // 某些镜像目录项可能返回 UNKNOWN，做一次路径探测以恢复真实类型。
                if file_type == InodeTypes::EXT4_DE_UNKNOWN {
                    if let Ok(c_probe) = CString::new(file_path.clone()) {
                        if ExtDir::open(&c_probe).is_ok() {
                            file_type = InodeTypes::EXT4_DE_DIR;
                        } else {
                            // 尝试作为 symlink 探测：ext4_readlink 对非 symlink 会返回错误
                            let mut probe_buf = [0u8; 1];
                            if ExtFS::readlink(&c_probe, &mut probe_buf).is_ok() {
                                file_type = InodeTypes::EXT4_DE_SYMLINK;
                            } else {
                                file_type = InodeTypes::EXT4_DE_REG_FILE;
                            }
                        }
                    }
                }

                trace!("found {} in lwext4, type: {:?}", name, file_type);
                let child_inode = Arc::new(Ext4Inode::new(
                    ino,
                    file_type.clone(),
                    file_path.clone(),
                    self.mount_id,
                ));
                if file_type == InodeTypes::EXT4_DE_REG_FILE {
                    let mut tmp_file = Lwext4File::new(&file_path, file_type);
                    if with_lwext4_lock(|| tmp_file.file_open(&file_path, O_RDONLY)).is_ok() {
                        let real_size = tmp_file.file_desc.fsize as usize;
                        child_inode.set_size(real_size);
                        with_lwext4_lock(|| {
                            let _ = tmp_file.file_close();
                        });
                    }
                }
                let my_arc = match self.self_weak.upgrade() {
                    Some(arc) => arc,
                    None => {
                        warn!("dentry dropped while finding child: {}", clean_target);
                        return Err(SysError::ENOENT);
                    }
                };
                let new_dentry = Ext4Dentry::new(clean_target, Some(my_arc), self.mount_id);
                new_dentry.set_inode(child_inode);
                self.inner
                    .children
                    .lock()
                    .insert(clean_target.to_string(), new_dentry.clone());
                return Ok(new_dentry);
            }
        }
        Err(SysError::ENOENT)
    }

    /// create a new dentry with the name and type, and return it, if the dentry already exists, return Err
    fn create(&self, name: &str, mode: InodeMode) -> SysResult<Arc<dyn Dentry>> {
        info!("create {:?} on Ext4Dentry: {}", mode, name);
        let parent_path = self.path();
        let target_path = format!("{}/{}", parent_path.trim_end_matches('/'), name);
        let cpath = match CString::new(target_path.clone()) {
            Ok(path) => path,
            Err(_) => {
                error!(
                    "failed to create {}: invalid path contains NUL",
                    target_path
                );
                return Err(SysError::EINVAL);
            }
        };
        match mode.get_type() {
            InodeMode::DIR => ExtFS::create(&cpath)?,
            InodeMode::FILE => ExtFS::create_file(&cpath)?,
            InodeMode::LINK => {
                // symlink 内容在创建时由 symlink() 方法处理，create 不单独处理 LINK
                warn!("create called with LINK mode, use symlink() instead");
                return Err(SysError::EINVAL);
            }
            _ => {
                warn!("unsupported inode mode: {:?}", mode);
                return Err(SysError::EINVAL);
            }
        };
        // Apply permission bits (lwext4 create functions don't accept mode)
        let _ = ExtFS::mode_set(&cpath, mode.bits());
        let new_dentry = match self.find(name) {
            Ok(dentry) => dentry,
            Err(_) => {
                error!("created {} on disk but failed to find it", target_path);
                return Err(SysError::EIO);
            }
        };
        self.inner
            .children
            .lock()
            .insert(name.to_string(), new_dentry.clone());
        GLOBAL_DCACHE.insert(target_path, new_dentry.clone());
        Ok(new_dentry)
    }

    /// list all the children of the current dentry
    /// return name and ino and type
    fn ls(&self) -> Vec<(String, u64, u8)> {
        info!("call ls on {}", self.path());
        let cpath = CString::new(self.path()).unwrap();
        ExtDir::open(&cpath)
            .map(|mut dir| {
                let mut entries = Vec::new();
                while let Some(entry) = dir.next() {
                    if let Ok(name) = entry.name() {
                        let ino = entry.ino() as u64;
                        let ext4_type = entry.file_type();
                        let dt_type = match ext4_type as i32 {
                            1 => DT_REG,
                            2 => DT_DIR,
                            7 => DT_LNK,
                            _ => DT_UNKNOWN,
                        };
                        entries.push((name, ino, dt_type));
                    }
                }
                entries
            })
            .unwrap_or_default()
    }

    fn unlink(&self, name: &str, flags: u32) -> SyscallResult {
        let is_rmdir = flags & AT_REMOVEDIR != 0;
        let target_path = format!("{}/{}", self.path(), name);
        let target_dentry = match GLOBAL_DCACHE.get(&target_path) {
            Some(dentry) => dentry,
            None => {
                // rename 后缓存可能失效，cache miss 时回落到底层目录查找。
                match self.find(name) {
                    Ok(dentry) => {
                        GLOBAL_DCACHE.insert(target_path.clone(), dentry.clone());
                        dentry
                    }
                    Err(_) => {
                        warn!("dentry not found for path: {}", target_path);
                        return Err(SysError::ENOENT);
                    }
                }
            }
        };
        let inode = target_dentry.get_inode().unwrap();
        let is_dir = inode.get_types() == InodeTypes::EXT4_DE_DIR;
        if is_rmdir && !is_dir {
            warn!("unlink failed: {} is not a directory", target_path);
            return Err(SysError::ENOTDIR);
        } else if !is_rmdir && is_dir {
            warn!("unlink failed: {} is a directory", target_path);
            return Err(SysError::EISDIR);
        }
        let cpath = CString::new(target_path.clone()).unwrap();
        if is_rmdir {
            ExtFS::remove_dir(&cpath)?;
        } else {
            ExtFS::remove_file(&cpath)?;
        }
        inode.dec_nlink();
        self.inner.children.lock().remove(name);
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

        let old_dentry = self.find(src_name)?;
        let old_inode = old_dentry.get_inode().ok_or(SysError::ENOENT)?;
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
        }

        let c_old = CString::new(old_abs.clone()).map_err(|_| SysError::EINVAL)?;
        let c_new = CString::new(new_abs.clone()).map_err(|_| SysError::EINVAL)?;
        if old_is_dir {
            ExtFS::rename(&c_old, &c_new)?;
        } else {
            match ExtFS::rename_file(&c_old, &c_new) {
                Ok(()) => {}
                Err(SysError::ENOENT) => {
                    ExtFS::link(&c_old, &c_new).and_then(|_| ExtFS::remove_file(&c_old))?;
                }
                Err(err) => return Err(err),
            }
        }

        self.inner.children.lock().remove(src_name);
        dst_parent.remove_child(dst_name);
        GLOBAL_DCACHE.remove_subtree(&old_abs);
        GLOBAL_DCACHE.remove_subtree(&new_abs);
        Ok(0)
    }

    fn link(&self, new_name: &str, old_dentry: Arc<dyn Dentry>) -> SyscallResult {
        if old_dentry.get_inode().unwrap().get_types() != InodeTypes::EXT4_DE_REG_FILE {
            return Err(SysError::EINVAL);
        }
        let new_path = if self.path() == "/" {
            format!("/{}", new_name)
        } else {
            format!("{}/{}", self.path(), new_name)
        };
        let c_old = CString::new(old_dentry.path()).unwrap();
        let c_new = CString::new(new_path.clone()).unwrap();
        ExtFS::link(&c_old, &c_new)?;
        old_dentry.get_inode().unwrap().inc_nlink();
        let new_dentry = Ext4Dentry::new(
            new_name,
            Some(self.self_weak.upgrade().unwrap()),
            self.mount_id,
        );
        if let Some(inode) = old_dentry.get_inode() {
            new_dentry.set_inode(inode);
        }
        self.inner
            .children
            .lock()
            .insert(new_name.to_string(), new_dentry.clone());
        GLOBAL_DCACHE.insert(new_path, new_dentry);
        Ok(0)
    }
    fn symlink(&self, name: &str, target: &str) -> SyscallResult {
        let new_path = if self.path() == "/" {
            format!("/{}", name)
        } else {
            format!("{}/{}", self.path(), name)
        };
        let c_target = CString::new(target).map_err(|_| SysError::EINVAL)?;
        let c_new = CString::new(new_path.clone()).map_err(|_| SysError::EINVAL)?;
        ExtFS::symlink(&c_target, &c_new)?;
        let new_dentry =
            Ext4Dentry::new(name, Some(self.self_weak.upgrade().unwrap()), self.mount_id);
        let inode = Arc::new(Ext4Inode::new(
            0,
            InodeTypes::EXT4_DE_SYMLINK,
            new_path.clone(),
            self.mount_id,
        ));
        new_dentry.set_inode(inode);
        self.inner
            .children
            .lock()
            .insert(name.to_string(), new_dentry.clone());
        GLOBAL_DCACHE.insert(new_path, new_dentry);
        Ok(0)
    }
    fn mknod(&self, name: &str, mode: InodeMode, dev: u32) -> SyscallResult {
        let parent_path = self.path();
        let target_path = format!("{}/{}", parent_path.trim_end_matches('/'), name);
        let cpath = match CString::new(target_path.clone()) {
            Ok(path) => path,
            Err(_) => {
                error!("failed to mknod {}: invalid path contains NUL", target_path);
                return Err(SysError::EINVAL);
            }
        };

        let filetype = match mode.get_type() {
            InodeMode::CHAR => InodeTypes::EXT4_DE_CHRDEV,
            InodeMode::BLOCK => InodeTypes::EXT4_DE_BLKDEV,
            InodeMode::FIFO => InodeTypes::EXT4_DE_FIFO,
            InodeMode::SOCKET => InodeTypes::EXT4_DE_SOCK,
            _ => {
                warn!("mknod called with unsupported mode: {:?}", mode);
                return Err(SysError::EINVAL);
            }
        };
        let filetype_i32 = filetype.clone() as i32;

        let err = with_lwext4_lock(|| unsafe {
            lwext4_rust::bindings::ext4_mknod(cpath.as_ptr(), filetype_i32, dev)
        });
        if err != 0 {
            warn!(
                "ext4_mknod failed: path = {}, filetype = {:?}, dev = {}, error = {}",
                target_path, filetype, dev, err
            );
            return Err(lwext4_err_to_sys(err));
        }

        // Apply permission bits
        let _ = ExtFS::mode_set(&cpath, mode.bits());

        let new_dentry = match self.find(name) {
            Ok(dentry) => dentry,
            Err(_) => {
                error!("mknod {} on disk but failed to find it", target_path);
                return Err(SysError::EIO);
            }
        };
        if let Some(inode) = new_dentry.get_inode() {
            inode.set_rdev(dev as usize);
        }
        self.inner
            .children
            .lock()
            .insert(name.to_string(), new_dentry.clone());
        GLOBAL_DCACHE.insert(target_path, new_dentry.clone());
        Ok(0)
    }
    fn open(self: Arc<Self>, flags: OpenFlags, mode: InodeMode) -> SysResult<Arc<dyn File>> {
        let (readable, writable) = flags.read_write();
        let types = mode.to_inode_type();
        Ok(Arc::new(Ext4File::new(
            readable, writable, self, types, flags,
        )?))
    }
}
