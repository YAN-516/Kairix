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
use spin::{Mutex, RwLock};

lazy_static! {
    static ref LWEXT4_LOCK: SleepLock<()> = SleepLock::new_fair(());
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
static LWEXT4_JOURNAL_PHASE: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_JOURNAL_OWNER: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_JOURNAL_DETAIL: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_FLUSH_PHASE: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_FLUSH_OWNER: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_FLUSH_LBA: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_BCACHE_PHASE: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_BCACHE_OWNER: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_BCACHE_CONTENTIONS: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_BCACHE_WAITER: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_BCACHE_OWNER_SITE: AtomicUsize = AtomicUsize::new(0);

/// Allocation- and lock-free C-side progress used by remote-CPU watchdogs.
#[derive(Debug, Clone, Copy)]
pub struct Lwext4CProgress {
    /// Journal: 0=idle, 1=waiting, 2=held, 3=releasing.
    pub journal_phase: usize,
    /// Stable task identity currently waiting for or holding the journal.
    pub journal_owner: usize,
    /// Owner while waiting, otherwise recursive depth.
    pub journal_detail: usize,
    /// Flush: 0=idle, 1=physical I/O, 2=I/O complete, 3=callback.
    pub flush_phase: usize,
    /// Stable task identity performing the most recently published flush.
    pub flush_owner: usize,
    /// Logical block address associated with the current flush phase.
    pub flush_lba: usize,
    /// Block-cache bookkeeping: 0=no observed waiter, 1=waiting.
    pub bcache_phase: usize,
    /// Stable task identity holding block-cache bookkeeping while a waiter exists.
    pub bcache_owner: usize,
    /// Cumulative contended block-cache bookkeeping acquisitions.
    pub bcache_contentions: usize,
    /// Stable task identity currently waiting for block-cache bookkeeping.
    pub bcache_waiter: usize,
    /// Return address at which the current holder acquired bookkeeping.
    pub bcache_owner_site: usize,
}

/// Read C progress without entering the mount registry or any filesystem lock.
pub fn lwext4_c_progress() -> Lwext4CProgress {
    Lwext4CProgress {
        journal_phase: LWEXT4_JOURNAL_PHASE.load(Ordering::Acquire),
        journal_owner: LWEXT4_JOURNAL_OWNER.load(Ordering::Acquire),
        journal_detail: LWEXT4_JOURNAL_DETAIL.load(Ordering::Acquire),
        flush_phase: LWEXT4_FLUSH_PHASE.load(Ordering::Acquire),
        flush_owner: LWEXT4_FLUSH_OWNER.load(Ordering::Acquire),
        flush_lba: LWEXT4_FLUSH_LBA.load(Ordering::Acquire),
        bcache_phase: LWEXT4_BCACHE_PHASE.load(Ordering::Acquire),
        bcache_owner: LWEXT4_BCACHE_OWNER.load(Ordering::Acquire),
        bcache_contentions: LWEXT4_BCACHE_CONTENTIONS.load(Ordering::Acquire),
        bcache_waiter: LWEXT4_BCACHE_WAITER.load(Ordering::Acquire),
        bcache_owner_site: LWEXT4_BCACHE_OWNER_SITE.load(Ordering::Acquire),
    }
}

/// Receive lock-free progress publications from the bundled lwext4 C code.
#[unsafe(no_mangle)]
pub extern "C" fn ext4_lock_progress(domain: u32, phase: u32, owner: usize, detail: u64) {
    let detail = usize::try_from(detail).unwrap_or(usize::MAX);
    match domain {
        1 => {
            LWEXT4_JOURNAL_OWNER.store(owner, Ordering::Relaxed);
            LWEXT4_JOURNAL_DETAIL.store(detail, Ordering::Relaxed);
            LWEXT4_JOURNAL_PHASE.store(phase as usize, Ordering::Release);
        }
        2 => {
            LWEXT4_FLUSH_OWNER.store(owner, Ordering::Relaxed);
            LWEXT4_FLUSH_LBA.store(detail, Ordering::Relaxed);
            LWEXT4_FLUSH_PHASE.store(phase as usize, Ordering::Release);
        }
        3 => {
            LWEXT4_BCACHE_OWNER.store(owner, Ordering::Relaxed);
            LWEXT4_BCACHE_CONTENTIONS.store(detail, Ordering::Relaxed);
            LWEXT4_BCACHE_PHASE.store(phase as usize, Ordering::Release);
        }
        4 => {
            LWEXT4_BCACHE_WAITER.store(owner, Ordering::Relaxed);
            LWEXT4_BCACHE_OWNER_SITE.store(detail, Ordering::Relaxed);
        }
        _ => {}
    }
}

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

    fn uses_shared_mount_gate(self) -> bool {
        matches!(
            self,
            Self::Metadata
                | Self::Read
                | Self::Write
                | Self::Seek
                | Self::Truncate
                | Self::Writeback
                | Self::Directory
                | Self::Xattr
                | Self::Stat
        )
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
const LWEXT4_NAMESPACE_GENERATION_SHARDS: usize = 256;

/// Cooperative reader/writer gate for one mounted ext4 instance.
pub struct Lwext4MountGate {
    mount_id: usize,
    mount_point: String,
    block_device: Arc<dyn BlockDevice>,
    lock: RwLock<()>,
    active_readers: AtomicUsize,
    max_active_readers: AtomicUsize,
    writer_active: AtomicUsize,
    waiting_writers: AtomicUsize,
    reader_owners: Mutex<Vec<(usize, usize)>>,
    namespace_lock: SleepLock<()>,
    namespace_owner: AtomicUsize,
    namespace_recursion: AtomicUsize,
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
    namespace_generations: [AtomicUsize; LWEXT4_NAMESPACE_GENERATION_SHARDS],
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
            lock: RwLock::new(()),
            active_readers: AtomicUsize::new(0),
            max_active_readers: AtomicUsize::new(0),
            writer_active: AtomicUsize::new(0),
            waiting_writers: AtomicUsize::new(0),
            reader_owners: Mutex::new(Vec::new()),
            namespace_lock: SleepLock::new_fair(()),
            namespace_owner: AtomicUsize::new(0),
            namespace_recursion: AtomicUsize::new(0),
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
            namespace_generations: [const { AtomicUsize::new(0) };
                LWEXT4_NAMESPACE_GENERATION_SHARDS],
            metadata_generation: AtomicUsize::new(0),
        })
    }

    /// Stable identifier allocated by `Ext4FsType` for this mount.
    pub fn mount_id(&self) -> usize {
        self.mount_id
    }

    fn namespace_generation_shard(namespace_key: usize) -> usize {
        let mixed = namespace_key ^ (namespace_key >> 11) ^ (namespace_key >> 23);
        mixed & (LWEXT4_NAMESPACE_GENERATION_SHARDS - 1)
    }

    /// Current generation for the directory shard containing `namespace_key`.
    /// Collisions only cause conservative extra invalidation.
    pub fn namespace_generation(&self, namespace_key: usize) -> usize {
        self.namespace_generations[Self::namespace_generation_shard(namespace_key)]
            .load(Ordering::Acquire)
    }

    /// Invalidate negative dentries for the mutated directory shard.
    pub fn note_namespace_change(&self, namespace_key: usize) {
        self.namespace_generations[Self::namespace_generation_shard(namespace_key)]
            .fetch_add(1, Ordering::AcqRel);
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

    fn reader_owned_by(&self, owner: usize) -> bool {
        self.reader_owners
            .lock()
            .iter()
            .any(|(candidate, depth)| *candidate == owner && *depth != 0)
    }

    fn note_reader_acquired(&self, owner: usize) {
        let mut owners = self.reader_owners.lock();
        if let Some((_, depth)) = owners.iter_mut().find(|(candidate, _)| *candidate == owner) {
            *depth += 1;
        } else {
            owners.push((owner, 1));
        }
        let active = self.active_readers.fetch_add(1, Ordering::AcqRel) + 1;
        update_max(&self.max_active_readers, active);
    }

    fn note_reader_released(&self, owner: usize) {
        let mut owners = self.reader_owners.lock();
        let position = owners
            .iter()
            .position(|(candidate, _)| *candidate == owner)
            .expect("lwext4 read gate owner missing");
        if owners[position].1 == 1 {
            owners.swap_remove(position);
        } else {
            owners[position].1 -= 1;
        }
        self.active_readers.fetch_sub(1, Ordering::Release);
    }
}

