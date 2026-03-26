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
use core::sync::atomic::Ordering;
use lwext4_rust::bindings::{
    O_APPEND, O_CREAT, O_RDONLY, O_RDWR, O_TRUNC, O_WRONLY, SEEK_CUR, SEEK_END, SEEK_SET,
};
use lwext4_rust::{Ext4BlockWrapper, Lwext4File, InodeTypes, KernelDevOp};
use crate::fs::vfs::inode::InodeType;
use virtio_drivers::device::blk::VirtIOBlk;
use virtio_drivers::transport::mmio::{MmioTransport, VirtIOHeader};
use virtio_drivers::transport::{DeviceType, Transport};

use crate::config::BLOCK_SIZE;
use crate::fs::vfs::inode::{Inode};
#[allow(unused)]
///The inode of the Ext4 filesystem
/// the InodeInner is ino
/// this_type is the InodeTypes
pub struct Ext4Inode{
    inner:InodeInner,
    this_type: InodeTypes,
}

unsafe impl Send for Ext4Inode {}
unsafe impl Sync for Ext4Inode {}

impl Ext4Inode{
    ///
    pub fn new(ino:usize, types: InodeTypes) -> Self {
        info!("Inode new {:?} with ino {}", types, ino);
        let mode = if types == InodeTypes::EXT4_DE_DIR { 0o040777 } else { 0o100777 };
        Self{
            inner: InodeInner::new(ino,0,mode),
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
    ///
    fn get_types(&self) -> InodeTypes {
        match self.this_type {
            InodeTypes::EXT4_DE_REG_FILE => InodeTypes::EXT4_DE_REG_FILE,
            InodeTypes::EXT4_DE_DIR => InodeTypes::EXT4_DE_DIR,
            _ => panic!("Unsupported InodeType: {:?}", self.this_type),
        }
    }
    fn get_ino(&self) -> usize {
        self.inner.ino
    }
    
    fn get_size(&self) -> usize {
        self.inner.size.load(Ordering::Relaxed)
    }

    fn get_nlink(&self) -> usize {
        self.inner.nlink.load(Ordering::Relaxed)
    }

    fn get_mode(&self) -> u32 {
        self.inner.mode
    }
    fn inc_nlink(&self) {
        self.inner.nlink.fetch_add(1, Ordering::SeqCst);
    }
    
    fn dec_nlink(&self) {
        self.inner.nlink.fetch_sub(1, Ordering::SeqCst);
    }
}
