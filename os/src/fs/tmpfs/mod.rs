///
pub mod dentry;
///
pub mod file;
///
pub mod fstype;
///
pub mod inode;
///
pub mod superblock;
use crate::fs::Dentry;
use crate::fs::GLOBAL_DCACHE;
use crate::fs::tmpfs::dentry::TempDentry;
use crate::fs::tmpfs::inode::TempInode;
use alloc::sync::Arc;
use core::sync::atomic::AtomicU64;
use core::sync::atomic::Ordering;
use log::*;

#[allow(unused)]
///
pub fn init_tempfs(root_dentry: Arc<dyn Dentry>) {}
