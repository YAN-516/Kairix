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
        Arc::new_cyclic(|me: &Weak<Fat32Dentry>| {
            Self {
                inner: DentryInner::new(name, parent_weak),
                self_weak: me.clone(),
                rel_path,
                superblock,
            }
        })
    }

    fn sb(&self) -> SysResult<Arc<Fat32SuperBlock>> {
        self.superblock.upgrade().ok_or(SysError::EIO)
    }
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
                let child_rel = if self.rel_path.is_empty() {
                    clean_target.to_string()
                } else {
                    format!("{}/{}", self.rel_path, clean_target)
                };
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

        let mut entries = Vec::new();
        for entry in dir.iter() {
            match entry {
                Ok(e) => {
                    let name = e.file_name();
                    if name == "." || name == ".." {
                        continue;
                    }
                    let size = e.len();
                    let dt = if e.is_dir() { DT_DIR } else { DT_REG };
                    entries.push((name, size, dt));
                }
                Err(_) => continue,
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
        let my_arc = self
            .self_weak
            .upgrade()
            .ok_or(SysError::ENOENT)?;
        let child_rel = if self.rel_path.is_empty() {
            name.to_string()
        } else {
            format!("{}/{}", self.rel_path, name)
        };
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

    fn rename(&self, _src_name: &str, _dst_parent: Arc<dyn Dentry>, _dst_name: &str) -> SysResult<usize> {
        Err(SysError::EINVAL)
    }

    fn open(self: Arc<Self>, flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        let (readable, writable) = flags.read_write();
        let _inode = self.get_inode().ok_or(SysError::ENOENT)?;
        Ok(Arc::new(Fat32File::new(
            readable,
            writable,
            self.clone(),
            self.rel_path.clone(),
            self.superblock.clone(),
        )))
    }
}
