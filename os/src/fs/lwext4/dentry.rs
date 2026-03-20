use crate::fs::vfs::DentryInner;
use alloc::vec::Vec;
use crate::fs::vfs::Dentry;
use alloc::string::String;
use log::info;
use alloc::sync::Arc;
pub struct Ext4Dentry{
    inner:DentryInner
}
    
impl Ext4Dentry{
    pub fn new(
        name:&str,
        parent:Option<Arc<dyn Dentry>>
    )->Arc<dyn Dentry>{
        let parent = parent.map(|p| Arc::downgrade(&p));
        Arc::new(Self{inner:DentryInner::new(name,parent)})
    }
    //待修改，加入dcache 等
    // /// list all files' name in the directory
    // #[allow(unused)]
    // fn ls(&self) -> Vec<String> {
    //     info!("call ls");
    //     let file = self.0.borrow_mut();

    //     if file.get_type() != InodeTypes::EXT4_DE_DIR {
    //         info!("not a directory");
    //     }
    //     let (name, inode_type) = match file.lwext4_dir_entries() {
    //         Ok((name, inode_type)) => (name, inode_type),
    //         Err(e) => {
    //             panic!("error when ls: {}", e);
    //         }
    //     };

    //     info!("here!");
    //     let mut name_iter = name.iter();
    //     let  _inode_type_iter = inode_type.iter();

    //     let mut names = Vec::new();
    //     while let Some(iname) = name_iter.next() {
    //         names.push(String::from(core::str::from_utf8(iname).unwrap()));
    //     }
    //     info!("return from ls");
    //     names
    // }
}
    
impl Dentry for Ext4Dentry{
    fn get_dentryinner(&self)->&DentryInner {
        &self.inner
    }
    // ///name
    // fn name(&self) -> &str {
    //     unimplemented!()
    // }

    /// Get the parent directory of this directory.
    /// Return `None` if the node is a file.
    #[allow(unused)]
    fn parent(&self) -> Option<Arc<dyn Dentry>> {
        self.inner.parent.as_ref().and_then(|p| p.upgrade())
    }
    // fn children(&self) -> BTreeMap<String, Arc<dyn Dentry>> {
    //     unimplemented!()
    // }
    // fn add_child(&self, _child: Arc<dyn Dentry>) {
    //     unimplemented!()
    // }
    //  fn remove_child(&self, _name: &str) {
    //     unimplemented!()
    // }
    ///inode
    // fn get_inode(&self)->Option<Arc<dyn Inode>>{
    //     self.get_dentryinner().inode.lock().clone()
    // }

    /// Find inode under current dentry by name
    /// In the futuer,will finish the dcache
    /// current dentry ,不递归,name 是子目录或者子文件的名字
    fn find(&self, name: &str) -> Option<Arc<dyn Dentry>> {
        //wait for the dcache
        // if let Some(child) = self.children().get(name) {
        //     return Some(child.clone());
        // }
        info!("find the dentry by the name {}",name);
        let inode = self.get_inode()?; 
        let child_inode = inode.lookup(name)?; 
        let new_dentry =Ext4Dentry::new(
            name,                
            None,
        );
        new_dentry.set_inode(child_inode);
        Some(new_dentry)
    }
    // fn set_inode(&self, _inode: Arc<dyn Inode>) {
    //     unimplemented!()
    // }
    // fn clear_inode(&self) {
    //     unimplemented!()
    // }
}