/// Non-blocking state snapshot for a mount's reader/writer gate.
#[derive(Debug, Clone, Copy)]
pub struct Lwext4MountRwLockStats {
    /// Number of currently active shared holders.
    pub active_readers: usize,
    /// Highest number of simultaneous shared holders since mount.
    pub max_active_readers: usize,
    /// Whether an exclusive holder is active.
    pub writer_active: bool,
    /// Number of exclusive callers waiting for readers/writer to leave.
    pub waiting_writers: usize,
}

/// Snapshot of lwext4's C-side stage-three lock domains.
#[derive(Debug, Clone, Copy, Default)]
pub struct Lwext4Stage3LockStats {
    /// Filesystem transaction lock acquisitions.
    pub journal_acquisitions: u64,
    /// Filesystem transaction acquisitions that had to wait.
    pub journal_contentions: u64,
    /// Owner-context map acquisitions.
    pub transaction_context_acquisitions: u64,
    /// Owner-context map acquisitions that had to wait.
    pub transaction_context_contentions: u64,
    /// Shared inode-reference acquisitions.
    pub inode_read_acquisitions: u64,
    /// Exclusive inode-reference acquisitions.
    pub inode_write_acquisitions: u64,
    /// Inode shard acquisitions that had to wait.
    pub inode_contentions: u64,
    /// Block-group shard acquisitions.
    pub block_group_acquisitions: u64,
    /// Block-group shard acquisitions that had to wait.
    pub block_group_contentions: u64,
    /// Packed superblock counter lock acquisitions.
    pub superblock_acquisitions: u64,
    /// Packed superblock counter acquisitions that had to wait.
    pub superblock_contentions: u64,
    /// Current owner-indexed JBD transaction contexts.
    pub active_transactions: u32,
    /// Peak number of transactions prepared concurrently.
    pub max_active_transactions: u32,
    /// Current number of inode readers.
    pub active_inode_readers: u32,
    /// Peak number of concurrent inode readers.
    pub max_active_inode_readers: u32,
    /// Current number of inode writers.
    pub active_inode_writers: u32,
    /// Peak number of concurrent inode writers.
    pub max_active_inode_writers: u32,
    /// Current number of independently active block-group shards.
    pub active_block_groups: u32,
    /// Peak number of independently active block-group shards.
    pub max_active_block_groups: u32,
}

