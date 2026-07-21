use crate::devices::BlockDevice;
use crate::error::SysError;
use crate::sync::SleepLock;
use alloc::collections::BTreeMap;
use alloc::ffi::CString;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use lazy_static::lazy_static;
use log::error;
use lwext4_rust::bindings::ext4_cache_flush;
use spin::Mutex;

lazy_static! {
    static ref LWEXT4_LOCK: SleepLock<()> = SleepLock::new(());
    static ref LWEXT4_MOUNT_GATES: Mutex<BTreeMap<usize, Arc<Lwext4MountGate>>> =
        Mutex::new(BTreeMap::new());
}

static LWEXT4_OWNER: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_OWNER_PID: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_OWNER_SYSCALL: AtomicUsize = AtomicUsize::new(usize::MAX);
static LWEXT4_RECURSION: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_PHASE: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_CALLS: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_ACQUISITIONS: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_RECURSIVE_ENTRIES: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_CONTENTIONS: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_TOTAL_WAIT_NS: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_MAX_WAIT_NS: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_TOTAL_HOLD_NS: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_MAX_HOLD_NS: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_CURRENT_OP: AtomicUsize = AtomicUsize::new(usize::MAX);
static LWEXT4_LAST_OP: AtomicUsize = AtomicUsize::new(usize::MAX);

/// Coarse operation classes used to attribute lwext4 lock contention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum Lwext4Op {
    /// Call site not yet assigned to a more specific class.
    Other = 0,
    /// Mount, unmount, and filesystem lifecycle operations.
    Mount = 1,
    /// Namespace or inode metadata mutation.
    Metadata = 2,
    /// File/directory handle open and close.
    OpenClose = 3,
    /// File data read.
    Read = 4,
    /// File data write.
    Write = 5,
    /// File-position update.
    Seek = 6,
    /// File-size update.
    Truncate = 7,
    /// Data or block-cache writeback.
    Writeback = 8,
    /// Directory iteration and lookup.
    Directory = 9,
    /// Extended-attribute operation.
    Xattr = 10,
    /// Filesystem/inode statistics query.
    Stat = 11,
}

impl Lwext4Op {
    /// Number of operation classes.
    pub const COUNT: usize = 12;

    fn from_index(index: usize) -> Option<Self> {
        Some(match index {
            0 => Self::Other,
            1 => Self::Mount,
            2 => Self::Metadata,
            3 => Self::OpenClose,
            4 => Self::Read,
            5 => Self::Write,
            6 => Self::Seek,
            7 => Self::Truncate,
            8 => Self::Writeback,
            9 => Self::Directory,
            10 => Self::Xattr,
            11 => Self::Stat,
            _ => return None,
        })
    }
}

static LWEXT4_OP_ACQUISITIONS: [AtomicUsize; Lwext4Op::COUNT] =
    [const { AtomicUsize::new(0) }; Lwext4Op::COUNT];
static LWEXT4_OP_CONTENTIONS: [AtomicUsize; Lwext4Op::COUNT] =
    [const { AtomicUsize::new(0) }; Lwext4Op::COUNT];
static LWEXT4_OP_TOTAL_WAIT_NS: [AtomicUsize; Lwext4Op::COUNT] =
    [const { AtomicUsize::new(0) }; Lwext4Op::COUNT];
static LWEXT4_OP_MAX_WAIT_NS: [AtomicUsize; Lwext4Op::COUNT] =
    [const { AtomicUsize::new(0) }; Lwext4Op::COUNT];
static LWEXT4_OP_TOTAL_HOLD_NS: [AtomicUsize; Lwext4Op::COUNT] =
    [const { AtomicUsize::new(0) }; Lwext4Op::COUNT];
static LWEXT4_OP_MAX_HOLD_NS: [AtomicUsize; Lwext4Op::COUNT] =
    [const { AtomicUsize::new(0) }; Lwext4Op::COUNT];

/// Sleepable serialization gate for one mounted ext4 instance.
pub struct Lwext4MountGate {
    mount_id: usize,
    mount_point: String,
    block_device: Arc<dyn BlockDevice>,
    lock: SleepLock<()>,
    owner: AtomicUsize,
    owner_pid: AtomicUsize,
    owner_syscall: AtomicUsize,
    recursion: AtomicUsize,
    current_operation: AtomicUsize,
    calls: AtomicUsize,
    acquisitions: AtomicUsize,
    contentions: AtomicUsize,
    total_wait_ns: AtomicUsize,
    max_wait_ns: AtomicUsize,
    total_hold_ns: AtomicUsize,
    max_hold_ns: AtomicUsize,
    namespace_generation: AtomicUsize,
    metadata_generation: AtomicUsize,
}

