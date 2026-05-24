///
pub mod dentry;
///
pub mod fstype;
///
pub mod inode;
///
pub mod superblock;
///
pub mod file;
use core::sync::atomic::AtomicU64;
use core::sync::atomic::Ordering;
use crate::fs::Dentry;
use crate::fs::tmpfs::dentry::TempDentry;
use crate::fs::tmpfs::inode::TempInode;
use log::*;
use crate::fs::GLOBAL_DCACHE;
use alloc::sync::Arc;


#[allow(unused)]
///
pub fn init_tempfs(root_dentry: Arc<dyn Dentry>) {
    
}
