use alloc::ffi::CString;
use alloc::format;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::vfs::OpenFlags;
use log::*;
use crate::fs::File;
use crate::fs::Ext4File;

use crate::fs::vfs::{
    dcache::GLOBAL_DCACHE, 
    inode::InodeMode, 
    Dentry, 
    DentryInner
};

use crate::fs::lwext4::ext4::{
    dir::ExtDir, 
    file::ExtFS
};
use crate::fs::lwext4::lwext4_err_to_sys;

use crate::fs::{Ext4Inode, InodeTypes};
use lwext4_rust::{Lwext4File, bindings::O_RDONLY};
use crate::fs::vfs::inode::Inode;

///remove the dentry with the name, if the flag has AT_REMOVEDIR, then remove the directory, otherwise remove the file
pub const AT_REMOVEDIR: u32 = 0x200;
/// 
pub const DT_UNKNOWN: u8 = 0;
///
pub const DT_DIR: u8 = 4;
///
pub const DT_REG: u8 = 8;
///
pub struct Ext4Dentry {
    inner: DentryInner,
    /// The self_weak field is designed to allow a Dentry to correctly set the parent reference 
    /// when creating child Dentry instances
    self_weak: Weak<Ext4Dentry>,
}

impl Ext4Dentry {
    ///
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>) -> Arc<dyn Dentry> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|me: &Weak<Ext4Dentry>| {
            Self {
                inner: DentryInner::new(name, parent_weak.clone()),
                self_weak: me.clone(),
            }
        })
    }
}

impl Dentry for Ext4Dentry {
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
    /// find the child dentry by the name, return None if not found
    /// the name was not the absolute path
    /// use the lwext4 dir operations to find the child dentry, and then create a new dentry for it
    /// so the path will with the '/0' at the end
    fn find(&self, name: &str) -> SysResult<Arc<dyn Dentry>> {
        info!("find the dentry by the name: {}", name);
        let clean_target = name.trim_matches(|c| c == '\0' || c == ' ');
        let current_dir_path = self.path(); 
        info!(">>> DEBUG: Ready to open dir [{}] to find [{}]", current_dir_path, clean_target);
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
                    current_dir_path,
                    err
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
                let file_path = format!("{}/{}", current_dir_path.trim_end_matches('/'), clean_target);
                // 某些镜像目录项可能返回 UNKNOWN，做一次路径探测以恢复真实类型。
                if file_type == InodeTypes::EXT4_DE_UNKNOWN {
                    if let Ok(c_probe) = CString::new(file_path.clone()) {
                        if ExtDir::open(&c_probe).is_ok() {
                            file_type = InodeTypes::EXT4_DE_DIR;
                        } else {
                            file_type = InodeTypes::EXT4_DE_REG_FILE;
                        }
                    }
                }

                info!("found {} in lwext4, type: {:?}", name, file_type);
                let child_inode = Arc::new(Ext4Inode::new(ino, file_type.clone()));
                if file_type == InodeTypes::EXT4_DE_REG_FILE {
                    let mut tmp_file = Lwext4File::new(&file_path, file_type);
                    if tmp_file.file_open(&file_path, O_RDONLY).is_ok() {
                        let real_size = tmp_file.file_desc.fsize as usize;
                        child_inode.set_size(real_size);
                    }
                }
                let my_arc = match self.self_weak.upgrade() {
                    Some(arc) => arc,
                    None => {
                        warn!("dentry dropped while finding child: {}", clean_target);
                        return Err(SysError::ENOENT);
                    }
                };
                let new_dentry = Ext4Dentry::new(name, Some(my_arc));
                new_dentry.set_inode(child_inode);
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
                error!("failed to create {}: invalid path contains NUL", target_path);
                return Err(SysError::EINVAL);
            }
        };
        match mode {
            InodeMode::DIR => ExtFS::create(&cpath)?,
            InodeMode::FILE => ExtFS::create_file(&cpath)?,
            _ => {
                warn!("unsupported inode mode: {:?}", mode);
                return Err(SysError::EINVAL);
            }
        };
        let new_dentry = match self.find(name) {
            Ok(dentry) => dentry,
            Err(_) => {
                error!("created {} on disk but failed to find it", target_path);
                return Err(SysError::EIO);
            }
        };
        GLOBAL_DCACHE.insert(target_path, new_dentry.clone());
        Ok(new_dentry)
    }

    /// list all the children of the current dentry
    /// return name and ino and type
    fn ls(&self) -> Vec<(String, u64, u8)> {
        info!("call ls on {}", self.path());
        let cpath = CString::new(self.path()).unwrap();
        ExtDir::open(&cpath).map(|mut dir| {
            let mut entries  = Vec::new();
            while let Some(entry) = dir.next() {
                if let Ok(name) = entry.name() {
                    let ino = entry.ino() as u64; 
                    let ext4_type = entry.file_type(); 
                    let dt_type = match ext4_type as i32 {
                        1 => DT_REG,
                        2 => DT_DIR, 
                        _ => DT_UNKNOWN,
                    };
                    entries.push((name, ino, dt_type));
                }
            }
            entries
        }).unwrap_or_default()
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
        GLOBAL_DCACHE.remove(&target_path);
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
        let new_dentry = Ext4Dentry::new(new_name, Some(self.self_weak.upgrade().unwrap()));
        GLOBAL_DCACHE.insert(new_path, new_dentry);
        Ok(0)
    }
    fn open(self: Arc<Self>, flags: OpenFlags, mode: InodeMode) -> SysResult<Arc<dyn File>> {
        let (readable, writable) = flags.read_write();
        let types = mode.to_inode_type();
        Ok(Arc::new(Ext4File::new(readable, writable, self, types, flags)))
    }
}

