use crate::error::SysError;
use crate::sync::SleepLock;
use core::sync::atomic::{AtomicUsize, Ordering};
use lazy_static::lazy_static;

lazy_static! {
    static ref LWEXT4_LOCK: SleepLock<()> = SleepLock::new(());
}

static LWEXT4_OWNER: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_OWNER_PID: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_OWNER_SYSCALL: AtomicUsize = AtomicUsize::new(usize::MAX);
static LWEXT4_RECURSION: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_PHASE: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Copy)]
/// Snapshot of the global lwext4 serialization gate.
pub struct Lwext4LockStats {
    /// Generic blocking-lock state and waiter counts.
    pub lock: crate::sync::mutex::sleep_mutex::BlockingMutexStats,
    /// Task identity currently recorded as the recursive owner, or zero.
    pub owner: usize,
    /// Process owning the gate, or zero outside task context.
    pub owner_pid: usize,
    /// Active syscall of the owning task, when available.
    pub owner_syscall: Option<usize>,
    /// Current recursive entry depth for the recorded owner.
    pub recursion: usize,
    /// Owner progress: 0 idle, 2 acquired, 3 in operation, 4 operation done,
    /// 5 metadata cleared, 6 releasing the blocking mutex.
    pub phase: usize,
}

/// Return a non-blocking diagnostic snapshot of the lwext4 gate.
pub fn lwext4_lock_stats() -> Lwext4LockStats {
    Lwext4LockStats {
        lock: LWEXT4_LOCK.stats(),
        owner: LWEXT4_OWNER.load(Ordering::Acquire),
        owner_pid: LWEXT4_OWNER_PID.load(Ordering::Acquire),
        owner_syscall: match LWEXT4_OWNER_SYSCALL.load(Ordering::Acquire) {
            usize::MAX => None,
            syscall_id => Some(syscall_id),
        },
        recursion: LWEXT4_RECURSION.load(Ordering::Acquire),
        phase: LWEXT4_PHASE.load(Ordering::Acquire),
    }
}

fn current_lwext4_context() -> (usize, usize, Option<usize>, bool) {
    if let Some(task) = crate::task::current_task() {
        let owner = alloc::sync::Arc::as_ptr(&task) as usize;
        let owner_pid = task
            .process
            .upgrade()
            .map(|process| process.getpid())
            .unwrap_or(0);
        return (owner, owner_pid, task.active_syscall(), true);
    }
    #[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
    {
        (usize::MAX - polyhal::arch::hart_id(), 0, None, false)
    }
    #[cfg(not(any(target_arch = "riscv64", target_arch = "loongarch64")))]
    {
        (usize::MAX, 0, None, false)
    }
}

/// Run one lwext4 operation while holding the kernel-side ext4 gate.
///
/// The C lwext4 layer keeps shared mount/cache state behind the Rust file
/// handles, so different `Ext4File` objects must not enter it concurrently.
pub fn with_lwext4_lock<R>(f: impl FnOnce() -> R) -> R {
    // Capture task identity before acquiring LWEXT4_LOCK. Calling
    // current_task() after acquisition nests the per-CPU PROCESSOR lock below
    // the global filesystem gate and can leave every other filesystem caller
    // blocked behind an owner that is itself waiting for scheduler state.
    let (owner, owner_pid, owner_syscall, in_task_context) = current_lwext4_context();
    if LWEXT4_OWNER.load(Ordering::Acquire) == owner {
        LWEXT4_RECURSION.fetch_add(1, Ordering::Relaxed);
        let ret = f();
        LWEXT4_RECURSION.fetch_sub(1, Ordering::Release);
        return ret;
    }

    let guard = match LWEXT4_LOCK.try_lock() {
        Some(guard) => guard,
        None if in_task_context => LWEXT4_LOCK.lock(),
        None => loop {
            if let Some(guard) = LWEXT4_LOCK.try_lock() {
                break guard;
            }
            core::hint::spin_loop();
        },
    };
    LWEXT4_PHASE.store(2, Ordering::Release);
    LWEXT4_OWNER.store(owner, Ordering::Release);
    LWEXT4_OWNER_PID.store(owner_pid, Ordering::Release);
    LWEXT4_OWNER_SYSCALL.store(owner_syscall.unwrap_or(usize::MAX), Ordering::Release);
    LWEXT4_RECURSION.store(1, Ordering::Release);
    LWEXT4_PHASE.store(3, Ordering::Release);
    let ret = f();
    LWEXT4_PHASE.store(4, Ordering::Release);
    LWEXT4_RECURSION.store(0, Ordering::Release);
    LWEXT4_OWNER_SYSCALL.store(usize::MAX, Ordering::Release);
    LWEXT4_OWNER_PID.store(0, Ordering::Release);
    LWEXT4_OWNER.store(0, Ordering::Release);
    LWEXT4_PHASE.store(5, Ordering::Release);
    LWEXT4_PHASE.store(6, Ordering::Release);
    drop(guard);
    // A woken waiter may acquire and publish phase 2 immediately after the
    // guard drops. Do not overwrite that newer owner's phase with idle.
    let _ = LWEXT4_PHASE.compare_exchange(6, 0, Ordering::AcqRel, Ordering::Acquire);
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
