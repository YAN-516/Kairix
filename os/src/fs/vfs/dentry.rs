#![allow(missing_docs)]
use spin::Mutex;
use alloc::string::{String, ToString};
use alloc::sync::{Arc,Weak};
use alloc::collections::BTreeMap;
use crate::fs::vfs::Inode;
use alloc::vec::Vec;
use log::info;
use crate::fs::vfs::inode::InodeType;

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
            inode:Mutex::new(None)
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
    fn name(&self) -> &str;
    fn rename(&self,_src_path: &str, _dst_path: &str)-> Result<usize, i32> {
        unimplemented!()
    }
    // directory operations:
    /// Get the parent directory of this directory.
    ///
    /// Return `None` if the node is a file.
    fn parent(&self) -> Option<Arc<dyn Dentry>>;
    fn children(&self) -> BTreeMap<String, Arc<dyn Dentry>> {
        unimplemented!()
    }
    fn add_child(&self, _child: Arc<dyn Dentry>) {
        unimplemented!()
    }
     fn remove_child(&self, _name: &str) {
        unimplemented!()
    }
    ///inode
    ///find the inode by the dcache,if can not find,use the lookup function of inode
    fn find(&self, _name: &str) -> Option<Arc<dyn Dentry>>;
    fn get_inode(&self)->Option<Arc<dyn Inode>>{
        self.get_dentryinner().inode.lock().clone()
    }
    fn set_inode(&self, inode: Arc<dyn Inode>) {
        *self.get_dentryinner().inode.lock()=Some(inode);
    }
    fn clear_inode(&self) {
        unimplemented!()
    }
    fn path(&self) -> String;
    fn create(&self, name: &str, ty: InodeType) -> Option<Arc<dyn Dentry>>;
    fn ls(&self) -> Vec<String>;
}

impl dyn Dentry{

}