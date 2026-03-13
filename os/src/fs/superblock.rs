
use core::cell::RefCell;
use core::ptr::NonNull;

use alloc::string::String;
use alloc::ffi::CString;
use super::disk::Disk;
use alloc::sync::Arc;
use alloc::vec::Vec;

use log::*;
use crate::logging;

use lwext4_rust::bindings::{
    O_APPEND, O_CREAT, O_RDONLY, O_RDWR, O_TRUNC, O_WRONLY, SEEK_CUR, SEEK_END, SEEK_SET,
};
use lwext4_rust::{Ext4BlockWrapper, Ext4File, InodeTypes, KernelDevOp};

use virtio_drivers::device::blk::VirtIOBlk;
use virtio_drivers::transport::mmio::{MmioTransport, VirtIOHeader};
use virtio_drivers::transport::{DeviceType, Transport};

use crate::config::BLOCK_SIZE;
use crate::fs::ext4fs::Ext4Inode;


/// The Ext4 filesystem
#[allow(dead_code)]
pub struct Ext4FileSystem {
    inner: Ext4BlockWrapper<Disk>,
    root: Arc<Ext4Inode>,
}

unsafe impl Sync for Ext4FileSystem {}
unsafe impl Send for Ext4FileSystem {}

impl Ext4FileSystem {
    /// Create a new Ext4 filesystem
    pub fn new(disk: Disk) -> Self {
        info!(
            "Got Disk size:{}, position:{}",
            disk.size(),
            disk.position()
        );
        let inner = Ext4BlockWrapper::<Disk>::new(disk)
            .expect("failed to initialize EXT4 filesystem");
        let root = Arc::new(Ext4Inode::new("/", InodeTypes::EXT4_DE_DIR));
        Self { inner, root }
    }

    /// Get the root directory
    pub fn root_dir(&self) -> Arc<Ext4Inode> {
        info!("trying to get the root dir");
        Arc::clone(&self.root)
    }
}