impl Lwext4MountGate {
    /// Create an unregistered gate for a mount being constructed.
    pub fn new(
        mount_id: usize,
        mount_point: &str,
        block_device: Arc<dyn BlockDevice>,
    ) -> Arc<Self> {
        Arc::new(Self {
            mount_id,
            mount_point: mount_point.to_string(),
            block_device,
            lock: SleepLock::new(()),
            owner: AtomicUsize::new(0),
            owner_pid: AtomicUsize::new(0),
            owner_syscall: AtomicUsize::new(usize::MAX),
            recursion: AtomicUsize::new(0),
            current_operation: AtomicUsize::new(usize::MAX),
            calls: AtomicUsize::new(0),
            acquisitions: AtomicUsize::new(0),
            contentions: AtomicUsize::new(0),
            total_wait_ns: AtomicUsize::new(0),
            max_wait_ns: AtomicUsize::new(0),
            total_hold_ns: AtomicUsize::new(0),
            max_hold_ns: AtomicUsize::new(0),
            namespace_generation: AtomicUsize::new(0),
            metadata_generation: AtomicUsize::new(0),
        })
    }

    /// Stable identifier allocated by `Ext4FsType` for this mount.
    pub fn mount_id(&self) -> usize {
        self.mount_id
    }

    /// Current mount-wide namespace generation used by negative dentries.
    pub fn namespace_generation(&self) -> usize {
        self.namespace_generation.load(Ordering::Acquire)
    }

    /// Invalidate negative dentries after a successful namespace mutation.
    pub fn note_namespace_change(&self) {
        self.namespace_generation.fetch_add(1, Ordering::AcqRel);
    }

    /// Current mount-wide generation for inode allocation metadata.
    pub fn metadata_generation(&self) -> usize {
        self.metadata_generation.load(Ordering::Acquire)
    }

    fn operation_changes_metadata(operation: Lwext4Op) -> bool {
        matches!(
            operation,
            Lwext4Op::Metadata
                | Lwext4Op::Write
                | Lwext4Op::Truncate
                | Lwext4Op::Writeback
                | Lwext4Op::Xattr
        )
    }
}

/// Non-blocking diagnostic snapshot for one ext4 mount gate.
#[derive(Debug, Clone)]
pub struct Lwext4MountLockStats {
    /// Stable mount identifier.
    pub mount_id: usize,
    /// Normalized VFS path at which this ext4 instance is mounted.
    pub mount_point: String,
    /// Blocking-lock state and waiter counts for this mount.
    pub lock: crate::sync::mutex::sleep_mutex::BlockingMutexStats,
    /// Task identity currently holding this mount gate, or zero.
    pub owner: usize,
    /// Process currently holding this mount gate, or zero.
    pub owner_pid: usize,
    /// Active syscall of the owner, when available.
    pub owner_syscall: Option<usize>,
    /// Recursive entry depth for the current owner.
    pub recursion: usize,
    /// Operation currently executing below the gate.
    pub current_operation: Option<Lwext4Op>,
    /// Total wrapper calls, including recursive calls.
    pub calls: usize,
    /// Successful outer acquisitions.
    pub acquisitions: usize,
    /// Acquisitions that initially observed the gate busy.
    pub contentions: usize,
    /// Cumulative time spent waiting for this gate.
    pub total_wait_ns: usize,
    /// Longest wait observed for this gate.
    pub max_wait_ns: usize,
    /// Cumulative time spent holding this gate.
    pub total_hold_ns: usize,
    /// Longest hold observed for this gate.
    pub max_hold_ns: usize,
}

/// Per-operation cumulative lwext4 lock measurements.
#[derive(Debug, Clone, Copy)]
pub struct Lwext4OperationStats {
    /// Operation class represented by this entry.
    pub operation: Lwext4Op,
    /// Successful outer lock acquisitions.
    pub acquisitions: usize,
    /// Acquisitions that first observed the lock busy.
    pub contentions: usize,
    /// Cumulative time spent waiting for the lock.
    pub total_wait_ns: usize,
    /// Longest observed wait duration.
    pub max_wait_ns: usize,
    /// Cumulative time holding the lock.
    pub total_hold_ns: usize,
    /// Longest observed hold duration.
    pub max_hold_ns: usize,
}

