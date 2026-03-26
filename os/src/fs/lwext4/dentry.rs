use alloc::ffi::CString;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use log::*;
use crate::fs::vfs::{Dentry, DentryInner};
use crate::fs::vfs::inode::InodeType;
use crate::fs::lwext4::ext4::dir::ExtDir; 
use alloc::sync::Weak;
use crate::fs::vfs::dcache::GLOBAL_DCACHE;
use crate::fs::Ext4Inode;
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
    fn find(&self, name: &str) -> Option<Arc<dyn Dentry>> {
        info!("find the dentry by the name: {}", name);
        let clean_target = name.trim_matches(|c| c == '\0' || c == ' ');
        let current_dir_path = self.path(); 
         info!(">>> DEBUG: Ready to open dir [{}] to find [{}]", current_dir_path, clean_target);
        let path = CString::new(self.path()).unwrap();
        let mut dir = ExtDir::open(&path).unwrap();
        while let Some(entry) = dir.next() {
            if entry.name().unwrap() == name {
                let (ino, file_type) = Some((entry.ino() as usize, entry.file_type())).unwrap();
                info!("found {} in lwext4, type: {:?}", name, file_type);
                let child_inode = Arc::new(Ext4Inode::new(ino, file_type)); 
                let my_arc = self.self_weak.upgrade().expect("Dentry dropped while in use!");
                let new_dentry = Ext4Dentry::new(name, Some(my_arc));
                new_dentry.set_inode(child_inode);
                return Some(new_dentry);
            }
        }
        return None;  
    }

    /// create a new dentry with the name and type, and return it, if the dentry already exists, return None
    fn create(&self, name: &str, ty: InodeType) -> Option<Arc<dyn Dentry>> {
        info!("create {:?} on Ext4Dentry: {}", ty, name);  
        let parent_path = self.path(); 
        let target_path = format!("{}/{}", parent_path.trim_end_matches('/'), name);  
        let cpath = CString::new(target_path.clone()).ok().unwrap();
        let is_success = match ty {
            InodeType::Dir => ExtDir::create(&cpath).is_ok(),
            InodeType::File => ExtDir::create_file(&cpath).is_ok(),
        };
        if !is_success {
            error!("failed to create {} on disk", target_path);
            return None;
        }
        let new_dentry = self.find(name).unwrap();
        GLOBAL_DCACHE.insert(target_path, new_dentry.clone());
        Some(new_dentry)
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
}

