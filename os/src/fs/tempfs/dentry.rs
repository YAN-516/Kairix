use alloc::ffi::CString;
use alloc::format;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::AtomicU64;
use core::sync::atomic::Ordering;
use crate::alloc::string::ToString;
use crate::fs::vfs::Inode;
use log::*;
use crate::fs::tempfs::inode::TempInode;
use crate::fs::tempfs::file::TempFile;
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
    
    /// find the child dentry by the name, return None if not found
    fn find(&self, name: &str) -> Option<Arc<dyn Dentry>> {
        let children = self.inner.children.lock();
        return children.get(name).cloned();  
    }

    /// create a new dentry with the name and type, and return it, if the dentry already exists, return None
    // fn create(&self, name: &str, mode: InodeMode) -> Option<Arc<dyn Dentry>> {
    //     info!("create {:?} on Ext4Dentry: {}", mode, name);  
    //     let parent_path = self.path(); 
    //     let target_path = format!("{}/{}", parent_path.trim_end_matches('/'), name);  
    //     let cpath = CString::new(target_path.clone()).ok().unwrap();
    //     let is_success = match mode {
    //         InodeMode::DIR => ExtFS::create(&cpath).is_ok(),
    //         InodeMode::FILE => ExtFS::create_file(&cpath).is_ok(),
    //         _ => {
    //             warn!("unsupported inode mode: {:?}", mode);
    //             return None;
    //         }
    //     };
    //     if !is_success {
    //         error!("failed to create {} on disk", target_path);
    //         return None;
    //     }
    //     let new_dentry = self.find(name).unwrap();
    //     GLOBAL_DCACHE.insert(target_path, new_dentry.clone());
    //     Some(new_dentry)
    // }
    // fn create(&self, name: &str, mode: InodeMode) -> Option<Arc<dyn Dentry>> {
    //     let mut children = self.inner.children.lock();
    //     if children.contains_key(name) {
    //         return None;
    //     }   
    //     let my_arc = self.self_weak.upgrade().unwrap();
    //     let new_dentry = TempDentry::new(name, Some(my_arc as Arc<dyn Dentry>));
        
    //     let ino = next_tmpfs_ino();
        
    //     let child_inode = Arc::new(TempInode::new(ino as usize, mode)); 
    //     new_dentry.set_inode(child_inode);

    //     children.insert(name.to_string(), new_dentry.clone());
        
    //     let target_path = format!("{}/{}", self.path().trim_end_matches('/'), name);
    //     GLOBAL_DCACHE.insert(target_path, new_dentry.clone());
        
    //     Some(new_dentry)
    // }

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

    fn unlink(&self, _name: &str, _flags: u32) -> isize {
       unimplemented!()
    }
    
    fn link(&self, _new_name: &str, _old_dentry: Arc<dyn Dentry>) -> isize {
        unimplemented!()
    }

    fn open(self: Arc<Self>, _flags: OpenFlags,_mode: InodeMode) -> Option<Arc<dyn File>> {
        // let (readable, writable) = flags.read_write();
        // let types = mode.to_inode_type();
        Some(Arc::new(TempFile::new(self)))
    }
}