#[derive(Debug, Clone)]
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
    /// Total wrapper entries, including recursive entries.
    pub calls: usize,
    /// Successful outer lock acquisitions.
    pub acquisitions: usize,
    /// Recursive entries that reused the current owner's lock.
    pub recursive_entries: usize,
    /// Outer acquisitions that first observed the lock busy.
    pub contentions: usize,
    /// Cumulative lock wait time.
    pub total_wait_ns: usize,
    /// Longest observed lock wait.
    pub max_wait_ns: usize,
    /// Cumulative outer lock hold time.
    pub total_hold_ns: usize,
    /// Longest observed outer lock hold time.
    pub max_hold_ns: usize,
    /// Operation currently holding the lock, when any.
    pub current_operation: Option<Lwext4Op>,
    /// Most recently completed outer operation.
    pub last_operation: Option<Lwext4Op>,
    /// Measurements split by operation class.
    pub operations: [Lwext4OperationStats; Lwext4Op::COUNT],
    /// Independent data-path gates, one for each registered ext4 mount.
    pub mounts: Vec<Lwext4MountLockStats>,
    /// Whether the mount registry was busy and `mounts` had to be omitted.
    pub mount_registry_busy: bool,
}

/// Return a non-blocking diagnostic snapshot of the lwext4 gate.
pub fn lwext4_lock_stats() -> Lwext4LockStats {
    let (mount_registry_busy, mounts) = if let Some(gates) = LWEXT4_MOUNT_GATES.try_lock() {
        (
            false,
            gates
                .values()
                .map(|gate| Lwext4MountLockStats {
                    mount_id: gate.mount_id,
                    mount_point: gate.mount_point.clone(),
                    lock: gate.lock.stats(),
                    owner: gate.owner.load(Ordering::Acquire),
                    owner_pid: gate.owner_pid.load(Ordering::Acquire),
                    owner_syscall: match gate.owner_syscall.load(Ordering::Acquire) {
                        usize::MAX => None,
                        syscall_id => Some(syscall_id),
                    },
                    recursion: gate.recursion.load(Ordering::Acquire),
                    current_operation: Lwext4Op::from_index(
                        gate.current_operation.load(Ordering::Acquire),
                    ),
                    calls: gate.calls.load(Ordering::Acquire),
                    acquisitions: gate.acquisitions.load(Ordering::Acquire),
                    contentions: gate.contentions.load(Ordering::Acquire),
                    total_wait_ns: gate.total_wait_ns.load(Ordering::Acquire),
                    max_wait_ns: gate.max_wait_ns.load(Ordering::Acquire),
                    total_hold_ns: gate.total_hold_ns.load(Ordering::Acquire),
                    max_hold_ns: gate.max_hold_ns.load(Ordering::Acquire),
                })
                .collect(),
        )
    } else {
        (true, Vec::new())
    };
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
        calls: LWEXT4_CALLS.load(Ordering::Acquire),
        acquisitions: LWEXT4_ACQUISITIONS.load(Ordering::Acquire),
        recursive_entries: LWEXT4_RECURSIVE_ENTRIES.load(Ordering::Acquire),
        contentions: LWEXT4_CONTENTIONS.load(Ordering::Acquire),
        total_wait_ns: LWEXT4_TOTAL_WAIT_NS.load(Ordering::Acquire),
        max_wait_ns: LWEXT4_MAX_WAIT_NS.load(Ordering::Acquire),
        total_hold_ns: LWEXT4_TOTAL_HOLD_NS.load(Ordering::Acquire),
        max_hold_ns: LWEXT4_MAX_HOLD_NS.load(Ordering::Acquire),
        current_operation: Lwext4Op::from_index(LWEXT4_CURRENT_OP.load(Ordering::Acquire)),
        last_operation: Lwext4Op::from_index(LWEXT4_LAST_OP.load(Ordering::Acquire)),
        operations: core::array::from_fn(|index| Lwext4OperationStats {
            operation: Lwext4Op::from_index(index).unwrap(),
            acquisitions: LWEXT4_OP_ACQUISITIONS[index].load(Ordering::Acquire),
            contentions: LWEXT4_OP_CONTENTIONS[index].load(Ordering::Acquire),
            total_wait_ns: LWEXT4_OP_TOTAL_WAIT_NS[index].load(Ordering::Acquire),
            max_wait_ns: LWEXT4_OP_MAX_WAIT_NS[index].load(Ordering::Acquire),
            total_hold_ns: LWEXT4_OP_TOTAL_HOLD_NS[index].load(Ordering::Acquire),
            max_hold_ns: LWEXT4_OP_MAX_HOLD_NS[index].load(Ordering::Acquire),
        }),
        mounts,
        mount_registry_busy,
    }
}

