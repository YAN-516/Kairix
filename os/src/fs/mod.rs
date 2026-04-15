//! File system in os
mod stdio;
pub mod vfs;
///
pub mod lwext4;
// pub mod fat32;
/// page cache
pub mod page;
pub use lwext4::file::{Ext4File, open_file};
use polyhal::println;
pub use stdio::{Stdin, Stdout};
pub use vfs::file::File;
use alloc::{collections::btree_map::BTreeMap, string::{String, ToString}, sync::Arc};
use log::*;
use crate::{drivers::BLOCK_DEVICE};
pub use crate::fs::lwext4::superblock::Ext4SuperBlock;
pub use vfs::superblock::{SuperBlock, SuperBlockInner};
use lwext4_rust::{InodeTypes};
use crate::fs::lwext4::dentry::Ext4Dentry;
use crate::fs::lwext4::inode::Ext4Inode;
use crate::fs::vfs::dcache::GLOBAL_DCACHE;
use crate::fs::vfs::inode::Inode;
use crate::sync::UPSafeCell;
use lazy_static::lazy_static;
use crate::fs::vfs::mount::Mountdata;
use crate::fs::vfs::Dentry;
use crate::fs::vfs::mount::MOUNT_TABLE;
/// init the file system
pub fn init() {
    let root_inode = Arc::new(Ext4Inode::new(0, InodeTypes::EXT4_DE_DIR)) as Arc<dyn Inode>;
    //root_dentry dont have parent
    println!("root inode get");
    let root_dentry = Ext4Dentry::new("/", None);
    println!("root dentry get");
    GLOBAL_DCACHE.insert("/".to_string(), root_dentry.clone());
    root_dentry.set_inode(root_inode);
    // SuperBlock should contain root_dentry
    println!("create super block");
    // info!("BLOCK_DEVICE size: {}", BLOCK_DEVICE.size());
    // info!("BLOCK_DEVICE block_size: {}", BLOCK_DEVICE.block_size());
    let mut buf = [0u8; 512];
    BLOCK_DEVICE.read_block(8, &mut buf);  // ext4 超级块通常在块 0 或偏移 1024 处
    error!("Block 8 data: {:02x?}", &buf[..64]);

    let lwext4_superblock = Arc::new(Ext4SuperBlock::new(
        SuperBlockInner::new(
            Some(BLOCK_DEVICE.clone()), 
            Some(root_dentry.clone()) 
        )
    ));
    println!("super block get");

    let root_record = Mountdata {
        mount_point: "/".to_string(),
        odentry: root_dentry.clone() as Arc<dyn Dentry>, 
        ndentry: root_dentry.clone() as Arc<dyn Dentry>,
        superblock: lwext4_superblock.clone() as Arc<dyn SuperBlock>,
    };
    MOUNT_TABLE.lock().insert("/".to_string(), root_record);

    info!("Root filesystem mounted at '/' successfully.");
}
