extern crate lwext4_rust;
extern crate virtio_drivers;

use alloc::sync::Weak;
use lwext4_rust::{InodeTypes, Lwext4File};

use virtio_drivers::device::blk::VirtIOBlk;
use virtio_drivers::transport::mmio::{MmioTransport, VirtIOHeader};
use virtio_drivers::transport::{DeviceType, Transport};


use crate::drivers::block::BLOCK_DEVICE;
use crate::fs::FS_MANAGER;
use crate::fs::vfs::inode::Inode;

use alloc::vec;
use alloc::{format, vec::Vec};
use alloc::boxed::Box;

use crate::fs::lwext4::inode::{Ext4Inode};
use crate::fs::lwext4::disk::Disk;

use crate::fs::vfs::file::File;
use crate::mm::UserBuffer;
use crate::sync::UPSafeCell;
use alloc::sync::Arc;
use bitflags::*;
use lazy_static::*;
use spin::Mutex;
use crate::fs::vfs::FileInner;
use crate::fs::vfs::Dentry;
use core::cell::RefMut;
use spin::MutexGuard;
use alloc::string::String;
use log::{warn,info};
use crate::fs::Ext4Dentry;
///the Ext4File
pub struct Ext4File {
    readable: bool,
    writable: bool,
    inner:Mutex<FileInner>,
}

impl Ext4File {
    /// Construct an Ext4File from a Dentry
    /// path 是绝对路径，暂时先传入path和types，等到后续能够返回查看父目录的时候进行修改
    pub fn new(readable: bool, writable: bool, dentry: Arc<dyn Dentry>,path: &str, types: InodeTypes) -> Self {
        let file  = Lwext4File::new(path, types);
        Self {
            readable,
            writable,
            inner:Mutex::new(FileInner { offset: 0, 
                                        dentry,
                                        ext4file:file
                                    }),
        }
    }
    /// Read all data inside a inode into vector
    pub fn read_all(&self) -> Vec<u8> {
        let mut inner = self.inner.lock();
        let mut buffer = [0u8; 512];
        let mut v: Vec<u8> = Vec::new();
        loop {
            let len = inner.dentry.get_inode().unwrap().read_at(inner.offset, &mut buffer).unwrap();
            if len == 0 {
                break;
            }
            inner.offset += len;
            v.extend_from_slice(&buffer[..len]);
        }
        v
    }
}


impl File for Ext4File {
    fn get_fileinner(&self)->MutexGuard<'_, FileInner> {
        self.inner.lock()
    }
    fn readable(&self) -> bool {
        self.readable
    }
    fn writable(&self) -> bool {
        self.writable
    }
    fn read(&self, mut buf: UserBuffer) -> usize {
        let inode = self.get_inode().unwrap(); 
        let mut inner = self.get_fileinner();
        let mut total_read_size = 0usize;
        for slice in buf.buffers.iter_mut() {
            let read_size = inode.read_at(inner.offset, *slice).unwrap();
            if read_size == 0 {
                break;
            }
            inner.offset += read_size;
            total_read_size += read_size;
        }
        total_read_size
    }
    fn write(&self, buf: UserBuffer) -> usize {
        info!("enter Ext4File write");
        let inode = self.get_inode().unwrap(); 
        let mut inner = self.inner.lock();
        let mut total_write_size = 0usize;
        for slice in buf.buffers.iter() {
            let write_size = inode.write_at(inner.offset, *slice).unwrap();
            assert_eq!(write_size, slice.len());
            inner.offset += write_size;
            total_write_size += write_size;
        }
        info!("finish Ext4File write");
        total_write_size
    }
    // ///get inode from the Dentry of FileInner
    // fn get_inode(&self)-> Option<Arc<dyn Inode>>{
    //     self.get_fileinner().dentry.get_inode()
    // }
    // /// Do something when the node is opened.
    // fn open(&self) -> Result<usize, i32> {
    //     Ok(0)
    // }
    // /// Do something when the node is closed.
    // fn release(&self) -> Result<usize, i32> {
    //     Ok(0)
    // }
    // #[allow(unused)]
    // ///chaneg the offset of file
    // /// 
    // fn seek(&self,new_offset:usize)->usize{
    //     unimplemented!()
    // }
}

bitflags! {
    ///Open file flags
    pub struct OpenFlags: u32 {
        ///Read only
        const RDONLY = 0;
        ///Write only
        const WRONLY = 1 << 0;
        ///Read & Write
        const RDWR = 1 << 1;
        ///Allow create
        const CREATE = 1 << 9;
        ///Clear file and return an empty one
        const TRUNC = 1 << 10;
    }
}

impl OpenFlags {
    /// Do not check validity for simplicity
    /// Return (readable, writable)
    pub fn read_write(&self) -> (bool, bool) {
        if self.is_empty() {
            (true, false)
        } else if self.contains(Self::WRONLY) {
            (false, true)
        } else {
            (true, true)
        }
    }
}

