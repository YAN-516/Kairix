//! implement the vfs operations and node operations for ext4 filesystem
//! definition in `vfs.rs`

use core::cell::RefCell;
use core::ptr::NonNull;

use alloc::string::String;
use alloc::ffi::CString;
use super::disk::Disk;
use alloc::sync::Arc;
use alloc::vec::Vec;
use crate::fs::vfs::inode::InodeInner;
use log::*;
use crate::logging;

use lwext4_rust::bindings::{
    O_APPEND, O_CREAT, O_RDONLY, O_RDWR, O_TRUNC, O_WRONLY, SEEK_CUR, SEEK_END, SEEK_SET,
};
use lwext4_rust::{Ext4BlockWrapper, Lwext4File, InodeTypes, KernelDevOp};

use virtio_drivers::device::blk::VirtIOBlk;
use virtio_drivers::transport::mmio::{MmioTransport, VirtIOHeader};
use virtio_drivers::transport::{DeviceType, Transport};

use crate::config::BLOCK_SIZE;
use crate::fs::vfs::inode::{Inode};
///The inode of the Ext4 filesystem
/// the InodeInner is ino
/// this_type is the InodeTypes
// pub struct Ext4Inode{
//     inner:InodeInner,
//     this_type: InodeTypes,
// }
// /// The inode of the Ext4 filesystem
pub struct Ext4Inode(RefCell<Lwext4File>);

unsafe impl Send for Ext4Inode {}
unsafe impl Sync for Ext4Inode {}

//ext4特有的inode实现
impl Ext4Inode{
     /// Create a new inode
    pub fn new(path: &str, types: InodeTypes) -> Self {
        info!("Inode new {:?} {}", types, path);
        //file.file_read_test("/test/test.txt", &mut buf);
        Self(RefCell::new(Lwext4File::new(path, types)))
    }
    fn path_deal_with(&self, path: &str) -> String {
        if path.starts_with('/') {
            warn!("path_deal_with: {}", path);
        }
        let p = path.trim_matches('/'); // 首尾去除
        if p.is_empty() || p == "." {
            return String::new();
        }

        if let Some(rest) = p.strip_prefix("./") {
            //if starts with "./"
            return self.path_deal_with(rest);
        }
        let rest_p = p.replace("//", "/");
        if p != rest_p {
            return self.path_deal_with(&rest_p);
        }

        //Todo ? ../
        //注：lwext4创建文件必须提供文件path的绝对路径
        let file = self.0.borrow_mut();
        let path = file.get_path();
        let fpath = String::from(path.to_str().unwrap().trim_end_matches('/')) + "/" + p;
        info!("dealt with full path: {}", fpath.as_str());
        fpath
    }
    
}


impl Inode for Ext4Inode {
    /// list all files' name in the directory
    #[allow(unused)]
    fn ls(&self) -> Vec<String> {
        info!("call ls");
        let file = self.0.borrow_mut();

        if file.get_type() != InodeTypes::EXT4_DE_DIR {
            info!("not a directory");
        }
        let (name, inode_type) = match file.lwext4_dir_entries() {
            Ok((name, inode_type)) => (name, inode_type),
            Err(e) => {
                panic!("error when ls: {}", e);
            }
        };

        info!("here!");
        let mut name_iter = name.iter();
        let  _inode_type_iter = inode_type.iter();

        let mut names = Vec::new();
        while let Some(iname) = name_iter.next() {
            names.push(String::from(core::str::from_utf8(iname).unwrap()));
        }
        info!("return from ls");
        names
    }
    /// Rename the inode
    #[allow(unused)]
    fn rename(&self, src_path: &str, dst_path: &str) -> Result<usize, i32> {
        info!("rename from {} to {}", src_path, dst_path);
        let mut file = self.0.borrow_mut();
        file.file_rename(src_path, dst_path)
    }
    /// Read data from inode at offset
    fn read_at(&self, offset:usize, buf: &mut [u8]) -> Result<usize, i32> {
        debug!("To read_at {}, buf len={}", offset, buf.len());
        let mut file = self.0.borrow_mut();
        let path = file.get_path();
        let path = path.to_str().unwrap();
        file.file_open(path, O_RDONLY)?;

        file.file_seek(offset as i64, SEEK_SET)?;
        let r = file.file_read(buf);

        let _ = file.file_close();
        r
    }

