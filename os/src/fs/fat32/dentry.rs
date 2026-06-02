use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::fat32::fat32_error_to_sys;
use crate::fs::fat32::file::Fat32File;
use crate::fs::fat32::inode::Fat32Inode;
use crate::fs::fat32::superblock::Fat32SuperBlock;
use crate::fs::vfs::dcache::GLOBAL_DCACHE;
use crate::fs::vfs::inode::{inode_alloc, InodeMode};
use crate::fs::vfs::{Dentry, DentryInner, OpenFlags};
use crate::fs::File;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use log::info;

pub const AT_REMOVEDIR: u32 = 0x200;
pub const DT_UNKNOWN: u8 = 0;
pub const DT_DIR: u8 = 4;
pub const DT_REG: u8 = 8;
pub const DT_LNK: u8 = 10;

pub struct Fat32Dentry {
    inner: DentryInner,
    self_weak: Weak<Fat32Dentry>,
    rel_path: String,
    superblock: Weak<Fat32SuperBlock>,
}

impl Fat32Dentry {
    pub fn new(
        name: &str,
        parent: Option<Arc<dyn Dentry>>,
        rel_path: String,
        superblock: Weak<Fat32SuperBlock>,
    ) -> Arc<dyn Dentry> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|me: &Weak<Fat32Dentry>| Self {
            inner: DentryInner::new(name, parent_weak),
            self_weak: me.clone(),
            rel_path,
            superblock,
        })
    }

    fn sb(&self) -> SysResult<Arc<Fat32SuperBlock>> {
        self.superblock.upgrade().ok_or(SysError::EIO)
    }

    fn child_rel_path(parent_rel: &str, name: &str) -> String {
        if parent_rel.is_empty() {
            name.to_string()
        } else {
            format!("{}/{}", parent_rel, name)
        }
    }

    fn clone_subtree(
        name: &str,
        parent: Arc<dyn Dentry>,
        rel_path: String,
        source: Arc<dyn Dentry>,
        superblock: Weak<Fat32SuperBlock>,
    ) -> SysResult<Arc<dyn Dentry>> {
        let new_dentry = Fat32Dentry::new(name, Some(parent), rel_path.clone(), superblock.clone());
        let inode = source.get_inode().ok_or(SysError::ENOENT)?;
        new_dentry.set_inode(inode);

        for (child_name, child) in source.children() {
            let child_rel = Self::child_rel_path(&rel_path, &child_name);
            let new_child = Self::clone_subtree(
                &child_name,
                new_dentry.clone(),
                child_rel,
                child,
                superblock.clone(),
            )?;
            let child_path = new_child.path();
            new_dentry.add_child(new_child.clone());
            GLOBAL_DCACHE.insert(child_path, new_child);
        }

        Ok(new_dentry)
    }
}

fn fat32_rel_path_for_abs(sb: &Fat32SuperBlock, abs_path: &str) -> SysResult<String> {
    let mount = sb.mount_point.trim_end_matches('/');
    if mount.is_empty() || mount == "/" {
        return Ok(abs_path.trim_start_matches('/').to_string());
    }
    if abs_path == mount {
        return Ok(String::new());
    }
    abs_path
        .strip_prefix(mount)
        .and_then(|rest| rest.strip_prefix('/'))
        .map(ToString::to_string)
        .ok_or(SysError::EXDEV)
}

