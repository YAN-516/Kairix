use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::vfs::{Dentry,DentryInner};
use alloc::sync::Weak;
use alloc::sync::Arc;
use alloc::string::String;
use log::info;
use alloc::ffi::CString;
use crate::fs::fat32::fat::dir::Fat32Dir;
use alloc::vec::Vec;
use crate::fs::fat32::io::FatIoAdapter;
pub struct Fat32Dentry {
    inner: DentryInner,
    /// The self_weak field is designed to allow a Dentry to correctly set the parent reference 
    /// when creating child Dentry instances
    self_weak: Weak<Fat32Dentry>,
}

impl Fat32Dentry{
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>) -> Arc<dyn Dentry> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|me: &Weak<Fat32Dentry>| {
            Self {
                inner: DentryInner::new(name, parent_weak.clone()),
                self_weak: me.clone(),
            }
        })
    }
}

impl Dentry for Fat32Dentry{
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

    fn find(&self, _name: &str) -> SysResult<Arc<dyn Dentry>> {
        unimplemented!()
    }

    fn create(&self, _name: &str, _ty: InodeType) -> SysResult<Arc<dyn Dentry>> {
        unimplemented!()
    }
    fn ls(&self) -> Vec<(String, u64, u8)> {
        unimplemented!()
    }
    fn unlink(&self, _name: &str, _flags: u32) -> SyscallResult {
        unimplemented!()
    }
    fn link(&self, _new_name: &str, _old_dentry: Arc<dyn Dentry>)-> SyscallResult {
        unimplemented!()
    }

}