/// Non-blocking diagnostic snapshot for one ext4 mount gate.
#[derive(Debug, Clone)]
pub struct Lwext4MountLockStats {
    /// Stable mount identifier.
    pub mount_id: usize,
    /// Normalized VFS path at which this ext4 instance is mounted.
    pub mount_point: String,
    /// Reader/writer state and queued writer count for this mount.
    pub lock: Lwext4MountRwLockStats,
    /// C-side journal/inode/block-group lock statistics. `None` means the
    /// mount lifecycle gate was busy while the non-blocking snapshot ran.
    pub stage3: Option<Lwext4Stage3LockStats>,
    /// Task identity currently holding the exclusive side, or zero.
    pub owner: usize,
    /// Process currently holding this mount gate, or zero.
    pub owner_pid: usize,
    /// Active syscall of the owner, when available.
    pub owner_syscall: Option<usize>,
    /// Recursive entry depth for the current exclusive owner.
    pub recursion: usize,
    /// Operation currently executing below the exclusive side of the gate.
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

fn lwext4_stage3_lock_stats(gate: &Lwext4MountGate) -> Option<Lwext4Stage3LockStats> {
    let _lifecycle = gate.lock.try_read()?;
    let mount_point = CString::new(gate.mount_point.as_str()).ok()?;
    let mut raw = lwext4_rust::bindings::ext4_fs_lock_stats::default();
    let rc =
        unsafe { lwext4_rust::bindings::ext4_mount_lock_stats_get(mount_point.as_ptr(), &mut raw) };
    if rc != 0 {
        return None;
    }
    Some(Lwext4Stage3LockStats {
        journal_acquisitions: raw.journal_acquisitions,
        journal_contentions: raw.journal_contentions,
        transaction_context_acquisitions: raw.transaction_context_acquisitions,
        transaction_context_contentions: raw.transaction_context_contentions,
        inode_read_acquisitions: raw.inode_read_acquisitions,
        inode_write_acquisitions: raw.inode_write_acquisitions,
        inode_contentions: raw.inode_contentions,
        block_group_acquisitions: raw.block_group_acquisitions,
        block_group_contentions: raw.block_group_contentions,
        superblock_acquisitions: raw.superblock_acquisitions,
        superblock_contentions: raw.superblock_contentions,
        active_transactions: raw.active_transactions,
        max_active_transactions: raw.max_active_transactions,
        active_inode_readers: raw.active_inode_readers,
        max_active_inode_readers: raw.max_active_inode_readers,
        active_inode_writers: raw.active_inode_writers,
        max_active_inode_writers: raw.max_active_inode_writers,
        active_block_groups: raw.active_block_groups,
        max_active_block_groups: raw.max_active_block_groups,
    })
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
                    lock: Lwext4MountRwLockStats {
                        active_readers: gate.active_readers.load(Ordering::Acquire),
                        max_active_readers: gate.max_active_readers.load(Ordering::Acquire),
                        writer_active: gate.writer_active.load(Ordering::Acquire) != 0,
                        waiting_writers: gate.waiting_writers.load(Ordering::Acquire),
                    },
                    stage3: lwext4_stage3_lock_stats(gate),
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
    // A full mount flush must freeze all data-path callers. This is distinct
    // from per-file page writeback, which uses the shared side above.
    with_lwext4_mount_write_lock_op(gate, Lwext4Op::Writeback, || {
        flush_lwext4_mount_locked(gate)
    })
}

/// Flush every registered ext4 mount and its backing block device.
pub fn flush_all_lwext4_mounts() -> Result<(), SysError> {
    with_lwext4_global_lock_op(Lwext4Op::Writeback, || {
        let gates: Vec<_> = LWEXT4_MOUNT_GATES.lock().values().cloned().collect();
        for gate in gates {
            with_lwext4_mount_write_lock_op(&gate, Lwext4Op::Writeback, || {
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

fn wait_for_lwext4_gate(in_task_context: bool) {
    if in_task_context {
        // Global lifecycle operations acquire mount gates in order, so this
        // waiter may already own an earlier gate.  It must resume this kernel
        // continuation and release every guard before honoring exec/exit.
        crate::task::suspend_current_kernel_continuation();
    } else {
        core::hint::spin_loop();
    }
}

fn acquire_lwext4_mount_read(
    gate: &Lwext4MountGate,
    owner: usize,
    in_task_context: bool,
) -> (spin::RwLockReadGuard<'_, ()>, bool) {
    let recursive_reader = gate.reader_owned_by(owner);
    let mut contended = false;
    loop {
        // Once a writer is queued, stop admitting unrelated readers. A
        // recursive reader may re-enter so it can finish its outer section.
        if !recursive_reader && gate.waiting_writers.load(Ordering::Acquire) != 0 {
            contended = true;
            wait_for_lwext4_gate(in_task_context);
            continue;
        }
        if let Some(guard) = gate.lock.try_read() {
            return (guard, contended);
        }
        contended = true;
        wait_for_lwext4_gate(in_task_context);
    }
}

fn acquire_lwext4_mount_write(
    gate: &Lwext4MountGate,
    in_task_context: bool,
) -> (Lwext4RawWriteGuard<'_>, bool) {
    gate.waiting_writers.fetch_add(1, Ordering::AcqRel);
    let mut contended = false;
    let guard = loop {
        if let Some(guard) = gate.lock.try_write() {
            break guard;
        }
        contended = true;
        wait_for_lwext4_gate(in_task_context);
    };
    gate.waiting_writers.fetch_sub(1, Ordering::AcqRel);
    gate.writer_active.store(1, Ordering::Release);
    (
        Lwext4RawWriteGuard {
            gate,
            _guard: guard,
        },
        contended,
    )
}

struct Lwext4RawWriteGuard<'a> {
    gate: &'a Lwext4MountGate,
    _guard: spin::RwLockWriteGuard<'a, ()>,
}

impl Drop for Lwext4RawWriteGuard<'_> {
    fn drop(&mut self) {
        self.gate.writer_active.store(0, Ordering::Release);
    }
}

/// Cooperative wait hook used by lwext4's short block-cache state lock and
/// same-LBA loading coordination.
#[unsafe(no_mangle)]
pub extern "C" fn ext4_bcache_yield() {
    if crate::task::processor::has_current_task_nolock() {
        // A caller can already own a journal/inode/block-group lock. Resume
        // this exact C continuation so pending exec/exit cannot abandon those
        // locks while waiting for the short bcache state lock.
        crate::task::suspend_current_kernel_continuation();
    } else {
        core::hint::spin_loop();
    }
}

/// Stable owner used by lwext4's reentrant transaction and metadata locks.
/// This is a lock-free scheduler publication rather than the hart ID: a task
/// waiting for a lower-level filesystem lock may yield while retaining an
/// outer lock, and another task can run on the same hart in the meantime.
#[unsafe(no_mangle)]
pub extern "C" fn ext4_lock_owner() -> usize {
    crate::task::processor::current_task_owner_nolock()
}

/// Mark a task continuation as owning one more lwext4 C-layer lock.
#[unsafe(no_mangle)]
pub extern "C" fn ext4_lock_critical_enter() {
    if let Some(task) = crate::task::current_task() {
        task.enter_kernel_critical_section();
    }
}

/// Release one lwext4 C-layer lock from the current task continuation.
#[unsafe(no_mangle)]
pub extern "C" fn ext4_lock_critical_exit() {
    if let Some(task) = crate::task::current_task() {
        task.leave_kernel_critical_section();
    }
}

/// Cooperative wait hook for stage-three journal/inode/block-group locks.
#[unsafe(no_mangle)]
pub extern "C" fn ext4_lock_yield() {
    ext4_bcache_yield();
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
    let (owner, _, _, in_task_context) = current_lwext4_context();
    with_lwext4_global_lock_op(operation, || {
        let gates: Vec<_> = LWEXT4_MOUNT_GATES.lock().values().cloned().collect();
        let _guards: Vec<_> = gates
            .iter()
            .filter(|gate| gate.owner.load(Ordering::Acquire) != owner)
            .map(|gate| acquire_lwext4_mount_write(gate, in_task_context).0)
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lwext4MountAccess {
    Shared,
    Exclusive,
}

#[allow(dead_code)]
enum Lwext4MountGuard<'a> {
    Shared(spin::RwLockReadGuard<'a, ()>),
    Exclusive(Lwext4RawWriteGuard<'a>),
}

fn with_lwext4_mount_access_op<R>(
    gate: &Lwext4MountGate,
    operation: Lwext4Op,
    access: Lwext4MountAccess,
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
    let (guard, contended) = match access {
        Lwext4MountAccess::Shared => {
            let (guard, contended) = acquire_lwext4_mount_read(gate, owner, in_task_context);
            gate.note_reader_acquired(owner);
            (Lwext4MountGuard::Shared(guard), contended)
        }
        Lwext4MountAccess::Exclusive => {
            let (guard, contended) = acquire_lwext4_mount_write(gate, in_task_context);
            (Lwext4MountGuard::Exclusive(guard), contended)
        }
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

    if access == Lwext4MountAccess::Exclusive {
        gate.owner.store(owner, Ordering::Release);
        gate.owner_pid.store(owner_pid, Ordering::Release);
        gate.owner_syscall
            .store(owner_syscall.unwrap_or(usize::MAX), Ordering::Release);
        gate.recursion.store(1, Ordering::Release);
        gate.current_operation
            .store(operation_index, Ordering::Release);
    }

    // Namespace operations can involve several directory/inode shards. Keep
    // them ordered per mount until rename/link use an explicit sorted prelock
    // set, while allowing file data writers on unrelated inodes to proceed.
    let namespace_recursive =
        operation == Lwext4Op::Metadata && gate.namespace_owner.load(Ordering::Acquire) == owner;
    let namespace_guard = if operation == Lwext4Op::Metadata && !namespace_recursive {
        let guard = gate.namespace_lock.lock();
        gate.namespace_owner.store(owner, Ordering::Release);
        gate.namespace_recursion.store(1, Ordering::Release);
        Some(guard)
    } else {
        if namespace_recursive {
            gate.namespace_recursion.fetch_add(1, Ordering::Relaxed);
        }
        None
    };
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

    if namespace_recursive {
        gate.namespace_recursion.fetch_sub(1, Ordering::Release);
    } else if namespace_guard.is_some() {
        gate.namespace_recursion.store(0, Ordering::Release);
        gate.namespace_owner.store(0, Ordering::Release);
    }
    drop(namespace_guard);

    if access == Lwext4MountAccess::Shared {
        gate.note_reader_released(owner);
    } else {
        gate.current_operation.store(usize::MAX, Ordering::Release);
        gate.recursion.store(0, Ordering::Release);
        gate.owner_syscall.store(usize::MAX, Ordering::Release);
        gate.owner_pid.store(0, Ordering::Release);
        gate.owner.store(0, Ordering::Release);
    }
    drop(guard);
    ret
}

/// Run one operation under the gate belonging to a single ext4 mount.
pub fn with_lwext4_mount_lock_op<R>(
    gate: &Lwext4MountGate,
    operation: Lwext4Op,
    f: impl FnOnce() -> R,
) -> R {
    let access = if operation.uses_shared_mount_gate() {
        Lwext4MountAccess::Shared
    } else {
        Lwext4MountAccess::Exclusive
    };
    with_lwext4_mount_access_op(gate, operation, access, f)
}

/// Run an explicitly read-only operation under a shared mount gate.
pub fn with_lwext4_mount_read_lock_op<R>(
    gate: &Lwext4MountGate,
    operation: Lwext4Op,
    f: impl FnOnce() -> R,
) -> R {
    with_lwext4_mount_access_op(gate, operation, Lwext4MountAccess::Shared, f)
}

/// Run a mutating operation under an exclusive mount gate.
pub fn with_lwext4_mount_write_lock_op<R>(
    gate: &Lwext4MountGate,
    operation: Lwext4Op,
    f: impl FnOnce() -> R,
) -> R {
    with_lwext4_mount_access_op(gate, operation, Lwext4MountAccess::Exclusive, f)
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
    let normalized = err.abs();
    let mapped = SysError::try_from(normalized).unwrap_or(SysError::EIO);
    if mapped == SysError::EIO {
        error!(
            "[LWEXT4_EIO] raw_error={} normalized_error={} ext4_flush={:?} block_io={:?}",
            err,
            normalized,
            crate::fs::lwext4::file::ext4_flush_stats(),
            crate::drivers::block::virtio_blk::virtio_block_io_stats(),
        );
    }
    mapped
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