    /// Write data to inode at offset
    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize, i32> {
        debug!("To write_at {}, buf len={}", offset, buf.len());
        let mut file = self.0.borrow_mut();
        let path = file.get_path();
        let path = path.to_str().unwrap();
        file.file_open(path, O_RDWR)?;

        file.file_seek(offset as i64, SEEK_SET)?;
        let r = file.file_write(buf);

        let _ = file.file_close();
        r
    }
    // /// 获取inode的属性
    // fn get_attr(&self) -> Result<usize, i32> {
    //     unimplemented!()
    // }
    // /// Flush the file, synchronize the data to disk.
    // fn fsync(&self) -> Result<usize, i32> {
    //     unimplemented!()
    // }
    /// Truncate the inode to the given size
    fn truncate(&self, size: u64) -> Result<usize, i32> {
        info!("truncate file to size={}", size);
        let mut file = self.0.borrow_mut();
        let path = file.get_path();
        let path = path.to_str().unwrap();
        file.file_open(path, O_RDWR)?;

        let t = file.file_truncate(size);

        let _ = file.file_close();
        t
    }
    
    //lookup,是单级目录的查找,不递归,只查找下一跳
    /// Look up the node with given `name` in the directory
    /// Return the node if found.
    fn lookup(&self, name: &str) -> Option<Arc<dyn Inode>> {
        let mut file = self.0.borrow_mut();
        
        let full_path = String::from(file.get_path().to_str().unwrap().trim_end_matches('/')) + "/" + name;
        
        if file.check_inode_exist(full_path.as_str(), InodeTypes::EXT4_DE_REG_FILE) {
            info!("lookup file {} success", name);
            return Some(Arc::new(Ext4Inode::new(full_path.as_str(), InodeTypes::EXT4_DE_REG_FILE)));
        }

        if file.check_inode_exist(full_path.as_str(),InodeTypes::EXT4_DE_DIR){
            info!("lookup  dir {} success", name);
            return Some(Arc::new(Ext4Inode::new(full_path.as_str(), InodeTypes::EXT4_DE_DIR)));
        }

        info!("lookup {} failed", name);
        None
    }

    /// Create a new inode and return the inode
    fn create(&self, path: &str, ty: InodeTypes) -> Option<Arc<dyn Inode>> {
        info!("create {:?} on Ext4fs: {}", ty, path);
        let fpath = self.path_deal_with(path);
        let fpath = fpath.as_str();
        if fpath.is_empty() {
            info!("given path is empty");
            return None;
        }

        let types = ty;

        let mut file = self.0.borrow_mut();

        let result = if file.check_inode_exist(fpath, types.clone()) {
            info!("inode already exists");
            Ok(0)
        } else {
            if types == InodeTypes::EXT4_DE_DIR {
                file.dir_mk(fpath)
            } else {
                file.file_open(fpath, O_WRONLY | O_CREAT | O_TRUNC)
                    .expect("create file failed");
                file.file_close()
            }
        };

        match result {
            Err(e) => {
                error!("create inode failed: {}", e);
                None
            }
            Ok(_) => {
                info!("create inode success");
                Some(Arc::new(Ext4Inode::new(fpath, types)))
            }
        }
    }

    /// Remove the inode
    #[allow(unused)]
    fn remove(&self, path: &str) -> Result<usize, i32> {
        info!("remove ext4fs: {}", path);
        let fpath = self.path_deal_with(path);
        let fpath = fpath.as_str();

        assert!(!fpath.is_empty()); // already check at `root.rs`

        let mut file = self.0.borrow_mut();
        if file.check_inode_exist(fpath, InodeTypes::EXT4_DE_DIR) {
            // Recursive directory remove
            file.dir_rm(fpath)
        } else {
            file.file_remove(fpath)
        }
    }
    // /// Renames or moves existing file or directory.
    // fn rename(&self, _src_path: &str, _dst_path: &str) -> Result<usize, i32> {
    //     unimplemented!()
    // }
    // // //链接部分
    // // fn link(&self, name: &str, target: Arc<dyn VfsInode>) -> Result<(), i32>{
    // //     unimplemented!()
    // // }
    // // fn symlink(&self, name: &str, target: &str) -> Result<(), i32>{
    // //     unimplemented!()
    // // }
    // // fn readlink(&self) -> Result<String, i32>{
    // //     unimplemented!()
    // // }


}

impl Drop for Ext4Inode {
    fn drop(&mut self) {
        let mut file = self.0.borrow_mut();
        debug!("Drop struct Inode {:?}", file.get_path());
        file.file_close().expect("failed to close fd");
        drop(file); // todo
    }
}
