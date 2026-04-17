use core::cell::RefCell;
use core::ptr::NonNull;
use core::sync::atomic::Ordering;

use alloc::ffi::CString;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use crate::fs::vfs::inode::InodeMode;

use log::*;
use spin::mutex::Mutex;

use lwext4_rust::{
    bindings::{
        O_APPEND, O_CREAT, O_RDONLY, O_RDWR, O_TRUNC, O_WRONLY, 
        SEEK_CUR, SEEK_END, SEEK_SET,
    },
    Ext4BlockWrapper, InodeTypes, KernelDevOp, Lwext4File,
};

use virtio_drivers::{
    device::blk::VirtIOBlk,
    transport::{
        mmio::{MmioTransport, VirtIOHeader},
        DeviceType, Transport,
    },
};

use crate::config::BLOCK_SIZE;
use crate::fs::vfs::inode::{Inode, InodeInner};
use crate::logging;

use super::disk::Disk;
#[allow(unused)]
///The inode of the Ext4 filesystem
/// the InodeInner is ino
/// this_type is the InodeTypes
pub struct Ext4Inode{
    inner:Mutex<InodeInner>,
    this_type: InodeTypes,
}

unsafe impl Send for Ext4Inode {}
unsafe impl Sync for Ext4Inode {}

impl Ext4Inode{
    ///
    pub fn new(ino:usize, types: InodeTypes) -> Self {
        info!("Inode new {:?} with ino {}", types, ino);
        let mode = InodeMode::from_inode_type(types.clone());
        
        Self{
            inner: Mutex::new(InodeInner::new(ino,0,mode)),
            this_type: types
        }
    }
}


impl Inode for Ext4Inode {
    
    /// Get the attributes of the file, such as size, permissions, etc.
    fn get_attr(&self) -> Result<usize, i32> {
        unimplemented!()
    }
    /// Flush the file, synchronize the data to disk.
    fn fsync(&self) -> Result<usize, i32> {
        unimplemented!()
    }
    fn truncate(&self, size: u64) -> Result<usize, i32> {
        self.set_size(size as usize);
        Ok(0)
    }
    ///
    fn get_types(&self) -> InodeTypes {
        match self.this_type {
            InodeTypes::EXT4_DE_REG_FILE => InodeTypes::EXT4_DE_REG_FILE,
            InodeTypes::EXT4_DE_DIR => InodeTypes::EXT4_DE_DIR,
            _ => panic!("Unsupported InodeType: {:?}", self.this_type),
        }
    }
    fn get_ino(&self) -> usize {
        self.inner.lock().ino
    }
    
    fn get_size(&self) -> usize {
        self.inner.lock().size.load(Ordering::Relaxed)
    }

    fn set_size(&self, new_size: usize) {
        self.inner.lock().size.store(new_size, Ordering::Relaxed);
    }

    fn get_nlink(&self) -> usize {
        self.inner.lock().nlink.load(Ordering::Relaxed)
    }

    fn get_mode(&self) -> InodeMode {
        self.inner.lock().mode
    }
    fn inc_nlink(&self) {
        self.inner.lock().nlink.fetch_add(1, Ordering::SeqCst);
    }
    
    fn dec_nlink(&self) {
        self.inner.lock().nlink.fetch_sub(1, Ordering::SeqCst);
    }
}


/// translate between InodeTypes and InodeMode
impl InodeMode {
    /// Convert an InodeTypes to an InodeMode, setting the type bits and permission bits.
    pub fn from_inode_type(itype: InodeTypes) -> Self {
        let perm_mode = InodeMode::OWNER_MASK | InodeMode::GROUP_MASK | InodeMode::OTHER_MASK;
        let file_mode = match itype {
            InodeTypes::EXT4_DE_DIR => InodeMode::DIR,
            InodeTypes::EXT4_DE_REG_FILE => InodeMode::FILE,
            InodeTypes::EXT4_DE_CHRDEV => InodeMode::CHAR,
            InodeTypes::EXT4_DE_FIFO => InodeMode::FIFO,
            InodeTypes::EXT4_DE_BLKDEV => InodeMode::BLOCK,
            InodeTypes::EXT4_DE_SOCK => InodeMode::SOCKET,
            InodeTypes::EXT4_DE_SYMLINK => InodeMode::LINK,
            _ => InodeMode::TYPE_MASK,
        };
        file_mode | perm_mode
    }
    /// Convert an InodeMode to an InodeTypes, extracting the type bits and ignoring the permission bits.
    pub fn to_inode_type(self) -> InodeTypes {
        match self.get_type() {
            InodeMode::DIR    => InodeTypes::EXT4_DE_DIR,
            InodeMode::FILE   => InodeTypes::EXT4_DE_REG_FILE,
            InodeMode::CHAR   => InodeTypes::EXT4_DE_CHRDEV,
            InodeMode::FIFO   => InodeTypes::EXT4_DE_FIFO,
            InodeMode::BLOCK  => InodeTypes::EXT4_DE_BLKDEV,
            InodeMode::SOCKET => InodeTypes::EXT4_DE_SOCK,
            InodeMode::LINK   => InodeTypes::EXT4_DE_SYMLINK,
            _ => InodeTypes::EXT4_DE_UNKNOWN,
        }
    }
    /// Get the type bits of the InodeMode, masking out the permission bits.
    pub fn get_type(self) -> Self {
        self.intersection(InodeMode::TYPE_MASK)
    }
}