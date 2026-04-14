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

// 全局的 Tmpfs Inode 发号器，从 10000 开始，避免和其它文件系统冲突
static TMPFS_INO_COUNTER: AtomicU64 = AtomicU64::new(10000);
/// 获取下一个 Tmpfs Inode 号
pub fn next_tmpfs_ino() -> u64 {
    TMPFS_INO_COUNTER.fetch_add(1, Ordering::SeqCst)
}