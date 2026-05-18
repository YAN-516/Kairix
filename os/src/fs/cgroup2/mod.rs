#![allow(missing_docs)]
pub mod dentry;
pub mod file;
pub mod fstype;
pub mod inode;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use crate::sync::SpinNoIrqLock;

lazy_static::lazy_static! {
    /// 全局 cgroup 表：key 为 cgroup 目录的绝对路径，value 为该 cgroup 中的进程 PID 列表
    pub static ref CGROUP_TABLE: SpinNoIrqLock<BTreeMap<String, Vec<usize>>> = SpinNoIrqLock::new(BTreeMap::new());
}
