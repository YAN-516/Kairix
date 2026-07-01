use crate::error::SysError;
use crate::sync::SleepLock;
use core::sync::atomic::{AtomicUsize, Ordering};
use lazy_static::lazy_static;

lazy_static! {
    static ref LWEXT4_LOCK: SleepLock<()> = SleepLock::new(());
}

static LWEXT4_OWNER: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_RECURSION: AtomicUsize = AtomicUsize::new(0);

fn current_lwext4_owner() -> usize {
    if let Some(task) = crate::task::current_task() {
        return alloc::sync::Arc::as_ptr(&task) as usize;
    }
    #[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
    {
        usize::MAX - polyhal::arch::hart_id()
    }
    #[cfg(not(any(target_arch = "riscv64", target_arch = "loongarch64")))]
    {
        usize::MAX
    }
}

/// Run one lwext4 operation while holding the kernel-side ext4 gate.
///
/// The C lwext4 layer keeps shared mount/cache state behind the Rust file
/// handles, so different `Ext4File` objects must not enter it concurrently.
pub fn with_lwext4_lock<R>(f: impl FnOnce() -> R) -> R {
    let owner = current_lwext4_owner();
    if LWEXT4_OWNER.load(Ordering::Acquire) == owner {
        LWEXT4_RECURSION.fetch_add(1, Ordering::Relaxed);
        let ret = f();
        LWEXT4_RECURSION.fetch_sub(1, Ordering::Release);
        return ret;
    }

    let _guard = match LWEXT4_LOCK.try_lock() {
        Some(guard) => guard,
        None if crate::task::current_task().is_some() => LWEXT4_LOCK.lock(),
        None => loop {
            if let Some(guard) = LWEXT4_LOCK.try_lock() {
                break guard;
            }
            core::hint::spin_loop();
        },
    };
    LWEXT4_OWNER.store(owner, Ordering::Release);
    LWEXT4_RECURSION.store(1, Ordering::Release);
    let ret = f();
    LWEXT4_RECURSION.store(0, Ordering::Release);
    LWEXT4_OWNER.store(0, Ordering::Release);
    ret
}

/// Convert a lwext4 C FFI error code to a [`SysError`].
///
/// lwext4 APIs in this tree may return either positive or negative errno values.
pub fn lwext4_err_to_sys(err: i32) -> SysError {
    SysError::try_from(err.abs()).unwrap_or(SysError::EIO)
}

///
pub mod dentry;
pub mod disk;
///
pub mod ext4;
///
pub mod file;
///vfs file system type
pub mod fstype;
///
pub mod inode;
///
pub mod superblock;