// /// 根据路径递归寻找 Inode
// /// 待优化.和..的处理,相对路径的处理,当前路径的处理
// fn find_inode(path: &str) -> Option<Arc<dyn Inode>> {
//     let mut current_inode = FS_MANAGER.exclusive_access().get("lwext4").unwrap().root();
//     //现在的逻辑都是从根目录开始找
//     let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
//     if parts.is_empty() {
//         return Some(current_inode);
//     }
    
//     //  逐级 lookup
//     for part in parts {
//         if let Some(next_inode) = current_inode.lookup(part) {
//             current_inode = next_inode;
//         } else {
//             return None; // 中间某一级查找失败
//         }
//     }
//     Some(current_inode)
// }

/// 根据路径递归寻找 Inode
/// 待优化.和..的处理,相对路径的处理,当前路径的处理
fn find_dentry(path: &str) -> Option<Arc<dyn Dentry>> {
    let root_dentry = FS_MANAGER.exclusive_access().get("lwext4").unwrap().root();
    //现在的逻辑都是从根目录开始找
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return Some(root_dentry);
    }
    let mut current_dentry = root_dentry;
    //  逐级 find
    for part in parts {
        if let Some(next_dentry) = current_dentry.find(part) {
            current_dentry = next_dentry;
        } else {
            return None; // 中间某一级查找失败
        }
    }
    Some(current_dentry)
}
#[allow(unused)]
//open_file已经修改,从开始的从根目录开始扁平查找改成使用find_inode直接找到最终的inode,支持多级目录
//需要添加，需支持.和..的处理,相对路径的处理,当前路径的处理
///Open file with flags
/// name 为绝对路径
pub fn open_file(name: &str, flags: OpenFlags) -> Option<Arc<Ext4File>> {
    let (readable, writable) = flags.read_write();
    let root_dentry = FS_MANAGER.exclusive_access().get("lwext4").unwrap().root();
    let dentry_result = find_dentry(name);
    if flags.contains(OpenFlags::CREATE) {
        //allow create, but existed
        if let Some(dentry) = dentry_result {
            if flags.contains(OpenFlags::TRUNC) {
                // clear by inode
                if let Some(inode) = dentry.get_inode() {
                    inode.truncate(0).expect("Error when truncating inode");
                }
            }
            let target_dentry = dentry;
            Some(Arc::new(Ext4File::new(readable, writable, target_dentry,name,InodeTypes::EXT4_DE_REG_FILE)))
        } else {
            //allow create and not exist  
            //Path splitting: Identify parent directory and new filename
            //  "/a/b/c.txt" -> parent_path = "/a/b", file_name = "c.txt"
            //采用ai的方法，切割字符串
            let (parent_path, file_name) = match name.rfind('/') {
                Some(idx) => {
                    let parent = if idx == 0 { "/" } else { &name[..idx] };
                    let file = &name[idx + 1..];
                    (parent, file)
                }
                None => ("/", name), // 如果没有斜杠，默认父目录是根目录 "/"
            };
            // find the parent_dentry
            let parent_dentry = if parent_path == "/" {
                FS_MANAGER.exclusive_access().get("lwext4").unwrap().root()
            } else {
                //if there is not parent_dentry,return None
                find_dentry(parent_path).unwrap()
            };
            // get the parent_inode,create the inode
            let parent_inode = parent_dentry.get_inode().unwrap();
            let new_inode = parent_inode
                .create(file_name, InodeTypes::EXT4_DE_REG_FILE)
                .expect("Failed to create file on disk");
            // create the new dentry
            let new_dentry = Ext4Dentry::new(
                file_name,
                None 
            );
            new_dentry.set_inode(new_inode);
            // parent_dentry.add_child(new_dentry.clone());
            let target_dentry = new_dentry;
            Some(Arc::new(Ext4File::new(readable, writable, target_dentry,name,InodeTypes::EXT4_DE_REG_FILE)))
        } 
    } else {
        let dentry = dentry_result.unwrap(); 
        if flags.contains(OpenFlags::TRUNC) {
            if let Some(inode) = dentry.get_inode() {
                inode.truncate(0).expect("Error when truncating inode");
            }
        }
        let target_dentry = dentry;
        Some(Arc::new(Ext4File::new(readable, writable, target_dentry,name,InodeTypes::EXT4_DE_REG_FILE)))
        
    }
}

/// List all files in the filesystems
pub fn list_apps() {
    let root_dentry = FS_MANAGER.exclusive_access().get("lwext4").unwrap().root();
    let root_inode = root_dentry.get_inode().unwrap();
    println!("/**** APPS ****");
    for app in root_inode.ls() {
        println!("{}", app);
    }
    println!("**************/");
}
