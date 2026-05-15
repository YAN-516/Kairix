#![allow(missing_docs)]
use crate::error::{SysError, SysResult, SyscallResult};
use spin::Mutex;
use alloc::string::{String, ToString};
use alloc::sync::{Arc,Weak};
use alloc::collections::BTreeMap;
use alloc::format;
use crate::fs::vfs::Inode;
use alloc::vec::Vec;
use log::info;
use crate::fs::vfs::inode::InodeMode;
use crate::fs::vfs::OpenFlags;
use crate::fs::File;

#[allow(unused)]
///the detail of data in dentry
pub struct DentryInner {
    /// Name.
    pub name: String,
    /// Parent dentry. This field is `None` if this dentry is the root of the filesystem.
    pub parent: Option<Weak<dyn Dentry>>,
    /// Children dentries.
    pub children: Mutex<BTreeMap<String, Arc<dyn Dentry>>>,
    /// Inode that this dentry points to.
    pub inode: Mutex<Option<Arc<dyn Inode>>>,
    /// Dentry before mount. `None` if this dentry has not been mounted.
    pub mdentry: Mutex<Option<Arc<dyn Dentry>>>,
    /// Dentry bind mount. `None` if this dentry has not been bind-mounted.
    pub bdentry: Mutex<Option<Arc<dyn Dentry>>>,
}

#[allow(unused)]
impl DentryInner{
    pub fn new(
        name:&str,
        parent:Option<Weak<dyn Dentry>>,
    )->Self{
        Self { 
            name: name.to_string(),
            parent,
            children: Mutex::new(BTreeMap::new()),
            inode:Mutex::new(None),
            mdentry: Mutex::new(None),
            bdentry: Mutex::new(None),
        }
    }
}
#[allow(unused)]
#[derive(Default, Debug, PartialEq, Eq, Clone, Copy)]
///
pub enum DentryState {
    #[default]
    UnInit,
    Sync,
    Dirty,
}

pub trait Dentry: Send + Sync{
    fn get_dentryinner(&self)->&DentryInner;
    ///name
    fn name(&self) -> &str{
        self.get_dentryinner().name.as_str()
    }
    fn rename(&self,_src_path: &str, _dst_path: &str)-> SysResult<usize> {
        todo!()
    }
    // directory operations:
    /// Get the parent directory of this directory.
    ///
    /// Return `None` if the node is a file.
    fn parent(&self) -> Option<Arc<dyn Dentry>>{
        self.get_dentryinner().parent.as_ref().and_then(|p| p.upgrade())
    }
    fn children(&self) -> BTreeMap<String, Arc<dyn Dentry>> {
        self.get_dentryinner().children.lock().clone()
    }
    fn add_child(&self, child: Arc<dyn Dentry>) {
        self.get_dentryinner().children.lock().insert(child.name().to_string(), child);
    }
     fn remove_child(&self, _name: &str) {
          self.get_dentryinner().children.lock().remove(_name);
    }
    ///inode
    ///find the inode by the dcache,if can not find,use the lookup function of inode
    fn find(&self, _name: &str) -> SysResult<Arc<dyn Dentry>>{
        if let Some(child) = self.get_dentryinner().children.lock().get(_name).cloned() {
            return Ok(child);
        }
        if let Some(bdentry) = self.get_dentryinner().bdentry.lock().clone() {
            if let Ok(child) = bdentry.find(_name) {
                return Ok(child);
            }
        }
        Err(SysError::ENOENT)
    }
    fn get_inode(&self)->Option<Arc<dyn Inode>>{
        self.get_dentryinner().inode.lock().clone()
    }
    
    fn set_inode(&self, inode: Arc<dyn Inode>) {
        *self.get_dentryinner().inode.lock()=Some(inode);
    }
    fn clear_inode(&self) {
        *self.get_dentryinner().inode.lock() = None;
    }
    fn path(&self) -> String{
        if let Some(parent) = self.parent() {
            let parent_path = parent.path();
            if parent_path == "/" {
                if self.name().is_empty() {
                    "/".to_string()
                } else {
                    format!("/{}", self.name())
                }
            } else if self.name().is_empty() {
                parent_path
            } else {
                format!("{}/{}", parent_path, self.name())
            }
        } else if self.name().is_empty() {
            "/".to_string()
        } else {
            self.name().to_string()
        }
    }
    fn create(&self, _name: &str, _mode: InodeMode) -> SysResult<Arc<dyn Dentry>>{
        todo!()
    }
    fn ls(&self) -> Vec<(String, u64, u8)> {
        alloc::vec::Vec::new() 
    }
    fn unlink(&self, _name: &str, _flags: u32) -> SyscallResult{
        Err(SysError::EIO)
    }
    fn link(&self, _new_name: &str, _old_dentry: Arc<dyn Dentry>)->SyscallResult{
        Err(SysError::EIO)
    }
    /// Create a symbolic link.
    fn symlink(&self, _name: &str, _target: &str) -> SyscallResult {
        Err(SysError::EIO)
    }
    /// Create a special file (device, fifo, socket).
    fn mknod(&self, _name: &str, _mode: InodeMode, _dev: u32) -> SyscallResult {
        Err(SysError::ENOSYS)
    }
    /// open the inode it points as File
    fn open(self: Arc<Self>, _flags: OpenFlags,_modes: InodeMode) -> SysResult<Arc<dyn File>> {
        todo!()
    }
}

impl dyn Dentry{
    /// Store the original dentry before mount, for umount restoration.
    pub fn store_mount_dentry(&self, dentry: Arc<dyn Dentry>) {
        *self.get_dentryinner().mdentry.lock() = Some(dentry);
    }
    /// Fetch and clear the stored original dentry.
    pub fn fetch_mount_dentry(&self) -> Option<Arc<dyn Dentry>> {
        self.get_dentryinner().mdentry.lock().take()
    }
    /// Peek the stored original dentry without clearing.
    pub fn get_mount_dentry(&self) -> Option<Arc<dyn Dentry>> {
        self.get_dentryinner().mdentry.lock().clone()
    }
    /// Store the original dentry for bind mount fallback.
    pub fn bind_mount_dentry(&self, dentry: Arc<dyn Dentry>) {
        *self.get_dentryinner().bdentry.lock() = Some(dentry);
    }
    /// Clear the bind mount fallback dentry.
    pub fn unbind_mount_dentry(&self) {
        *self.get_dentryinner().bdentry.lock() = None;
    }
    /// Peek the bind mount fallback dentry.
    pub fn get_bind_dentry(&self) -> Option<Arc<dyn Dentry>> {
        self.get_dentryinner().bdentry.lock().clone()
    }
}