impl Dentry for Fat32Dentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }

    fn find(&self, name: &str) -> SysResult<Arc<dyn Dentry>> {
        let clean_target = name.trim_matches(|c| c == '\0' || c == ' ');
        if clean_target.is_empty() {
            return Err(SysError::ENOENT);
        }

        {
            let children = self.inner.children.lock();
            if let Some(child) = children.get(clean_target) {
                return Ok(child.clone());
            }
        }

        let sb = self.sb()?;
        let fs = sb.fs.lock();
        let root = fs.root_dir();
        let dir = if self.rel_path.is_empty() {
            root
        } else {
            root.open_dir(&self.rel_path).map_err(fat32_error_to_sys)?
        };

        for entry in dir.iter() {
            let e = entry.map_err(fat32_error_to_sys)?;
            if e.file_name() == clean_target {
                let is_dir = e.is_dir();
                let size = e.len() as usize;
                let ino = inode_alloc();
                let mode = if is_dir {
                    InodeMode::DIR | InodeMode::from_bits_truncate(0o777)
                } else {
                    InodeMode::FILE | InodeMode::from_bits_truncate(0o644)
                };
                let child_rel = Self::child_rel_path(&self.rel_path, clean_target);
                let inode = Arc::new(Fat32Inode::new(
                    ino,
                    size,
                    mode,
                    child_rel.clone(),
                    is_dir,
                    self.superblock.clone(),
                ));
                let my_arc = self.self_weak.upgrade().ok_or(SysError::ENOENT)?;
                let child_dentry = Fat32Dentry::new(
                    clean_target,
                    Some(my_arc),
                    child_rel,
                    self.superblock.clone(),
                );
                child_dentry.set_inode(inode);
                self.add_child(child_dentry.clone());
                return Ok(child_dentry);
            }
        }
        Err(SysError::ENOENT)
    }

    fn create(&self, name: &str, mode: InodeMode) -> SysResult<Arc<dyn Dentry>> {
        let sb = self.sb()?;
        {
            let fs = sb.fs.lock();
            let root = fs.root_dir();
            let dir = if self.rel_path.is_empty() {
                root
            } else {
                root.open_dir(&self.rel_path).map_err(fat32_error_to_sys)?
            };

            let is_dir = mode.get_type() == InodeMode::DIR;
            if is_dir {
                dir.create_dir(name).map_err(fat32_error_to_sys)?;
            } else {
                let mut f = dir.create_file(name).map_err(fat32_error_to_sys)?;
                f.truncate().map_err(fat32_error_to_sys)?;
            }
        }

        let child = self.find(name)?;
        if let Some(inode) = child.get_inode() {
            inode.set_mode(mode);
        }
        let abs = self.path();
        let child_abs = if abs == "/" {
            format!("/{}", name)
        } else {
            format!("{}/{}", abs, name)
        };
        GLOBAL_DCACHE.insert(child_abs, child.clone());
        Ok(child)
    }

    fn ls(&self) -> Vec<(String, u64, u8)> {
        let sb = match self.sb() {
            Ok(sb) => sb,
            Err(_) => return Vec::new(),
        };

        let mut entries = Vec::new();
        {
            let fs = sb.fs.lock();
            let root = fs.root_dir();
            let dir = if self.rel_path.is_empty() {
                root
            } else {
                match root.open_dir(&self.rel_path) {
                    Ok(d) => d,
                    Err(_) => return Vec::new(),
                }
            };

            for entry in dir.iter() {
                match entry {
                    Ok(e) => {
                        let name = e.file_name();
                        if name == "." || name == ".." {
                            continue;
                        }
                        let dt = if e.is_dir() { DT_DIR } else { DT_REG };
                        entries.push((name, 0, dt));
                    }
                    Err(_) => continue,
                }
            }
        }

        for (name, ino, _) in entries.iter_mut() {
            if let Ok(child) = self.find(name) {
                if let Some(inode) = child.get_inode() {
                    *ino = inode.get_ino() as u64;
                }
            }
        }
        for (name, child) in self.children() {
            if entries.iter().any(|(entry_name, _, _)| entry_name == &name) {
                continue;
            }
            if let Some(inode) = child.get_inode() {
                let inode_type = inode.get_mode().get_type();
                let dt = if inode_type == InodeMode::DIR {
                    DT_DIR
                } else if inode_type == InodeMode::LINK {
                    DT_LNK
                } else if inode_type == InodeMode::FILE {
                    DT_REG
                } else {
                    DT_UNKNOWN
                };
                entries.push((name, inode.get_ino() as u64, dt));
            }
        }
        entries
    }

    fn unlink(&self, name: &str, flags: u32) -> SyscallResult {
        let is_rmdir = flags & AT_REMOVEDIR != 0;
        let child = self.find(name)?;
        let inode = child.get_inode().ok_or(SysError::ENOENT)?;
        let inode_type = inode.get_mode().get_type();
        let is_dir = inode_type == InodeMode::DIR;
        let is_link = inode_type == InodeMode::LINK;
        if is_rmdir && !is_dir {
            return Err(SysError::ENOTDIR);
        }
        if !is_rmdir && is_dir {
            return Err(SysError::EISDIR);
        }

        if !is_link {
            let sb = self.sb()?;
            {
                let fs = sb.fs.lock();
                let root = fs.root_dir();
                let dir = if self.rel_path.is_empty() {
                    root
                } else {
                    root.open_dir(&self.rel_path).map_err(fat32_error_to_sys)?
                };
                dir.remove(name).map_err(fat32_error_to_sys)?;
            }
        }

        inode.dec_nlink();
        let child_abs = child.path();
        GLOBAL_DCACHE.remove(&child_abs);
        self.remove_child(name);
        Ok(0)
    }

    fn link(&self, _new_name: &str, _old_dentry: Arc<dyn Dentry>) -> SyscallResult {
        Err(SysError::EPERM)
    }

    fn symlink(&self, name: &str, target: &str) -> SyscallResult {
        let mut children = self.inner.children.lock();
        if children.contains_key(name) {
            return Err(SysError::EEXIST);
        }
        let my_arc = self.self_weak.upgrade().ok_or(SysError::ENOENT)?;
        let child_rel = Self::child_rel_path(&self.rel_path, name);
        let new_dentry = Fat32Dentry::new(
            name,
            Some(my_arc as Arc<dyn Dentry>),
            child_rel.clone(),
            self.superblock.clone(),
        );
        let symlink_inode = Arc::new(Fat32Inode::new_symlink(
            target,
            child_rel,
            self.superblock.clone(),
        ));
        new_dentry.set_inode(symlink_inode);
        children.insert(name.to_string(), new_dentry.clone());
        let new_path = format!("{}/{}", self.path().trim_end_matches('/'), name);
        GLOBAL_DCACHE.insert(new_path, new_dentry);
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
        let old_is_link = old_inode.get_mode().get_type() == InodeMode::LINK;
        let dst_parent_abs = dst_parent.path();
        if old_is_dir
            && (dst_parent_abs == old_abs
                || dst_parent_abs.starts_with(&format!("{}/", old_abs.trim_end_matches('/'))))
        {
            return Err(SysError::EINVAL);
        }

        let existing_dentry = dst_parent.find(dst_name).ok();
        if let Some(existing) = existing_dentry.as_ref() {
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
        }

        let sb = self.sb()?;
        let dst_parent_rel = fat32_rel_path_for_abs(&sb, &dst_parent_abs)?;
        let remove_existing_on_disk = existing_dentry
            .as_ref()
            .and_then(|dentry| dentry.get_inode())
            .is_some_and(|inode| inode.get_mode().get_type() != InodeMode::LINK);
        if !old_is_link || remove_existing_on_disk {
            let fs = sb.fs.lock();
            let root = fs.root_dir();
            let src_dir = if self.rel_path.is_empty() {
                root.clone()
            } else {
                root.open_dir(&self.rel_path).map_err(fat32_error_to_sys)?
            };
            let dst_dir = if dst_parent_rel.is_empty() {
                root
            } else {
                root.open_dir(&dst_parent_rel).map_err(fat32_error_to_sys)?
            };
            if remove_existing_on_disk {
                dst_dir.remove(dst_name).map_err(fat32_error_to_sys)?;
            }
            if !old_is_link {
                src_dir
                    .rename(src_name, &dst_dir, dst_name)
                    .map_err(fat32_error_to_sys)?;
            }
        }

        if let Some(existing) = existing_dentry {
            if let Some(inode) = existing.get_inode() {
                inode.dec_nlink();
            }
        }
        let new_rel = if dst_parent_rel.is_empty() {
            dst_name.to_string()
        } else {
            format!("{}/{}", dst_parent_rel, dst_name)
        };
        self.remove_child(src_name);
        dst_parent.remove_child(dst_name);
        GLOBAL_DCACHE.remove_subtree(&old_abs);
        GLOBAL_DCACHE.remove_subtree(&new_abs);
        let new_dentry = Self::clone_subtree(
            dst_name,
            dst_parent.clone(),
            new_rel,
            old_dentry,
            self.superblock.clone(),
        )?;
        dst_parent.add_child(new_dentry.clone());
        GLOBAL_DCACHE.insert(new_abs, new_dentry);
        Ok(0)
    }

    fn mknod(&self, name: &str, mode: InodeMode, _dev: u32) -> SyscallResult {
        if mode.get_type() == InodeMode::FILE {
            self.create(name, mode).map(|_| 0)
        } else {
            Err(SysError::EPERM)
        }
    }

    fn open(self: Arc<Self>, flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        let (readable, writable) = flags.read_write();
        let inode = self.get_inode().ok_or(SysError::ENOENT)?;
        if flags.contains(OpenFlags::O_TRUNC) && inode.get_mode().get_type() == InodeMode::FILE {
            let sb = self.sb()?;
            {
                let fs = sb.fs.lock();
                let root = fs.root_dir();
                let mut fat_file = root.open_file(&self.rel_path).map_err(fat32_error_to_sys)?;
                fat_file.truncate().map_err(fat32_error_to_sys)?;
            }
            inode.truncate(0)?;
        }
        Ok(Arc::new(Fat32File::new(
            readable,
            writable,
            self.clone(),
            self.rel_path.clone(),
            self.superblock.clone(),
            flags,
        )))
    }
}
