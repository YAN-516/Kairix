
use core::cell::RefCell;
use core::ptr::NonNull;

use alloc::string::String;
use alloc::ffi::CString;
use super::disk::Disk;
use alloc::sync::Arc;
use alloc::vec::Vec;
use crate::drivers::block::{self, BLOCK_DEVICE};
use log::*;
use crate::fs::vfs::superblock::{SuperBlock,SuperBlockInner};
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


/// The Ext4SuperBlock
#[allow(dead_code)]
pub struct Ext4SuperBlock {
    inner:SuperBlockInner,
    block: Ext4BlockWrapper<Disk>,
}

unsafe impl Sync for Ext4SuperBlock {}
unsafe impl Send for Ext4SuperBlock {}

impl Ext4SuperBlock {
    /// Create a new Ext4 super block
    pub fn new(inner:SuperBlockInner) -> Self {
        // let disk =Disk::new(BLOCK_DEVICE.clone());
        let block_device = inner.device.as_ref().unwrap().clone();
        let disk = Disk::new(block_device);

        info!(
            "Got Disk size:{}, position:{}",
            disk.size(),
            disk.position()
        );
        let block = Ext4BlockWrapper::<Disk>::new(disk)
            .expect("failed to initialize EXT4 filesystem");
       
        Self { inner, block }
    }
}
impl SuperBlock for Ext4SuperBlock {
    fn inner(&self) -> &SuperBlockInner {
        &self.inner
    }
}