/// Publish a successfully mounted ext4 gate to data-path lookups.
pub fn register_lwext4_mount_gate(gate: Arc<Lwext4MountGate>) {
    LWEXT4_MOUNT_GATES.lock().insert(gate.mount_id, gate);
}

/// Remove an ext4 gate after its C mount has been torn down.
pub fn unregister_lwext4_mount_gate(mount_id: usize) {
    LWEXT4_MOUNT_GATES.lock().remove(&mount_id);
}

fn path_belongs_to_mount(path: &str, mount_point: &str) -> bool {
    if mount_point == "/" {
        return path.starts_with('/');
    }
    let mount_point = mount_point.trim_end_matches('/');
    path == mount_point
        || path
            .strip_prefix(mount_point)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

/// Resolve the most specific registered ext4 mount containing `path`.
pub fn lwext4_mount_gate_for_path(path: &str) -> Option<Arc<Lwext4MountGate>> {
    LWEXT4_MOUNT_GATES
        .lock()
        .values()
        .filter(|gate| path_belongs_to_mount(path, &gate.mount_point))
        .max_by_key(|gate| gate.mount_point.trim_end_matches('/').len())
        .cloned()
}

fn flush_lwext4_mount_locked(gate: &Lwext4MountGate) -> Result<(), SysError> {
    let mount_point = CString::new(gate.mount_point.as_str()).map_err(|_| SysError::EINVAL)?;
    let ret = unsafe { ext4_cache_flush(mount_point.as_ptr()) };
    if ret != 0 {
        let error = lwext4_err_to_sys(ret);
        error!(
            "[EXT4_MOUNT_SYNC] cache flush failed: mount={} ret={} error={:?}",
            gate.mount_point, ret, error
        );
        return Err(error);
    }
    gate.block_device.flush().map_err(|error| {
        error!(
            "[EXT4_MOUNT_SYNC] device flush failed: mount={} error={:?}",
            gate.mount_point, error
        );
        error
    })
}

/// Flush one ext4 mount's block cache and then issue a storage barrier.
pub fn flush_lwext4_mount(gate: &Lwext4MountGate) -> Result<(), SysError> {
    with_lwext4_mount_lock_op(gate, Lwext4Op::Writeback, || {
        flush_lwext4_mount_locked(gate)
    })
}

/// Flush every registered ext4 mount and its backing block device.
pub fn flush_all_lwext4_mounts() -> Result<(), SysError> {
    with_lwext4_global_lock_op(Lwext4Op::Writeback, || {
        let gates: Vec<_> = LWEXT4_MOUNT_GATES.lock().values().cloned().collect();
        for gate in gates {
            with_lwext4_mount_lock_op(&gate, Lwext4Op::Writeback, || {
                flush_lwext4_mount_locked(&gate)
            })?;
        }
        Ok(())
    })
}

fn monotonic_now_ns() -> usize {
    polyhal::timer::current_time().as_nanos() as usize
}

fn update_max(target: &AtomicUsize, value: usize) {
    let mut current = target.load(Ordering::Relaxed);
    while value > current {
        match target.compare_exchange_weak(current, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return,
            Err(observed) => current = observed,
        }
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

/// Run an uncategorized operation while excluding mount-table lifecycle work.
///
/// New data-path call sites should use [`with_lwext4_mount_lock_op`].  This
/// compatibility entry point takes every registered mount gate and is kept for
/// uncommon paths that have not got an explicit mount identity.
pub fn with_lwext4_lock<R>(f: impl FnOnce() -> R) -> R {
    with_lwext4_lock_op(Lwext4Op::Other, f)
}

/// Run a categorized compatibility operation while excluding every mount.
pub fn with_lwext4_lock_op<R>(operation: Lwext4Op, f: impl FnOnce() -> R) -> R {
    let owner = current_lwext4_context().0;
    with_lwext4_global_lock_op(operation, || {
        let gates: Vec<_> = LWEXT4_MOUNT_GATES.lock().values().cloned().collect();
        let _guards: Vec<_> = gates
            .iter()
            .filter(|gate| gate.owner.load(Ordering::Acquire) != owner)
            .map(|gate| gate.lock.lock())
            .collect();
        f()
    })
}

/// Run a mount/unmount operation after quiescing every existing ext4 mount.
pub fn with_lwext4_lifecycle_lock_op<R>(operation: Lwext4Op, f: impl FnOnce() -> R) -> R {
    with_lwext4_lock_op(operation, f)
}

fn with_lwext4_global_lock_op<R>(operation: Lwext4Op, f: impl FnOnce() -> R) -> R {
    LWEXT4_CALLS.fetch_add(1, Ordering::Relaxed);
    // Capture task identity before acquiring LWEXT4_LOCK. Calling
    // current_task() after acquisition nests the per-CPU PROCESSOR lock below
    // the global filesystem gate and can leave every other filesystem caller
    // blocked behind an owner that is itself waiting for scheduler state.
    let (owner, owner_pid, owner_syscall, in_task_context) = current_lwext4_context();
    if LWEXT4_OWNER.load(Ordering::Acquire) == owner {
        LWEXT4_RECURSIVE_ENTRIES.fetch_add(1, Ordering::Relaxed);
        LWEXT4_RECURSION.fetch_add(1, Ordering::Relaxed);
        let ret = f();
        LWEXT4_RECURSION.fetch_sub(1, Ordering::Release);
        return ret;
    }

    let wait_started = monotonic_now_ns();
    let (guard, contended) = match LWEXT4_LOCK.try_lock() {
        Some(guard) => (guard, false),
        None if in_task_context => (LWEXT4_LOCK.lock(), true),
        None => loop {
            if let Some(guard) = LWEXT4_LOCK.try_lock() {
                break (guard, true);
            }
            core::hint::spin_loop();
        },
    };
    let acquired_at = monotonic_now_ns();
    let wait_ns = acquired_at.saturating_sub(wait_started);
    let operation_index = operation as usize;
    LWEXT4_ACQUISITIONS.fetch_add(1, Ordering::Relaxed);
    LWEXT4_TOTAL_WAIT_NS.fetch_add(wait_ns, Ordering::Relaxed);
    update_max(&LWEXT4_MAX_WAIT_NS, wait_ns);
    LWEXT4_OP_ACQUISITIONS[operation_index].fetch_add(1, Ordering::Relaxed);
    LWEXT4_OP_TOTAL_WAIT_NS[operation_index].fetch_add(wait_ns, Ordering::Relaxed);
    update_max(&LWEXT4_OP_MAX_WAIT_NS[operation_index], wait_ns);
    if contended {
        LWEXT4_CONTENTIONS.fetch_add(1, Ordering::Relaxed);
        LWEXT4_OP_CONTENTIONS[operation_index].fetch_add(1, Ordering::Relaxed);
    }
    LWEXT4_PHASE.store(2, Ordering::Release);
    LWEXT4_OWNER.store(owner, Ordering::Release);
    LWEXT4_OWNER_PID.store(owner_pid, Ordering::Release);
    LWEXT4_OWNER_SYSCALL.store(owner_syscall.unwrap_or(usize::MAX), Ordering::Release);
    LWEXT4_RECURSION.store(1, Ordering::Release);
    LWEXT4_CURRENT_OP.store(operation_index, Ordering::Release);
    LWEXT4_PHASE.store(3, Ordering::Release);
    let ret = f();
    let hold_ns = monotonic_now_ns().saturating_sub(acquired_at);
    LWEXT4_TOTAL_HOLD_NS.fetch_add(hold_ns, Ordering::Relaxed);
    update_max(&LWEXT4_MAX_HOLD_NS, hold_ns);
    LWEXT4_OP_TOTAL_HOLD_NS[operation_index].fetch_add(hold_ns, Ordering::Relaxed);
    update_max(&LWEXT4_OP_MAX_HOLD_NS[operation_index], hold_ns);
    LWEXT4_LAST_OP.store(operation_index, Ordering::Release);
    LWEXT4_CURRENT_OP.store(usize::MAX, Ordering::Release);
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

/// Run one operation under the gate belonging to a single ext4 mount.
pub fn with_lwext4_mount_lock_op<R>(
    gate: &Lwext4MountGate,
    operation: Lwext4Op,
    f: impl FnOnce() -> R,
) -> R {
    LWEXT4_CALLS.fetch_add(1, Ordering::Relaxed);
    gate.calls.fetch_add(1, Ordering::Relaxed);
    let (owner, owner_pid, owner_syscall, in_task_context) = current_lwext4_context();
    if gate.owner.load(Ordering::Acquire) == owner {
        LWEXT4_RECURSIVE_ENTRIES.fetch_add(1, Ordering::Relaxed);
        gate.recursion.fetch_add(1, Ordering::Relaxed);
        let ret = f();
        if Lwext4MountGate::operation_changes_metadata(operation) {
            gate.metadata_generation.fetch_add(1, Ordering::AcqRel);
        }
        gate.recursion.fetch_sub(1, Ordering::Release);
        return ret;
    }

    let wait_started = monotonic_now_ns();
    let (guard, contended) = match gate.lock.try_lock() {
        Some(guard) => (guard, false),
        None if in_task_context => (gate.lock.lock(), true),
        None => loop {
            if let Some(guard) = gate.lock.try_lock() {
                break (guard, true);
            }
            core::hint::spin_loop();
        },
    };
    let acquired_at = monotonic_now_ns();
    let wait_ns = acquired_at.saturating_sub(wait_started);
    let operation_index = operation as usize;

    LWEXT4_ACQUISITIONS.fetch_add(1, Ordering::Relaxed);
    LWEXT4_TOTAL_WAIT_NS.fetch_add(wait_ns, Ordering::Relaxed);
    update_max(&LWEXT4_MAX_WAIT_NS, wait_ns);
    LWEXT4_OP_ACQUISITIONS[operation_index].fetch_add(1, Ordering::Relaxed);
    LWEXT4_OP_TOTAL_WAIT_NS[operation_index].fetch_add(wait_ns, Ordering::Relaxed);
    update_max(&LWEXT4_OP_MAX_WAIT_NS[operation_index], wait_ns);
    gate.acquisitions.fetch_add(1, Ordering::Relaxed);
    gate.total_wait_ns.fetch_add(wait_ns, Ordering::Relaxed);
    update_max(&gate.max_wait_ns, wait_ns);
    if contended {
        LWEXT4_CONTENTIONS.fetch_add(1, Ordering::Relaxed);
        LWEXT4_OP_CONTENTIONS[operation_index].fetch_add(1, Ordering::Relaxed);
        gate.contentions.fetch_add(1, Ordering::Relaxed);
    }

    gate.owner.store(owner, Ordering::Release);
    gate.owner_pid.store(owner_pid, Ordering::Release);
    gate.owner_syscall
        .store(owner_syscall.unwrap_or(usize::MAX), Ordering::Release);
    gate.recursion.store(1, Ordering::Release);
    gate.current_operation
        .store(operation_index, Ordering::Release);
    let ret = f();
    if Lwext4MountGate::operation_changes_metadata(operation) {
        gate.metadata_generation.fetch_add(1, Ordering::AcqRel);
    }
    let hold_ns = monotonic_now_ns().saturating_sub(acquired_at);

    LWEXT4_TOTAL_HOLD_NS.fetch_add(hold_ns, Ordering::Relaxed);
    update_max(&LWEXT4_MAX_HOLD_NS, hold_ns);
    LWEXT4_OP_TOTAL_HOLD_NS[operation_index].fetch_add(hold_ns, Ordering::Relaxed);
    update_max(&LWEXT4_OP_MAX_HOLD_NS[operation_index], hold_ns);
    gate.total_hold_ns.fetch_add(hold_ns, Ordering::Relaxed);
    update_max(&gate.max_hold_ns, hold_ns);
    LWEXT4_LAST_OP.store(operation_index, Ordering::Release);

    gate.current_operation.store(usize::MAX, Ordering::Release);
    gate.recursion.store(0, Ordering::Release);
    gate.owner_syscall.store(usize::MAX, Ordering::Release);
    gate.owner_pid.store(0, Ordering::Release);
    gate.owner.store(0, Ordering::Release);
    drop(guard);
    ret
}

/// Resolve `path` and run one operation under that mount's data-path gate.
pub fn with_lwext4_path_lock_op<R>(
    path: &str,
    operation: Lwext4Op,
    f: impl FnOnce() -> R,
) -> Result<R, SysError> {
    let gate = lwext4_mount_gate_for_path(path).ok_or(SysError::EIO)?;
    Ok(with_lwext4_mount_lock_op(&gate, operation, f))
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
