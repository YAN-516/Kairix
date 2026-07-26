use super::TaskControlBlock;
use super::add_task;
use super::id::{RecycleAllocator, kstack_alloc};
use super::manager::*;
use super::task_entry;
use super::{PidHandle, TaskStatus, alloc_pid_raw, dealloc_pid, pid_alloc};
// use crate::config::PAGE_SIZE;
use crate::error::SysError;
use crate::fs::File;
use crate::fs::devfs::urandom::fill_random;
use crate::sync::{BlockingMutexGuard, SleepLock, SpinNoIrq, SpinNoIrqLock};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use core::{mem::ManuallyDrop, ops::Deref, ops::DerefMut};

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub(crate) struct ForkCloneStats {
    pub active: bool,
    pub generation: usize,
    pub parent_pid: usize,
    pub owner_cpu: usize,
    pub phase: usize,
}

static FORK_CLONE_ACTIVE: AtomicBool = AtomicBool::new(false);
static FORK_CLONE_GENERATION: AtomicUsize = AtomicUsize::new(0);
static FORK_CLONE_PARENT_PID: AtomicUsize = AtomicUsize::new(0);
static FORK_CLONE_OWNER_CPU: AtomicUsize = AtomicUsize::new(usize::MAX);
static FORK_CLONE_PHASE: AtomicUsize = AtomicUsize::new(0);

struct ForkCloneTraceGuard {
    tracked: bool,
}

impl ForkCloneTraceGuard {
    fn begin(parent_pid: usize) -> Self {
        if FORK_CLONE_ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Self { tracked: false };
        }
        FORK_CLONE_GENERATION.fetch_add(1, Ordering::AcqRel);
        FORK_CLONE_PARENT_PID.store(parent_pid, Ordering::Relaxed);
        FORK_CLONE_OWNER_CPU.store(polyhal::arch::hart_id(), Ordering::Relaxed);
        FORK_CLONE_PHASE.store(1, Ordering::Release);
        Self { tracked: true }
    }

    fn phase(&self, phase: usize) {
        if self.tracked {
            FORK_CLONE_PHASE.store(phase, Ordering::Release);
        }
    }
}

impl Drop for ForkCloneTraceGuard {
    fn drop(&mut self) {
        if self.tracked {
            FORK_CLONE_ACTIVE.store(false, Ordering::Release);
        }
    }
}

pub(crate) fn fork_clone_stats() -> ForkCloneStats {
    ForkCloneStats {
        active: FORK_CLONE_ACTIVE.load(Ordering::Acquire),
        generation: FORK_CLONE_GENERATION.load(Ordering::Relaxed),
        parent_pid: FORK_CLONE_PARENT_PID.load(Ordering::Relaxed),
        owner_cpu: FORK_CLONE_OWNER_CPU.load(Ordering::Relaxed),
        phase: FORK_CLONE_PHASE.load(Ordering::Acquire),
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Rlimit64 {
    pub rlim_cur: u64,
    pub rlim_max: u64,
}

pub const RLIMIT_FSIZE: i32 = 1;
pub const RLIMIT_NOFILE: i32 = 7;
pub const RLIM_INFINITY: u64 = u64::MAX;
use crate::fs::devfs::tty::TtyFile;
use crate::fs::vfs::Dentry;
use crate::fs::vfs::dcache::GLOBAL_DCACHE;
use crate::fs::vfs::file::find_dentry;
use crate::mm::PageTable;
use crate::mm::UserMapArea;
use crate::mm::VMSpace;
use crate::mm::exception::SetPageFaultException;
use crate::mm::frame_alloc;
use crate::mm::frame_allocator;
use crate::mm::vm_set::{self, AccessType, PageFaultError};
use crate::mm::{MapPermission, MapType, VirtAddr};
use crate::mm::{UserVMSet, translated_byte_buffer_for_write};
use crate::security::landlock::LandlockDomain;
use crate::signal::*;
use crate::socket::*;
use crate::syscall::shm::{fork_inherit_shm_attach, release_shm_attaches};
use crate::task::id::PgidHandle;
// use crate::timer::get_time;
use crate::mm::UserMapAreaType;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec;
use alloc::vec::Vec;
use lazy_static::lazy_static;

use polyhal::MappingFlags;
use polyhal::MappingSize;
use polyhal::consts::*;
use polyhal::pagetable;
use polyhal::pagetable::PTEFlags;
use polyhal::timer::current_time;
use polyhal::utils::addr::VirtPageNum;
#[cfg(target_arch = "riscv64")]
use riscv::register::mcause::Trap;

use core::arch::asm;
use core::cell::RefMut;
use core::error;
use core::mem;
use log::error;
use log::info;
use log::trace;
use log::warn;
use polyhal::kcontext::*;
use polyhal_trap::trap::*;
use polyhal_trap::trapframe::*;
use spin::MutexGuard;

static PROCESS_CREATE_COUNT: AtomicUsize = AtomicUsize::new(0);
static PROCESS_DROP_COUNT: AtomicUsize = AtomicUsize::new(0);
const PROCESS_REGISTRY_PRUNE_INTERVAL: usize = 256;
const PROCESS_REGISTRY_PRUNE_THRESHOLD: usize = 512;

lazy_static! {
    static ref PROCESS_REGISTRY: SpinNoIrqLock<Vec<Weak<ProcessControlBlock>>> =
        SpinNoIrqLock::new(Vec::new());
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ProcessRegistryStats {
    pub created: usize,
    pub dropped: usize,
    pub live_delta: usize,
    pub registry_entries: usize,
    pub registry_live: usize,
    pub registry_dead: usize,
    pub hidden_processes: usize,
    pub hidden_zombies: usize,
    pub hidden_task_slots: usize,
    pub hidden_open_files: usize,
    pub hidden_child_refs: usize,
    pub hidden_locked: usize,
    pub max_hidden_strong_count: usize,
    pub max_hidden_strong_count_pid: usize,
    pub lock_busy: bool,
    pub pid_table_lock_busy: bool,
}

fn register_process(process: &Arc<ProcessControlBlock>) {
    let created = PROCESS_CREATE_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    let mut registry = PROCESS_REGISTRY.lock();
    if created % PROCESS_REGISTRY_PRUNE_INTERVAL == 0
        || registry.len() >= PROCESS_REGISTRY_PRUNE_THRESHOLD
    {
        registry.retain(|weak| weak.strong_count() != 0);
    }
    registry.push(Arc::downgrade(process));
}

fn enqueue_new_clone_task(task: Arc<TaskControlBlock>) {
    let pid = task.process_id();
    log::error!(
        "[FORK_CLONE_ENQUEUE_ENTER] cpu={} pid={} ready_queued={} on_cpu={}",
        polyhal::arch::hart_id(),
        pid,
        task.is_ready_queued(),
        task.is_on_cpu(),
    );
    add_task(task);
    log::error!(
        "[FORK_CLONE_ENQUEUE_DONE] cpu={} pid={}",
        polyhal::arch::hart_id(),
        pid,
    );
}

pub(crate) fn process_registry_stats() -> ProcessRegistryStats {
    let created = PROCESS_CREATE_COUNT.load(Ordering::Relaxed);
    let dropped = PROCESS_DROP_COUNT.load(Ordering::Relaxed);
    let Some(registry) = PROCESS_REGISTRY.try_lock() else {
        return ProcessRegistryStats {
            created,
            dropped,
            live_delta: created.saturating_sub(dropped),
            registry_entries: 0,
            registry_live: 0,
            registry_dead: 0,
            hidden_processes: 0,
            hidden_zombies: 0,
            hidden_task_slots: 0,
            hidden_open_files: 0,
            hidden_child_refs: 0,
            hidden_locked: 0,
            max_hidden_strong_count: 0,
            max_hidden_strong_count_pid: 0,
            lock_busy: true,
            pid_table_lock_busy: false,
        };
    };
    let mut registry_entries = registry.len();
    drop(registry);
    let mut registry_snapshot = Vec::with_capacity(registry_entries);
    loop {
        let Some(registry) = PROCESS_REGISTRY.try_lock() else {
            return ProcessRegistryStats {
                created,
                dropped,
                live_delta: created.saturating_sub(dropped),
                registry_entries,
                registry_live: 0,
                registry_dead: 0,
                hidden_processes: 0,
                hidden_zombies: 0,
                hidden_task_slots: 0,
                hidden_open_files: 0,
                hidden_child_refs: 0,
                hidden_locked: 0,
                max_hidden_strong_count: 0,
                max_hidden_strong_count_pid: 0,
                lock_busy: true,
                pid_table_lock_busy: false,
            };
        };
        let required = registry.len();
        if required > registry_snapshot.capacity() {
            drop(registry);
            registry_snapshot.reserve(required);
            continue;
        }
        registry_entries = required;
        for process in registry.iter() {
            registry_snapshot.push(process.clone());
        }
        drop(registry);
        break;
    }
    let Some(pid_table) = crate::task::manager::try_all_processes() else {
        return ProcessRegistryStats {
            created,
            dropped,
            live_delta: created.saturating_sub(dropped),
            registry_entries,
            registry_live: 0,
            registry_dead: 0,
            hidden_processes: 0,
            hidden_zombies: 0,
            hidden_task_slots: 0,
            hidden_open_files: 0,
            hidden_child_refs: 0,
            hidden_locked: 0,
            max_hidden_strong_count: 0,
            max_hidden_strong_count_pid: 0,
            lock_busy: false,
            pid_table_lock_busy: true,
        };
    };

    let mut stats = ProcessRegistryStats {
        created,
        dropped,
        live_delta: created.saturating_sub(dropped),
        registry_entries,
        registry_live: 0,
        registry_dead: 0,
        hidden_processes: 0,
        hidden_zombies: 0,
        hidden_task_slots: 0,
        hidden_open_files: 0,
        hidden_child_refs: 0,
        hidden_locked: 0,
        max_hidden_strong_count: 0,
        max_hidden_strong_count_pid: 0,
        lock_busy: false,
        pid_table_lock_busy: false,
    };

    for weak in &registry_snapshot {
        let Some(process) = weak.upgrade() else {
            stats.registry_dead += 1;
            continue;
        };
        stats.registry_live += 1;
        let pid = process.getpid();
        if pid_table.iter().any(|process| process.getpid() == pid) {
            continue;
        }
        stats.hidden_processes += 1;
        let strong_count = Arc::strong_count(&process);
        if strong_count > stats.max_hidden_strong_count {
            stats.max_hidden_strong_count = strong_count;
            stats.max_hidden_strong_count_pid = pid;
        }
        let Some(inner) = process.inner_try_access() else {
            stats.hidden_locked += 1;
            continue;
        };
        if inner.is_zombie {
            stats.hidden_zombies += 1;
        }
        stats.hidden_task_slots += inner.tasks.iter().flatten().count();
        stats.hidden_open_files += inner.fd_table.iter().filter(|file| file.is_some()).count();
        stats.hidden_child_refs += inner.children.len();
    }

    stats
}

#[allow(unused)]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Tms {
    pub tms_utime: usize,
    pub tms_stime: usize,
    pub tms_cutime: usize,
    pub tms_cstime: usize,
}
#[allow(unused)]
impl Tms {
    pub fn new() -> Self {
        Self {
            tms_utime: 0,
            tms_stime: 0,
            tms_cutime: 0,
            tms_cstime: 0,
        }
    }
}

pub enum ProcessStatus {
    Ready,
    Running,
    Blocked,
    Terminal,
}

/// 进程终止状态，用于 waitpid 正确格式化 status 字
#[derive(Clone, Copy, Debug)]
pub enum TermStatus {
    Running,
    Exited(i32),
    Signaled(i32, bool), // 信号编号, 是否产生 core dump
    Stopped(i32),        // 停止信号编号
}

pub struct ProcessControlBlock {
    // immutable
    pub pid: PidHandle,
    user_token: AtomicUsize,
    /// Linux PR_GET/SET_DUMPABLE state.
    dumpable: AtomicBool,
    inner_owner_cpu: AtomicUsize,
    inner_owner_line: AtomicUsize,
    /// The final live thread has published process exit but has not yet
    /// switched off its kernel stack and become safe for the parent to reap.
    /// Deferred payloads retain resources whose destruction may finish later.
    final_exit_cleanup_pending: AtomicBool,
    /// Monotonic sequence for child wait predicates (exit/stop/continue).
    /// Waiters compare this while publishing Blocked under their task lock so
    /// a child event cannot be lost between the final scan and schedule().
    child_event_seq: AtomicUsize,
    /// The mm object is independently reference counted so non-thread
    /// `CLONE_VM` children can share subsequent VMA/PTE changes.  The short
    /// outer lock protects replacement during exec, which must unshare the mm.
    vm_set: SpinNoIrqLock<Arc<SleepLock<UserVMSet>>>,
    /// Current files_struct handle. Keeping this outside `inner` lets every
    /// access acquire the shared files lock before the per-process PCB lock,
    /// avoiding ABBA deadlocks between CLONE_FILES peer processes.
    files_handle: SpinNoIrqLock<Arc<SharedFiles>>,
    // mutable
    inner: SpinNoIrqLock<ProcessControlBlockInner>,
}

/// An owning mm guard. The Arc keeps the selected address-space object alive
/// even if another thread replaces the process's mm during exec.
pub struct ProcessVmGuard {
    guard: ManuallyDrop<BlockingMutexGuard<'static, UserVMSet, SpinNoIrq>>,
    _handle: Arc<SleepLock<UserVMSet>>,
}

impl Deref for ProcessVmGuard {
    type Target = UserVMSet;

    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl DerefMut for ProcessVmGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

impl Drop for ProcessVmGuard {
    fn drop(&mut self) {
        // The mutex guard borrows the allocation owned by `handle`, so release
        // the borrow before allowing the Arc field to drop.
        unsafe { ManuallyDrop::drop(&mut self.guard) };
    }
}

/// Holds both the per-process state lock and the potentially shared
/// CLONE_FILES lock. This preserves the existing `inner.fd_table` API while
/// serializing accesses made through different process control blocks.
pub struct ProcessInnerGuard<'a> {
    inner: ManuallyDrop<crate::sync::SpinMutexGuard<'a, ProcessControlBlockInner, SpinNoIrq>>,
    _files_handle: Arc<SharedFiles>,
}

impl Deref for ProcessInnerGuard<'_> {
    type Target = ProcessControlBlockInner;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for ProcessInnerGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl Drop for ProcessInnerGuard<'_> {
    fn drop(&mut self) {
        unsafe { ManuallyDrop::drop(&mut self.inner) };
        let previous = self
            ._files_handle
            .borrow_depth
            .fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0);
        if previous == 1 {
            self._files_handle.borrow_owner.store(0, Ordering::Release);
            let held_gate = unsafe { &mut *self._files_handle.held_gate.get() };
            let mut gate = held_gate
                .take()
                .expect("shared files gate missing at final borrow release");
            unsafe { ManuallyDrop::drop(&mut gate) };
        }
    }
}

impl Drop for ProcessControlBlock {
    fn drop(&mut self) {
        PROCESS_DROP_COUNT.fetch_add(1, Ordering::Relaxed);
    }
}

pub struct ProcessControlBlockInner {
    pub is_zombie: bool,
    pub is_stopped: bool,
    pub was_continued: bool,
    pub zombie_flag: AtomicBool,
    pub pgid: PgidHandle,
    /// 真实 UID
    pub uid: u32,
    /// 有效 UID
    pub euid: u32,
    /// 保存的 set-user-ID
    pub suid: u32,
    /// 真实 GID
    pub gid: u32,
    /// 有效 GID
    pub egid: u32,
    /// 保存的 set-group-ID
    pub sgid: u32,
    pub parent: Option<Weak<ProcessControlBlock>>,
    pub children: Vec<Arc<ProcessControlBlock>>,
    pub exit_code: i32,
    pub term_status: TermStatus,
    pub fd_table: SharedFdTable,
    pub fd_flags: SharedFdFlags,
    pub tasks: Vec<Option<Arc<TaskControlBlock>>>,
    pub task_res_allocator: RecycleAllocator,
    pub fs_context: Arc<SpinNoIrqLock<FsContext>>,
    /// Resolved path of the ELF installed by the most recent successful execve.
    pub executable_path: String,
    pub time: Tms,
    pub ustart: usize,
    pub kstart: usize,
    pub state: ProcessStatus,

    pub pending_signals: SignalSet,
    pub pending_signal_queue: alloc::collections::VecDeque<crate::task::signal::SigInfo>,
    pub blocked_signals: SignalSet,
    pub signals_handler: Arc<SpinNoIrqLock<SignalHandlers>>,
    pub need_signal_handle: bool,
    pub itimer_real_deadline: Option<usize>,
    pub itimer_real_interval: Option<usize>,
    pub wait_waker: Option<core::task::Waker>,
    /// 信号处理上下文栈（保存在 PCB 中，单线程场景下安全）
    pub sig_context_stack: Vec<(TrapFrame, SignalSet)>,
    /// ITIMER_REAL 的到期时间（微秒），None 表示未设置
    pub alarm_deadline_us: Option<u128>,
    /// ITIMER_REAL 的间隔时间（微秒），None 表示单次定时器
    pub alarm_interval_us: Option<u128>,
    /// 资源限制：文件大小上限
    pub rlimit_fsize: Rlimit64,
    /// 资源限制：单文件描述符最大数量
    pub rlimit_nofile: Rlimit64,
    /// prctl(PR_SET_NO_NEW_PRIVS) state.
    pub no_new_privs: bool,
    /// Minimal capability tracking used by Landlock tests.
    pub has_cap_sys_admin: bool,
    /// Landlock security domain.
    pub landlock: LandlockDomain,
    /// 还活着的线程数量（用于 waitpid 判断是否可以回收进程）
    pub alive_thread_count: usize,
    /// Prevent repeated exit cleanup from releasing this process's mm/shm
    /// attachment accounting more than once.
    pub user_space_released: bool,
    /// Prevent duplicate files_struct owner release during concurrent teardown.
    pub files_released: bool,
    /// Global TID of the thread currently committing an execve.
    pub exec_owner_tid: Option<usize>,
    /// CLONE_VFORK 时记录需要唤醒的父任务
    pub vfork_parent: Option<Arc<TaskControlBlock>>,
    /// 网络命名空间 ID（0 表示初始命名空间）
    pub net_ns_id: usize,
    /// 进程退出时是否关闭过 socket。父进程 wait 返回前可据此给网络后台任务收尾机会。
    pub needs_post_wait_network_quiesce: bool,
    /// 子进程退出时发送给父进程的信号（clone/clone3 的 exit_signal）
    pub exit_signal: i32,
    /// 最近一次投递信号时携带的 siginfo（用于 pidfd_send_signal 等）
    pub last_siginfo: Option<crate::task::signal::SigInfo>,
}

#[derive(Clone)]
pub struct FsContext {
    pub cwd: Arc<dyn Dentry>,
    pub umask: u32,
}

struct SharedFiles {
    gate: SpinNoIrqLock<()>,
    borrow_depth: AtomicUsize,
    borrow_owner: AtomicUsize,
    held_gate:
        UnsafeCell<Option<ManuallyDrop<crate::sync::SpinMutexGuard<'static, (), SpinNoIrq>>>>,
    owners: AtomicUsize,
    data: UnsafeCell<FilesContext>,
}

unsafe impl Send for SharedFiles {}
unsafe impl Sync for SharedFiles {}

#[derive(Clone)]
pub struct FilesContext {
    pub fd_table: Vec<Option<Arc<dyn File + Send + Sync>>>,
    pub fd_flags: Vec<u32>,
}

impl SharedFiles {
    fn new(context: FilesContext) -> Arc<Self> {
        Arc::new(Self {
            gate: SpinNoIrqLock::new(()),
            borrow_depth: AtomicUsize::new(0),
            borrow_owner: AtomicUsize::new(0),
            held_gate: UnsafeCell::new(None),
            owners: AtomicUsize::new(1),
            data: UnsafeCell::new(context),
        })
    }
}

pub struct SharedFdTable(Arc<SharedFiles>);
pub struct SharedFdFlags(Arc<SharedFiles>);

impl Deref for SharedFdTable {
    type Target = Vec<Option<Arc<dyn File + Send + Sync>>>;

    fn deref(&self) -> &Self::Target {
        // ProcessInnerGuard holds this SharedFiles::gate for every public
        // access to ProcessControlBlockInner.
        unsafe { &(*self.0.data.get()).fd_table }
    }
}

impl DerefMut for SharedFdTable {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut (*self.0.data.get()).fd_table }
    }
}

impl Deref for SharedFdFlags {
    type Target = Vec<u32>;

    fn deref(&self) -> &Self::Target {
        unsafe { &(*self.0.data.get()).fd_flags }
    }
}

impl DerefMut for SharedFdFlags {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut (*self.0.data.get()).fd_flags }
    }
}

impl ProcessControlBlockInner {
    pub fn is_zombie(&self) -> bool {
        self.is_zombie
    }

    pub fn handle_default_action(&mut self, signal: Signal) {
        match signal.default_action() {
            SignalAction::Ignore => {}
            SignalAction::Stop => {
                self.state = ProcessStatus::Terminal;
                self.is_stopped = true;
            }
            SignalAction::Continue => {
                self.state = ProcessStatus::Ready;
                self.is_stopped = false;
            }
            SignalAction::Terminate | SignalAction::Core => {
                self.is_zombie = true;
                self.zombie_flag.store(true, Ordering::SeqCst);
            }
        }
    }

    pub fn alloc_fd(&mut self) -> Result<usize, SysError> {
        let max_fd = self.rlimit_nofile.rlim_cur as usize;
        // 在允许范围内寻找最小的空闲 fd（0..max_fd）
        if let Some(fd) =
            (0..max_fd.min(self.fd_table.len())).find(|fd| self.fd_table[*fd].is_none())
        {
            return Ok(fd);
        }
        // 允许范围内没有空闲 slot，尝试扩展（前提是当前长度 < max_fd）
        if self.fd_table.len() < max_fd {
            self.fd_table.push(None);
            self.fd_flags.push(0);
            Ok(self.fd_table.len() - 1)
        } else {
            Err(SysError::EMFILE)
        }
    }

    pub fn alloc_tid(&mut self) -> usize {
        self.task_res_allocator.alloc()
    }

    pub fn dealloc_tid(&mut self, tid: usize) {
        self.task_res_allocator.dealloc(tid)
    }

    pub fn thread_count(&self) -> usize {
        self.tasks.iter().flatten().count()
    }

    pub fn get_task(&self, tid: usize) -> Arc<TaskControlBlock> {
        self.tasks[tid].as_ref().unwrap().clone()
    }
}

impl ProcessControlBlock {
    pub(crate) fn begin_final_exit_cleanup(&self) {
        let already_pending = self.final_exit_cleanup_pending.swap(true, Ordering::AcqRel);
        assert!(!already_pending, "final process-exit cleanup started twice");
    }

    pub(crate) fn finish_final_exit_cleanup(&self) {
        let was_pending = self
            .final_exit_cleanup_pending
            .swap(false, Ordering::AcqRel);
        assert!(
            was_pending,
            "final process-exit cleanup finished without start"
        );
    }

    pub(crate) fn final_exit_cleanup_pending(&self) -> bool {
        self.final_exit_cleanup_pending.load(Ordering::Acquire)
    }

    pub(crate) fn child_event_sequence(&self) -> usize {
        self.child_event_seq.load(Ordering::Acquire)
    }

    pub(crate) fn publish_child_event(&self) -> usize {
        self.child_event_seq.fetch_add(1, Ordering::AcqRel) + 1
    }

    pub fn dumpable(&self) -> bool {
        self.dumpable.load(Ordering::Acquire)
    }

    pub fn set_dumpable(&self, dumpable: bool) {
        self.dumpable.store(dumpable, Ordering::Release);
    }

    #[allow(missing_docs)]
    pub fn user_token(&self) -> usize {
        self.user_token.load(Ordering::Acquire)
    }

    #[allow(missing_docs)]
    pub fn activate_user_page_table(&self) -> bool {
        let token = self.user_token();
        let unchanged = PageTable::from_token(token).change();
        crate::mm::vm_set::record_active_page_table_token(token);
        unchanged
    }

    fn set_user_token(&self, token: usize) {
        self.user_token.store(token, Ordering::Release);
    }

    /// Clone the current mm handle. Callers then take its sleeping lock without
    /// retaining the short pointer lock across page faults or filesystem I/O.
    pub fn vm_handle(&self) -> Arc<SleepLock<UserVMSet>> {
        self.vm_set.lock().clone()
    }

    /// Access the address space selected at acquisition time. The owning guard
    /// remains valid across a concurrent exec pointer replacement.
    #[track_caller]
    pub fn vm_exclusive_access(&self) -> ProcessVmGuard {
        let handle = self.vm_handle();
        let guard = handle.lock();
        // `handle` is stored in ProcessVmGuard and outlives `guard`; extending
        // the borrow lifetime is therefore valid until ProcessVmGuard::drop.
        let guard = unsafe {
            core::mem::transmute::<
                BlockingMutexGuard<'_, UserVMSet, SpinNoIrq>,
                BlockingMutexGuard<'static, UserVMSet, SpinNoIrq>,
            >(guard)
        };
        ProcessVmGuard {
            guard: ManuallyDrop::new(guard),
            _handle: handle,
        }
    }

    #[track_caller]
    pub fn try_vm_exclusive_access(&self) -> Option<ProcessVmGuard> {
        let handle = self.vm_handle();
        let guard = handle.try_lock()?;
        let guard = unsafe {
            core::mem::transmute::<
                BlockingMutexGuard<'_, UserVMSet, SpinNoIrq>,
                BlockingMutexGuard<'static, UserVMSet, SpinNoIrq>,
            >(guard)
        };
        Some(ProcessVmGuard {
            guard: ManuallyDrop::new(guard),
            _handle: handle,
        })
    }

    /// Install a private mm during exec and return the previously referenced
    /// object. Other processes created with `CLONE_VM` retain the old object.
    fn replace_vm_handle(
        &self,
        replacement: Arc<SleepLock<UserVMSet>>,
    ) -> Arc<SleepLock<UserVMSet>> {
        core::mem::replace(&mut *self.vm_set.lock(), replacement)
    }

    fn current_files_borrow_owner() -> usize {
        crate::task::processor::current_task_owner_nolock()
    }

    fn release_files_borrow(files_handle: &Arc<SharedFiles>) {
        let previous = files_handle.borrow_depth.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0);
        if previous == 1 {
            files_handle.borrow_owner.store(0, Ordering::Release);
            let held_gate = unsafe { &mut *files_handle.held_gate.get() };
            let mut gate = held_gate
                .take()
                .expect("shared files gate missing at final borrow release");
            unsafe { ManuallyDrop::drop(&mut gate) };
        }
    }

    fn try_acquire_files(&self) -> Option<Arc<SharedFiles>> {
        let files_handle = self.files_handle.lock().clone();
        let owner = Self::current_files_borrow_owner();
        if files_handle.gate.owner_hart() == polyhal::arch::hart_id() {
            // A cooperative context switch can run a different task on the
            // same hart. Hart identity alone is therefore not a valid
            // reentrancy key for an UnsafeCell-backed files_struct.
            if files_handle.borrow_owner.load(Ordering::Acquire) != owner {
                return None;
            }
            files_handle.borrow_depth.fetch_add(1, Ordering::Relaxed);
            return Some(files_handle);
        }
        let files = files_handle.gate.try_lock()?;
        files_handle.borrow_owner.store(owner, Ordering::Relaxed);
        files_handle.borrow_depth.store(1, Ordering::Release);
        let files = unsafe {
            core::mem::transmute::<
                crate::sync::SpinMutexGuard<'_, (), SpinNoIrq>,
                crate::sync::SpinMutexGuard<'static, (), SpinNoIrq>,
            >(files)
        };
        let held_gate = unsafe { &mut *files_handle.held_gate.get() };
        debug_assert!(held_gate.is_none());
        *held_gate = Some(ManuallyDrop::new(files));
        Some(files_handle)
    }

    fn try_build_inner_guard(&self) -> Option<ProcessInnerGuard<'_>> {
        // Global order is files_struct -> PCB. On contention we release the
        // first lock before retrying, so two CLONE_FILES peers cannot form
        // files(A)->pcb(B) / pcb(B)->files(A) cycles.
        let files_handle = self.try_acquire_files()?;
        let Some(inner) = self.inner.try_lock() else {
            Self::release_files_borrow(&files_handle);
            return None;
        };
        if !Arc::ptr_eq(&inner.fd_table.0, &files_handle) {
            drop(inner);
            Self::release_files_borrow(&files_handle);
            return None;
        }
        Some(ProcessInnerGuard {
            inner: ManuallyDrop::new(inner),
            _files_handle: files_handle,
        })
    }

    #[track_caller]
    pub fn try_inner_exclusive_access(&self) -> Option<ProcessInnerGuard<'_>> {
        let caller = core::panic::Location::caller();
        let guard = self.try_build_inner_guard()?;
        self.note_inner_owner(caller.line() as usize);
        Some(guard)
    }

    #[track_caller]
    pub fn inner_exclusive_access(&self) -> ProcessInnerGuard<'_> {
        let caller = core::panic::Location::caller();
        loop {
            if let Some(guard) = self.try_build_inner_guard() {
                self.note_inner_owner(caller.line() as usize);
                return guard;
            }
            polyhal::multicore::mark_current_cpu_kernel_entry();
            core::hint::spin_loop();
        }
    }

    /// Acquire the PCB from a cleanup context that cannot schedule because its
    /// current task has already been removed. Polling kernel entry while the
    /// lock is busy closes the dependency on a PCB owner waiting for this CPU's
    /// synchronous TLB-shootdown acknowledgement.
    #[track_caller]
    pub fn inner_exclusive_access_with_tlb_progress(&self) -> ProcessInnerGuard<'_> {
        let caller = core::panic::Location::caller();
        loop {
            polyhal::multicore::mark_current_cpu_kernel_entry();
            if let Some(guard) = self.try_build_inner_guard() {
                self.note_inner_owner(caller.line() as usize);
                return guard;
            }
            if self.inner.owner_hart() == polyhal::arch::hart_id() {
                // Preserve the ordinary spinlock's recursive-acquisition
                // diagnosis; TLB progress only resolves cross-CPU waits.
                return self.inner_exclusive_access();
            }
            core::hint::spin_loop();
        }
    }

    #[track_caller]
    pub fn inner_try_access(&self) -> Option<ProcessInnerGuard<'_>> {
        let caller = core::panic::Location::caller();
        let guard = self.try_build_inner_guard()?;
        self.note_inner_owner(caller.line() as usize);
        Some(guard)
    }

    fn note_inner_owner(&self, line: usize) {
        self.inner_owner_cpu
            .store(polyhal::arch::hart_id(), Ordering::Release);
        self.inner_owner_line.store(line, Ordering::Release);
    }

    pub(crate) fn inner_owner_site(&self) -> (usize, usize) {
        (
            self.inner_owner_cpu.load(Ordering::Acquire),
            self.inner_owner_line.load(Ordering::Acquire),
        )
    }

    /// Report whether the PCB lock is currently held without acquiring it.
    /// Diagnostic code must not become a PCB owner merely to describe lock
    /// contention, especially from an idle CPU that can be host-descheduled.
    pub(crate) fn inner_is_locked(&self) -> bool {
        self.inner.is_locked()
    }

    pub fn close_all_files_on_exit(&self) {
        self.close_all_files_on_exit_inner(false);
    }

    pub(crate) fn close_all_files_on_exit_with_tlb_progress(&self) {
        self.close_all_files_on_exit_inner(true);
    }

    fn close_all_files_on_exit_inner(&self, tlb_progress: bool) {
        let pid = self.getpid();
        crate::syscall::release_process_record_locks(pid);
        let (socket_fds, retired_files) = {
            let mut inner = if tlb_progress {
                self.inner_exclusive_access_with_tlb_progress()
            } else {
                self.inner_exclusive_access()
            };
            if inner.files_released {
                return;
            }
            inner.files_released = true;
            let shared = inner.fd_table.0.clone();
            let last_owner = shared.owners.fetch_sub(1, Ordering::AcqRel) == 1;
            let socket_fds = inner
                .fd_table
                .iter()
                .enumerate()
                .filter_map(|(fd, file)| file.as_ref().filter(|file| file.is_socket()).map(|_| fd))
                .collect::<Vec<_>>();
            if !socket_fds.is_empty() {
                inner.needs_post_wait_network_quiesce = true;
            }
            let retired_files = if last_owner {
                let files = inner
                    .fd_table
                    .drain(..)
                    .enumerate()
                    .filter_map(|(fd, file)| file.map(|file| (fd, file)))
                    .collect::<Vec<_>>();
                inner.fd_flags.clear();
                files
            } else {
                Vec::new()
            };
            (socket_fds, retired_files)
        };
        crate::syscall::remove_fs_contexts_for_pid(pid);

        {
            let mut socket_manager = SOCKET_MANAGER.lock();
            for fd in socket_fds {
                let _ = socket_manager.close_socket_with_refcount(fd, pid);
            }
        }
        for (_, file) in retired_files {
            crate::syscall::release_file_description_flock_if_unreferenced(&file);
            crate::fs::writeback::queue_file(file);
        }
    }

    pub fn release_user_space_on_exit(&self) {
        self.release_user_space_on_exit_inner(false);
    }

    pub(crate) fn release_user_space_on_exit_with_tlb_progress(&self) {
        self.release_user_space_on_exit_inner(true);
    }

    fn release_user_space_on_exit_inner(&self, tlb_progress: bool) {
        let pid = self.getpid();
        {
            let mut inner = if tlb_progress {
                self.inner_exclusive_access_with_tlb_progress()
            } else {
                self.inner_exclusive_access()
            };
            if inner.user_space_released {
                return;
            }
            inner.user_space_released = true;
        }
        let vm_handle = self.vm_handle();
        let mut vm_set = if tlb_progress {
            loop {
                polyhal::multicore::mark_current_cpu_kernel_entry();
                if let Some(vm_set) = vm_handle.try_lock() {
                    break vm_set;
                }
                core::hint::spin_loop();
            }
        } else {
            vm_handle.lock()
        };
        vm_set.process_owners = vm_set
            .process_owners
            .checked_sub(1)
            .expect("mm process owner underflow during exit");
        if vm_set.process_owners != 0 {
            // A CLONE_VM peer still owns this mm. Only this process's SysV-shm
            // attachment accounting ends here; the shared VMA/PTE object must
            // remain intact for the peer.
            release_shm_attaches(&vm_set.areas);
            return;
        }
        let (old_areas, page_table_pages) = vm_set.release_user_space();
        drop(vm_set);
        if old_areas.is_empty() && page_table_pages == 0 {
            return;
        }
        release_shm_attaches(&old_areas);
        drop(old_areas);
        crate::mm::reclaim::request_background_reclaim();
        info!(
            "[MEMDEBUG] pid={} released zombie user address space, page_table_pages={}",
            pid, page_table_pages
        );
    }

    /// Reparent children that still really belong to this process.
    ///
    /// CLONE_PARENT children can appear in an ancestor's children list, so this
    /// only moves entries whose parent weak pointer resolves back to `self`.
    /// Moved children are removed from the old list immediately, preventing a
    /// dead zombie parent from retaining a hidden subtree after adoption.
    pub fn reparent_children_to(&self, new_parent: &Arc<ProcessControlBlock>) -> bool {
        self.reparent_children_to_inner(new_parent, false)
    }

    pub(crate) fn reparent_children_to_with_tlb_progress(
        &self,
        new_parent: &Arc<ProcessControlBlock>,
    ) -> bool {
        self.reparent_children_to_inner(new_parent, true)
    }

    fn reparent_children_to_inner(
        &self,
        new_parent: &Arc<ProcessControlBlock>,
        tlb_progress: bool,
    ) -> bool {
        let pid = self.getpid();
        let children = {
            let mut inner = if tlb_progress {
                self.inner_exclusive_access_with_tlb_progress()
            } else {
                self.inner_exclusive_access()
            };
            core::mem::take(&mut inner.children)
        };
        if children.is_empty() {
            return false;
        }

        let mut adopted_children = Vec::new();
        let mut remaining_children = Vec::new();
        let mut should_wake_new_parent = false;

        for child in children {
            let belongs_to_self = {
                let mut child_inner = if tlb_progress {
                    child.inner_exclusive_access_with_tlb_progress()
                } else {
                    child.inner_exclusive_access()
                };
                let belongs = child_inner
                    .parent
                    .as_ref()
                    .and_then(|weak| weak.upgrade())
                    .is_some_and(|actual_parent| actual_parent.getpid() == pid);
                if belongs {
                    child_inner.parent = Some(Arc::downgrade(new_parent));
                    if child_inner.is_zombie && child_inner.alive_thread_count == 0 {
                        should_wake_new_parent = true;
                    }
                }
                belongs
            };

            if belongs_to_self {
                adopted_children.push(child);
            } else {
                remaining_children.push(child);
            }
        }

        if !remaining_children.is_empty() {
            let mut inner = if tlb_progress {
                self.inner_exclusive_access_with_tlb_progress()
            } else {
                self.inner_exclusive_access()
            };
            inner.children.extend(remaining_children);
        }
        if !adopted_children.is_empty() {
            let mut inner = if tlb_progress {
                new_parent.inner_exclusive_access_with_tlb_progress()
            } else {
                new_parent.inner_exclusive_access()
            };
            inner.children.extend(adopted_children);
        }

        should_wake_new_parent
    }

    fn write_tid_to_user(token: usize, ptr: usize, tid: usize) -> Result<(), SysError> {
        let mut bufs =
            translated_byte_buffer_for_write(token, ptr as *mut u8, core::mem::size_of::<i32>())?;
        let bytes = (tid as i32).to_ne_bytes();
        let mut copied = 0usize;
        for buf in bufs.iter_mut() {
            let len = (bytes.len() - copied).min(buf.len());
            buf[..len].copy_from_slice(&bytes[copied..copied + len]);
            copied += len;
            if copied == bytes.len() {
                return Ok(());
            }
        }
        Err(SysError::EFAULT)
    }

    fn write_bytes_to_vm_set(
        vm_set: &mut UserVMSet,
        ptr: usize,
        bytes: &[u8],
    ) -> Result<(), SysError> {
        let end = ptr.checked_add(bytes.len()).ok_or(SysError::EFAULT)?;
        if ptr < USER_MEMORY_SPACE.0 || end == 0 || end.saturating_sub(1) > USER_MEMORY_SPACE.1 {
            return Err(SysError::EFAULT);
        }

        let mut copied = 0usize;
        let mut va = ptr;
        while copied < bytes.len() {
            let start_va = VirtAddr::from(va);
            let vpn = start_va.floor();
            let writable = vm_set
                .page_table
                .translate(vpn)
                .map_or(false, |pte| pte.writable());
            if !writable {
                match vm_set.handle_store_page_fault_set(start_va, AccessType::Write) {
                    Some(PageFaultError::Normal) => {}
                    Some(PageFaultError::OutOfMemory) => return Err(SysError::ENOMEM),
                    _ => return Err(SysError::EFAULT),
                }
            }

            let Some(pte) = vm_set.page_table.translate(vpn) else {
                return Err(SysError::EFAULT);
            };
            if !pte.writable() {
                return Err(SysError::EFAULT);
            }

            let page_offset = start_va.page_offset();
            let len = (PAGE_SIZE - page_offset).min(bytes.len() - copied);
            pte.ppn().get_bytes_array()[page_offset..page_offset + len]
                .copy_from_slice(&bytes[copied..copied + len]);
            copied += len;
            va = va.checked_add(len).ok_or(SysError::EFAULT)?;
        }
        Ok(())
    }

    fn write_tid_to_vm_set(vm_set: &mut UserVMSet, ptr: usize, tid: usize) -> Result<(), SysError> {
        Self::write_bytes_to_vm_set(vm_set, ptr, &(tid as i32).to_ne_bytes())
    }

    fn write_minimal_initial_stack(
        vm_set: &mut UserVMSet,
        stack_top: usize,
        auxv: &[(usize, usize)],
    ) -> Result<usize, SysError> {
        let mut ptrs: Vec<usize> = vec![0, 0, 0]; // argc, argv NULL, envp NULL
        for (aux_type, aux_val) in auxv {
            ptrs.push(*aux_type);
            ptrs.push(*aux_val);
        }
        ptrs.push(0); // AT_NULL
        ptrs.push(0);

        let ptrs_size = ptrs.len() * core::mem::size_of::<usize>();
        let stack_bottom = stack_top.checked_sub(ptrs_size).ok_or(SysError::EFAULT)? & !0xF;
        let ptrs_bytes =
            unsafe { core::slice::from_raw_parts(ptrs.as_ptr() as *const u8, ptrs_size) };
        Self::write_bytes_to_vm_set(vm_set, stack_bottom, ptrs_bytes)?;
        Ok(stack_bottom)
    }

    #[allow(dead_code)]
    fn rollback_thread_clone(&self, tid: usize, global_tid: usize, task: &Arc<TaskControlBlock>) {
        {
            let mut inner = self.inner_exclusive_access();
            if tid < inner.tasks.len() {
                inner.tasks[tid] = None;
            }
            inner.alive_thread_count = inner.alive_thread_count.saturating_sub(1);
        }
        crate::task::remove_task(Arc::clone(task));
        crate::task::manager::remove_from_tid2task_if_present(global_tid);
        crate::syscall::futex::remove_task_from_futex_table(task);
        dealloc_pid(global_tid);
    }

    #[allow(unused)]
    #[allow(dead_code)]
    fn rollback_fork_clone(
        &self,
        child: &Arc<ProcessControlBlock>,
        task: &Arc<TaskControlBlock>,
        clone_parent: bool,
        grandparent: Option<&Arc<ProcessControlBlock>>,
    ) {
        let pid = child.getpid();
        crate::task::remove_task(Arc::clone(task));
        crate::task::manager::remove_from_tid2task_if_present(pid);
        if pid2process(pid).is_some() {
            remove_from_pid2process(pid);
        }
        if clone_parent {
            if let Some(gp) = grandparent {
                gp.inner_exclusive_access()
                    .children
                    .retain(|candidate| !Arc::ptr_eq(candidate, child));
            }
        } else {
            self.inner_exclusive_access()
                .children
                .retain(|candidate| !Arc::ptr_eq(candidate, child));
        }
        crate::syscall::futex::remove_task_from_futex_table(task);
        child.close_all_files_on_exit();
        child.release_user_space_on_exit();
        crate::task::manager::TIMER_PROCS.lock().remove(&pid);
        {
            let mut inner = child.inner_exclusive_access();
            inner.tasks.clear();
            inner.children.clear();
            inner.vfork_parent.take();
            inner.alive_thread_count = 0;
            inner.is_zombie = true;
        }
    }

    pub fn new(elf_data: &[u8]) -> Arc<Self> {
        // memory_set with elf program headers/trampoline/trap context/user stack
        // let (memory_set, ustack_base, entry_point) = UserVMSet::from_elf(elf_data);
        // allocate a pid

        // let memory_set = UserVMSet {
        //     inner: VMSet::new_bare(),
        // };
        let pid_handle = pid_alloc();
        let pid = pid_handle.as_usize();
        let kstack = kstack_alloc();

        let (vm_set, ustack_top, entry_point, auxv) = UserVMSet::from_elf(elf_data).unwrap();
        let user_token = vm_set.token();
        let tty_dentry =
            find_dentry("/dev/tty").expect("Failed to find /dev/tty! Make sure devfs is mounted.");

        let tty_file: Arc<dyn File> = Arc::new(TtyFile::new(tty_dentry));
        let files_context = SharedFiles::new(FilesContext {
            fd_table: vec![
                Some(tty_file.clone()), // fd 0: 准标准输入
                Some(tty_file.clone()), // fd 1: 标准输出
                Some(tty_file.clone()), // fd 2: 标准错误输出
            ],
            fd_flags: vec![0; 3],
        });
        let process = Arc::new(Self {
            pid: pid_handle,
            user_token: AtomicUsize::new(user_token),
            dumpable: AtomicBool::new(true),
            inner_owner_cpu: AtomicUsize::new(usize::MAX),
            inner_owner_line: AtomicUsize::new(0),
            final_exit_cleanup_pending: AtomicBool::new(false),
            child_event_seq: AtomicUsize::new(0),
            vm_set: SpinNoIrqLock::new(Arc::new(SleepLock::new(vm_set))),
            files_handle: SpinNoIrqLock::new(files_context.clone()),
            inner: SpinNoIrqLock::new(ProcessControlBlockInner {
                uid: 0,
                euid: 0,
                suid: 0,
                gid: 0,
                egid: 0,
                sgid: 0,
                is_zombie: false,
                is_stopped: false,
                was_continued: false,
                zombie_flag: AtomicBool::new(false),
                pgid: PgidHandle(pid),
                parent: None,
                children: Vec::new(),
                exit_code: 0,
                term_status: TermStatus::Running,
                fd_table: SharedFdTable(files_context.clone()),
                fd_flags: SharedFdFlags(files_context),
                // fd_table: vec![
                //     // 0 -> stdin
                //     Some(Arc::new(Stdin)),
                //     // 1 -> stdout
                //     Some(Arc::new(Stdout)),
                //     // 2 -> stderr
                //     Some(Arc::new(Stdout)),
                // ],
                tasks: Vec::new(),
                task_res_allocator: RecycleAllocator::new(),
                fs_context: Arc::new(SpinNoIrqLock::new(FsContext {
                    cwd: GLOBAL_DCACHE.get("/").unwrap().clone(),
                    umask: 0o022,
                })),
                executable_path: String::from("/initproc"),
                time: Tms::new(),
                ustart: 0,
                kstart: current_time().as_micros() as usize,
                state: ProcessStatus::Ready,

                pending_signals: SignalSet::empty(),
                pending_signal_queue: alloc::collections::VecDeque::new(),
                blocked_signals: SignalSet::empty(),
                signals_handler: Arc::new(SpinNoIrqLock::new(SignalHandlers::new())),
                wait_waker: None,
                need_signal_handle: false,
                itimer_real_deadline: None,
                itimer_real_interval: None,
                sig_context_stack: Vec::new(),
                alarm_deadline_us: None,
                alarm_interval_us: None,
                rlimit_fsize: Rlimit64 {
                    rlim_cur: RLIM_INFINITY,
                    rlim_max: RLIM_INFINITY,
                },
                rlimit_nofile: Rlimit64 {
                    rlim_cur: 1024,
                    rlim_max: 1024,
                },
                no_new_privs: false,
                has_cap_sys_admin: true,
                landlock: LandlockDomain::new(),
                alive_thread_count: 1,
                user_space_released: false,
                files_released: false,
                exec_owner_tid: None,
                vfork_parent: None,
                net_ns_id: 0,
                needs_post_wait_network_quiesce: false,
                exit_signal: 17, // SIGCHLD
                last_siginfo: None,
            }),
        });

        // create a main thread, we should allocate ustack and trap_cx here
        // Linux 语义：主线程的 tid 等于进程 pid
        let task = Arc::new(TaskControlBlock::new(
            Arc::clone(&process),
            ustack_top,
            true,
            kstack,
            pid,
        ));

        // prepare trap_cx of main thread
        let mut task_inner = task.inner_exclusive_access();
        let trap_cx = task_inner.get_trap_cx();
        let task_ustack_top = task_inner.res.as_ref().unwrap().ustack_top();
        let kstack_top = task.kstack.get_top();

        task_inner.task_cx[KContextArgs::KSP] = kstack_top;
        task_inner.task_cx[KContextArgs::KPC] = task_entry as usize;

        drop(task_inner);
        let initial_user_sp = {
            let vm_handle = process.vm_handle();
            let mut vm_set = vm_handle.lock();
            Self::write_minimal_initial_stack(&mut vm_set, task_ustack_top, &auxv)
                .expect("failed to prepare init process initial stack")
        };
        trap_cx[TrapFrameArgs::SEPC] = entry_point;
        #[cfg(target_arch = "riscv64")]
        unsafe {
            let sstatus_ptr = &mut trap_cx.sstatus as *mut _ as *mut usize;
            polyhal::println!("[DEBUG new] sstatus before={:#x}", *sstatus_ptr);
            *sstatus_ptr &= !(1 << 8);
            polyhal::println!("[DEBUG new] sstatus after={:#x}", *sstatus_ptr);
        }
        polyhal::println!("set sp {:#x}", initial_user_sp);
        trap_cx[TrapFrameArgs::SP] = initial_user_sp;
        // add main thread to the process
        let mut process_inner = process.inner_exclusive_access();
        process_inner.tasks.push(Some(Arc::clone(&task)));
        drop(process_inner);
        register_process(&process);
        insert_into_pid2process(process.getpid(), Arc::clone(&process));
        // add main thread to scheduler
        add_task(task);
        process
    }

    /// Remove every sibling thread before replacing this process's address space.
    ///
    /// Linux permits any thread to call execve. The winning caller survives and
    /// becomes the thread-group leader; all siblings must have stopped using the
    /// old VM before the caller can install the new image.
    fn de_thread_for_exec(self: &Arc<Self>, caller: &Arc<TaskControlBlock>) {
        let caller_global_tid = caller.inner_exclusive_access().global_tid;
        let siblings = {
            let mut inner = self.inner_exclusive_access();
            match inner.exec_owner_tid {
                Some(owner) if owner != caller_global_tid => {
                    drop(inner);
                    caller.request_exec_exit();
                    crate::task::exit_current_and_run_next(0);
                    return;
                }
                Some(_) => {}
                None => inner.exec_owner_tid = Some(caller_global_tid),
            }
            inner
                .tasks
                .iter()
                .filter_map(|task| task.as_ref().map(Arc::clone))
                .filter(|task| !Arc::ptr_eq(task, caller))
                .collect::<Vec<_>>()
        };

        let had_siblings = !siblings.is_empty();
        if had_siblings {
            info!(
                "[execve] de-thread start: pid={} caller_tid={} siblings={}",
                self.getpid(),
                caller_global_tid,
                siblings.len()
            );
        }

        for sibling in &siblings {
            sibling.request_exec_exit();
            let should_wake = {
                let mut task_inner = sibling.inner_exclusive_access();
                task_inner.interrupted_by_signal = true;
                task_inner.task_status != TaskStatus::Zombie
            };
            crate::task::remove_task_from_timer_queue(sibling);
            crate::syscall::futex::remove_task_from_futex_table(sibling);
            if should_wake {
                crate::task::wakeup_task(Arc::clone(sibling));
            }
        }
        drop(siblings);

        loop {
            let (siblings_left, process_is_zombie, exit_code) = {
                let inner = self.inner_exclusive_access();
                (
                    inner
                        .tasks
                        .iter()
                        .flatten()
                        .any(|task| !Arc::ptr_eq(task, caller)),
                    inner.is_zombie,
                    inner.exit_code,
                )
            };
            if process_is_zombie {
                crate::task::exit_current_and_run_next(exit_code);
                return;
            }
            if !siblings_left {
                break;
            }
            crate::task::suspend_current_and_run_next();
        }

        // Canonicalize the survivor as local TID 0 and global TID == PID. Use
        // try-locking for task -> process so this cannot deadlock with paths
        // which briefly inspect the task while already holding the PCB lock.
        let pid = self.getpid();
        let old_global_tid = loop {
            let mut task_inner = caller.inner_exclusive_access();
            let Some(mut process_inner) = self.try_inner_exclusive_access() else {
                drop(task_inner);
                core::hint::spin_loop();
                continue;
            };

            let old_global_tid = task_inner.global_tid;
            let task_res = task_inner
                .res
                .as_mut()
                .expect("execve caller lost its user resources");
            task_res.tid = 0;
            task_res.global_tid = pid;
            task_inner.global_tid = pid;
            task_inner.clear_child_tid = 0;
            task_inner.saved_sigtrapframe = None;
            task_inner.interrupted_by_signal = false;
            task_inner.pending_signals = SignalSet::empty();
            task_inner.pending_signal_queue.clear();
            task_inner.need_signal_handle = false;
            task_inner.sig_context_stack.clear();
            task_inner.signal_wait_old_masks.clear();
            task_inner.signal_alt_stack = crate::task::signal::SignalAltStack::disabled();
            task_inner.futex_woken = false;
            task_inner.futex_timed_out = false;
            task_inner.pending_wakeup = false;
            task_inner.vfork_child_pid = None;
            task_inner.requeue_after_switch = false;
            task_inner.requeue_front_after_switch = false;
            task_inner.robust_list_head = 0;
            task_inner.robust_list_len = 0;
            task_inner.exit_code = None;
            task_inner.auto_reap_on_exit = false;
            task_inner.zombie_flag.store(false, Ordering::Release);

            process_inner.tasks.clear();
            process_inner.tasks.push(Some(Arc::clone(caller)));
            process_inner.task_res_allocator = RecycleAllocator::with_start(1);
            process_inner.alive_thread_count = 1;
            process_inner.exec_owner_tid = None;
            break old_global_tid;
        };

        remove_from_tid2task(old_global_tid);
        if old_global_tid != pid {
            dealloc_pid(old_global_tid);
        }
        insert_into_tid2task(pid, Arc::clone(caller));
        caller.clear_exec_exit_request();

        if caller_global_tid != pid {
            info!(
                "[execve] caller promoted to thread-group leader: pid={} old_tid={}",
                pid, caller_global_tid
            );
        }
        if had_siblings {
            info!("[execve] de-thread complete: pid={}", pid);
        }
    }

    pub fn execve(
        self: &Arc<Self>,
        elf_data: &[u8],
        args: Vec<String>,
        envs: Vec<String>,
    ) -> isize {
        trace!("execve");
        //println!("execve a new elf for process");
        // memory_set with elf program headers/trampoline/trap context/user stack
        let elf_result = UserVMSet::from_elf(elf_data);
        let (memory_set, ustack_base, entry_point, auxv) = match elf_result {
            Some(res) => res,
            None => {
                // BusyBox 收到 -8 后会自动把它当成 Shell 脚本去解释执行！
                return -8;
            }
        };
        self.execve_loaded(memory_set, ustack_base, entry_point, auxv, None, args, envs)
    }

    pub fn execve_file(
        self: &Arc<Self>,
        file: &Arc<dyn File>,
        path: &str,
        args: Vec<String>,
        envs: Vec<String>,
    ) -> isize {
        trace!("execve_file");
        let active_task = crate::task::current_task();
        if let Some(task) = active_task.as_ref() {
            task.set_active_syscall_stage(22140);
        }
        let elf_result = UserVMSet::from_elf_file(file, path);
        if let Some(task) = active_task.as_ref() {
            task.set_active_syscall_stage(22141);
        }
        let (memory_set, ustack_base, entry_point, auxv) = match elf_result {
            Some(res) => res,
            None => {
                // BusyBox 收到 -8 后会自动把它当成 Shell 脚本去解释执行！
                return -8;
            }
        };
        self.execve_loaded(
            memory_set,
            ustack_base,
            entry_point,
            auxv,
            Some(String::from(path)),
            args,
            envs,
        )
    }

    fn execve_loaded(
        self: &Arc<Self>,
        memory_set: UserVMSet,
        ustack_base: usize,
        entry_point: usize,
        auxv: Vec<(usize, usize)>,
        executable_path: Option<String>,
        args: Vec<String>,
        envs: Vec<String>,
    ) -> isize {
        let caller = crate::task::current_task().expect("execve without a current task");
        caller.set_active_syscall_stage(22150);
        self.de_thread_for_exec(&caller);
        caller.set_active_syscall_stage(22151);

        let new_user_token = memory_set.token();

        // This kernel does not support set-id executables, so a successful
        // exec resets dumpability to the normal Linux value and renames the
        // surviving thread after the executable basename.
        self.set_dumpable(true);
        if let Some(path) = executable_path.as_deref() {
            let basename = path
                .rsplit('/')
                .find(|part| !part.is_empty())
                .unwrap_or(path);
            caller.set_comm(basename.as_bytes());
        }

        // substitute memory_set
        let mut files_to_flush = Vec::new();
        let mut sockets_to_close = Vec::new();
        let pid = self.getpid();
        caller.set_active_syscall_stage(22152);
        let old_vm_handle = {
            let replacement = Arc::new(SleepLock::new(memory_set));
            let old = self.replace_vm_handle(replacement);
            self.set_user_token(new_user_token);

            let mut inner = self.inner_exclusive_access();
            if let Some(executable_path) = executable_path {
                inner.executable_path = executable_path;
            }
            // POSIX: execve 必须重置所有信号处理器为 SIG_DFL（SIG_IGN 保持不变）
            let private_handlers =
                Arc::new(SpinNoIrqLock::new(inner.signals_handler.lock().clone()));
            private_handlers.lock().reset_all();
            inner.signals_handler = private_handlers;
            inner.pending_signals = SignalSet::empty();
            inner.pending_signal_queue.clear();
            inner.need_signal_handle = false;
            // POSIX: execve 关闭所有设置了 FD_CLOEXEC 的文件描述符
            // execve unshares CLONE_FILES before applying close-on-exec.
            let private_files = SharedFiles::new(FilesContext {
                fd_table: inner.fd_table.clone(),
                fd_flags: inner.fd_flags.clone(),
            });
            let old_files = inner.fd_table.0.clone();
            inner.fd_table = SharedFdTable(private_files.clone());
            inner.fd_flags = SharedFdFlags(private_files.clone());
            *self.files_handle.lock() = private_files;
            old_files.owners.fetch_sub(1, Ordering::AcqRel);
            let fd_len = inner.fd_table.len();
            for fd in 0..fd_len {
                if inner.fd_flags.get(fd).copied().unwrap_or(0) & 1 != 0 {
                    if let Some(file) = inner.fd_table[fd].take() {
                        files_to_flush.push(file);
                        sockets_to_close.push(fd);
                        crate::syscall::remove_fs_context(pid, fd);
                    }
                    if fd < inner.fd_flags.len() {
                        inner.fd_flags[fd] = 0;
                    }
                }
            }
            old
        };

        // Publish the new process VM before installing it in hardware. The
        // vm_set mutex may sleep; activating the new root before that wait lets
        // the scheduler reinstall the still-published old token when this task
        // resumes, leaving software and hardware page-table state divergent.
        // Once replacement and publication are complete, this activation is
        // stable across every later scheduling boundary.
        self.activate_user_page_table();
        caller.set_active_syscall_stage(22153);

        // A CPU that trapped from a sibling still has the old root installed
        // while it finishes its kernel-side exit path. TLB shootdown alone is
        // insufficient: software walkers and kernel instruction/data accesses
        // still depend on the page-table pages themselves. Do not recycle the
        // old root until every CPU has installed a different token.
        let (old_user_token, old_mm_shared) = {
            let mut old_vm_set = old_vm_handle.lock();
            old_vm_set.process_owners = old_vm_set
                .process_owners
                .checked_sub(1)
                .expect("mm process owner underflow during exec");
            (old_vm_set.token(), old_vm_set.process_owners != 0)
        };
        let mut wait_logged = false;
        while !old_mm_shared {
            let active_mask = crate::mm::vm_set::active_page_table_mask(old_user_token);
            if active_mask == 0 {
                break;
            }
            caller.set_active_syscall_stage(22154);
            if !wait_logged {
                error!(
                    "[PAGE_TABLE_RETIRE_WAIT] enter pid={} old_token={:#x} new_token={:#x} active_mask={:#x}",
                    pid, old_user_token, new_user_token, active_mask
                );
                wait_logged = true;
            }
            // Address-space replacement is already committed. Abandoning this
            // continuation here would strand old page-table frames and leave a
            // partially completed exec image, so termination is honored only
            // after retirement finishes.
            crate::task::suspend_current_kernel_continuation();
        }
        if wait_logged {
            error!(
                "[PAGE_TABLE_RETIRE_WAIT] done pid={} old_token={:#x} new_token={:#x}",
                pid, old_user_token, new_user_token
            );
        }
        {
            let old_vm_set = old_vm_handle.lock();
            release_shm_attaches(&old_vm_set.areas);
        }
        drop(old_vm_handle);
        caller.set_active_syscall_stage(22155);
        for file in files_to_flush {
            crate::syscall::release_process_file_locks(pid, &file);
            crate::fs::writeback::queue_file(file);
        }
        let mut manager = crate::socket::SOCKET_MANAGER.lock();
        for fd in sockets_to_close {
            let _ = manager.close_socket_with_refcount(fd, pid);
        }
        drop(manager);
        // then we alloc user resource for main thread again
        // since memory_set has been changed
        let task = self.inner_exclusive_access().get_task(0);
        let mut task_inner = task.inner_exclusive_access();
        // execve installs a new address space, so the old thread-local rseq
        // pointer must never survive into the new program image.
        task_inner.rseq_address = 0;
        task_inner.rseq_len = 0;
        task_inner.rseq_signature = 0;
        task_inner.rseq_signal_fault_bypass = false;
        task_inner.rseq_prepare_fault_bypass = false;
        task.complete_rseq_resume_update();
        task_inner
            .res
            .as_mut()
            .unwrap()
            .rebind_user_res(ustack_base);
        caller.set_active_syscall_stage(22156);

        trace!("ustack base: {:#x}", ustack_base);
        task_inner.res.as_mut().unwrap().alloc_user_res();
        // task_inner.trap_cx_ppn = task_inner.res.as_mut().unwrap().trap_cx_ppn();
        task_inner.trap_cx = TrapFrame::new();
        // push arguments on user stack
        let mut user_sp = task_inner.res.as_mut().unwrap().ustack_top();
        drop(task_inner);

        let user_token = task.get_user_token();
        // Copy through the user translation path so the lazy user stack is populated on demand.
        let write_to_user = |mut va: usize, data: &[u8]| -> Result<(), SysError> {
            let mut offset = 0;
            while offset < data.len() {
                let page_offset = va % PAGE_SIZE;
                let write_len = (PAGE_SIZE - page_offset).min(data.len() - offset);
                trace!("va {:#x} write to user", va);
                let mut buffers =
                    translated_byte_buffer_for_write(user_token, va as *mut u8, write_len)?;
                if buffers.len() != 1 {
                    return Err(SysError::EFAULT);
                }
                buffers[0].copy_from_slice(&data[offset..offset + write_len]);

                va += write_len;
                offset += write_len;
            }
            Ok(())
        };
        let mut arg_ptrs: Vec<usize> = Vec::new();
        let mut env_ptrs: Vec<usize> = Vec::new();
        caller.set_active_syscall_stage(22157);

        //压入环境变量字符串 (Env)
        for env in envs.iter() {
            let bytes = env.as_bytes();
            user_sp -= bytes.len() + 1;
            if let Err(err) = write_to_user(user_sp, bytes) {
                return -(err.code() as isize);
            }
            if let Err(err) = write_to_user(user_sp + bytes.len(), &[0]) {
                return -(err.code() as isize);
            }
            env_ptrs.push(user_sp);
        }

        // 压入参数字符串 (Args)
        for arg in args.iter() {
            let bytes = arg.as_bytes();
            user_sp -= bytes.len() + 1;
            if let Err(err) = write_to_user(user_sp, bytes) {
                return -(err.code() as isize);
            }
            if let Err(err) = write_to_user(user_sp + bytes.len(), &[0]) {
                return -(err.code() as isize);
            }
            arg_ptrs.push(user_sp);
        }
        user_sp &= !0xF;
        //压入auxv
        user_sp -= 16;
        let random_ptr = user_sp;
        let mut random_bytes = [0u8; 16];
        fill_random(&mut random_bytes);
        if let Err(err) = write_to_user(random_ptr, &random_bytes) {
            return -(err.code() as isize);
        }

        user_sp &= !0xF;
        //指针数组
        // 布局：[argc, argv[0], ..., NULL, envp[0], ..., NULL]
        let mut ptrs: Vec<usize> = Vec::new();
        ptrs.push(args.len()); // argc
        for ptr in arg_ptrs.iter() {
            ptrs.push(*ptr);
        } // argv pointers
        ptrs.push(0);
        for ptr in env_ptrs.iter() {
            ptrs.push(*ptr);
        } // envp pointers
        ptrs.push(0);

        for (aux_type, aux_val) in auxv {
            ptrs.push(aux_type);
            ptrs.push(aux_val);
        }
        // glibc 启动期会使用这两个辅助向量项。
        const AT_RANDOM: usize = 25;
        const AT_EXECFN: usize = 31;
        ptrs.push(AT_RANDOM);
        ptrs.push(random_ptr);
        ptrs.push(AT_EXECFN);
        ptrs.push(arg_ptrs.first().copied().unwrap_or(0));
        ptrs.push(0); // AT_NULL (结束标志)
        ptrs.push(0);

        // 将指针数组压入用户栈
        let ptrs_size = ptrs.len() * core::mem::size_of::<usize>();
        user_sp -= ptrs_size;
        user_sp &= !0xF; // 16字节对齐
        let ptrs_bytes =
            unsafe { core::slice::from_raw_parts(ptrs.as_ptr() as *const u8, ptrs_size) };
        if let Err(err) = write_to_user(user_sp, ptrs_bytes) {
            return -(err.code() as isize);
        }
        // unsafe {
        //     riscv::register::satp::write(task_satp);
        //     core::arch::asm!("sfence.vma");
        // }
        // initialize trap_cx
        let mut trap_cx = TrapFrame::new();

        trap_cx[TrapFrameArgs::SEPC] = entry_point;
        #[cfg(target_arch = "riscv64")]
        unsafe {
            let sstatus_ptr = &mut trap_cx.sstatus as *mut _ as *mut usize;
            *sstatus_ptr &= !(1 << 8);
        }
        info!("user sp {:#x}", user_sp);
        trap_cx[TrapFrameArgs::SP] = user_sp;
        trap_cx[TrapFrameArgs::ARG0] = 0;
        trap_cx[TrapFrameArgs::ARG1] = 0;
        trap_cx[TrapFrameArgs::ARG2] = 0;

        let task_inner = task.inner_exclusive_access();
        *task_inner.get_trap_cx() = trap_cx;
        drop(task_inner);
        caller.set_active_syscall_stage(22158);
        let vfork_parent = {
            let mut inner = self.inner_exclusive_access();
            inner.vfork_parent.take()
        };
        if let Some(parent_task) = vfork_parent {
            let parent_pid = parent_task
                .process
                .upgrade()
                .map(|process| process.getpid())
                .unwrap_or(usize::MAX);
            let (parent_tid, parent_status, parent_pending_wakeup, completion_matched) = {
                let mut parent_inner = parent_task.inner_exclusive_access();
                let completion_matched = parent_inner.vfork_child_pid == Some(pid);
                if completion_matched {
                    parent_inner.vfork_child_pid = None;
                }
                (
                    parent_inner.global_tid,
                    parent_inner.task_status,
                    parent_inner.pending_wakeup,
                    completion_matched,
                )
            };
            let should_wake = completion_matched
                && matches!(
                    parent_status,
                    crate::task::TaskStatus::Blocked
                        | crate::task::TaskStatus::Sleep
                        | crate::task::TaskStatus::Ready
                );
            let child_executable = self.inner_exclusive_access().executable_path.clone();
            error!(
                "[VFORK_WAKE_EXEC] cpu={} child_pid={} parent_pid={} parent_tid={} parent_status={:?} parent_pending_wakeup={} wake_submitted={} child_executable={}",
                polyhal::arch::hart_id(),
                pid,
                parent_pid,
                parent_tid,
                parent_status,
                parent_pending_wakeup,
                should_wake,
                child_executable,
            );
            if should_wake {
                crate::task::wakeup_task(parent_task);
            }
        }
        0
        // *task_inner.get_trap_cx() = trap_cx;
    }

    pub fn getpid(&self) -> usize {
        self.pid.as_usize()
    }

    pub fn release_pid_handle(&self) {
        self.pid.release();
    }

    pub fn getpgid(&self) -> usize {
        self.inner_exclusive_access().pgid.0
    }

    pub fn setpgid(&self, pgid: usize) {
        self.inner_exclusive_access().pgid = PgidHandle(pgid);
    }

    pub fn _clone(
        self: &Arc<Self>,
        _flags: u32,
        _stack: usize,
        _ptid: usize,
        _ctid: usize,
        _tls: usize,
        _exit_signal: i32,
        _clear_sighand: bool,
    ) -> isize {
        self._clone_inner(
            _flags,
            _stack,
            _ptid,
            _ctid,
            _tls,
            _exit_signal,
            _clear_sighand,
        )
    }

    fn _clone_inner(
        self: &Arc<Self>,
        _flags: u32,
        _stack: usize,
        _ptid: usize,
        _ctid: usize,
        _tls: usize,
        _exit_signal: i32,
        _clear_sighand: bool,
    ) -> isize {
        if (_flags & CLONE_THREAD) != 0 {
            // 线程创建路径：共享进程、地址空间、fd_table 等
            let caller_task = crate::task::current_task().unwrap();
            let clone_trace = ForkCloneTraceGuard::begin(self.getpid());

            // 1. Snapshot all caller state before acquiring any child lock.
            // Keeping child.inner while acquiring caller.inner creates a
            // task-to-task lock dependency in the non-preemptible kernel and
            // can pin the cloning CPU indefinitely behind concurrent task
            // teardown. One snapshot also shortens the total no-IRQ interval.
            clone_trace.phase(2);
            let task0 = {
                let inner = self.inner_exclusive_access();
                inner.get_task(0).clone()
            };
            let ustack_base = task0
                .inner_exclusive_access()
                .res
                .as_ref()
                .unwrap()
                .ustack_base();
            let (caller_trap_cx, caller_blocked_signals, caller_comm) = {
                let caller_inner = caller_task.inner_exclusive_access();
                (
                    caller_inner.trap_cx.clone(),
                    caller_inner.blocked_signals.clone(),
                    caller_inner.comm,
                )
            };

            // 2. 释放进程锁后再创建 TaskControlBlock
            //    避免在持有进程锁时调用 TaskControlBlock::new（内部会再次获取进程锁）
            //    同时也避免 process.inner -> task.inner 的锁顺序，防止与 exit_current_and_run_next 死锁。
            clone_trace.phase(3);
            let global_tid = alloc_pid_raw();
            let kstack = kstack_alloc();
            let task = Arc::new(TaskControlBlock::new(
                Arc::clone(self),
                ustack_base,
                false,
                kstack,
                global_tid,
            ));
            clone_trace.phase(4);
            if caller_task.sched_reset_on_fork() {
                task.set_sched(0, 0);
                task.set_sched_reset_on_fork(false);
            } else {
                task.set_sched(caller_task.sched_policy(), caller_task.sched_priority());
                task.set_sched_reset_on_fork(false);
            }
            task.set_affinity_mask(caller_task.affinity_mask());
            let tid = task.inner_exclusive_access().res.as_ref().unwrap().tid;
            insert_into_tid2task(global_tid, Arc::clone(&task));

            // 3. 将新线程加入当前进程的 tasks
            {
                let mut parent_inner = self.inner_exclusive_access();
                parent_inner.alive_thread_count += 1;
                let tasks = &mut parent_inner.tasks;
                while tasks.len() < tid + 1 {
                    tasks.push(None);
                }
                tasks[tid] = Some(Arc::clone(&task));
            }

            clone_trace.phase(5);
            // 4. Linux CLONE_THREAD tasks are detached from waitpid-style reaping.
            {
                let mut t_inner = task.inner_exclusive_access();
                t_inner.comm = caller_comm;
                t_inner.auto_reap_on_exit = true;
                if _ctid != 0 && (_flags & CLONE_CHILD_CLEARTID) != 0 {
                    t_inner.clear_child_tid = _ctid;
                }
            }

            // 5. CLONE_PARENT_SETTID：将 global_tid 写入 ptid 指向的用户地址
            // Linux deliberately ignores put_user() failure for parent_tid;
            // the task has already been created and must not be rolled back.
            if _ptid != 0 && (_flags & CLONE_PARENT_SETTID) != 0 {
                let token = crate::task::current_user_token();
                if let Err(err) = Self::write_tid_to_user(token, _ptid, global_tid) {
                    warn!(
                        "[clone] parent_tid write failed: mode=thread flags={:#x} ptid={:#x} ctid={:#x} tls={:#x} tid={} err={:?}",
                        _flags, _ptid, _ctid, _tls, global_tid, err
                    );
                    error!(
                        "[CLONE_TID_STORE_IGNORED] mode=thread kind=parent flags={:#x} ptr={:#x} tid={} err={:?}",
                        _flags, _ptid, global_tid, err
                    );
                }
            }

            // schedule_tail() performs the Linux child_tid store as a
            // best-effort put_user() before the child first returns to user.
            if _ctid != 0 && (_flags & CLONE_CHILD_SETTID) != 0 {
                let token = crate::task::current_user_token();
                if let Err(err) = Self::write_tid_to_user(token, _ctid, global_tid) {
                    warn!(
                        "[clone] child_tid write failed: mode=thread flags={:#x} ptid={:#x} ctid={:#x} tls={:#x} tid={} err={:?}",
                        _flags, _ptid, _ctid, _tls, global_tid, err
                    );
                    error!(
                        "[CLONE_TID_STORE_IGNORED] mode=thread kind=child flags={:#x} ptr={:#x} tid={} err={:?}",
                        _flags, _ctid, global_tid, err
                    );
                }
            }

            clone_trace.phase(6);
            // 6. 设置 trap_cx
            {
                let mut task_inner = task.inner_exclusive_access();
                let trap_cx = task_inner.get_trap_cx();
                trap_cx.clone_from(&caller_trap_cx);
                if _stack != 0 {
                    info!("_clone thread: set sp to {:#x}", _stack);
                    trap_cx[TrapFrameArgs::SP] = _stack;
                }
                if (_flags & CLONE_SETTLS) != 0 {
                    trap_cx[TrapFrameArgs::TLS] = _tls;
                }
                trap_cx[TrapFrameArgs::RET] = 0; // 子线程 clone 返回 0
                task_inner.blocked_signals = caller_blocked_signals;
            }

            clone_trace.phase(7);
            enqueue_new_clone_task(task);
            clone_trace.phase(8);
            info!("_clone thread: created tid {}", tid);
            global_tid as isize
        } else {
            // fork 路径：创建新进程
            let parent_pid = self.getpid();
            let parent_task = crate::task::current_task().expect("fork without a current task");
            debug_assert!(
                parent_task
                    .process
                    .upgrade()
                    .is_some_and(|process| Arc::ptr_eq(&process, self))
            );
            let fork_trace = ForkCloneTraceGuard::begin(parent_pid);
            let (
                memory_set,
                parent_files,
                inherited_task_slots,
                child_parent_weak,
                grandparent_opt,
                parent_uid,
                parent_euid,
                parent_suid,
                parent_gid,
                parent_egid,
                parent_sgid,
                parent_pgid,
                parent_fs_context,
                parent_executable_path,
                parent_blocked_signals_for_process,
                parent_signal_handlers,
                parent_rlimit_fsize,
                parent_rlimit_nofile,
                parent_no_new_privs,
                parent_has_cap_sys_admin,
                parent_landlock,
                parent_net_ns_id,
            ) = {
                fork_trace.phase(2);
                // fork() from a multithreaded process copies only the caller.
                let parent_blocked_signals_for_process = crate::task::current_task()
                    .unwrap()
                    .inner_exclusive_access()
                    .blocked_signals;
                // CLONE_VM shares the complete mm object, including for
                // vfork. The vfork parent is blocked until exec/exit, so a COW
                // snapshot here would both violate Linux and hide child-side
                // stack/argument writes from the parent address space.
                let share_vm = (_flags & CLONE_VM) != 0;
                let memory_set = if share_vm {
                    let handle = self.vm_handle();
                    {
                        let mut vm_set = handle.lock();
                        vm_set.process_owners = vm_set
                            .process_owners
                            .checked_add(1)
                            .expect("mm process owner overflow during CLONE_VM");
                    }
                    handle
                } else {
                    let parent_vm_handle = self.vm_handle();
                    let mut parent_vm = parent_vm_handle.lock();
                    Arc::new(SleepLock::new(UserVMSet::from_existed_user_cow(
                        &mut parent_vm,
                        parent_pid,
                    )))
                };
                fork_trace.phase(3);
                let parent = self.inner_exclusive_access();
                let parent_files = if (_flags & CLONE_FILES) != 0 {
                    let files = parent.fd_table.0.clone();
                    files.owners.fetch_add(1, Ordering::AcqRel);
                    files
                } else {
                    SharedFiles::new(FilesContext {
                        fd_table: parent.fd_table.clone(),
                        fd_flags: parent.fd_flags.clone(),
                    })
                };
                let child_parent_weak = if (_flags & CLONE_PARENT) != 0 {
                    parent.parent.clone()
                } else {
                    Some(Arc::downgrade(self))
                };
                let grandparent_opt = if (_flags & CLONE_PARENT) != 0 {
                    parent.parent.clone().and_then(|parent| parent.upgrade())
                } else {
                    None
                };
                (
                    memory_set,
                    parent_files,
                    parent.tasks.len().max(1),
                    child_parent_weak,
                    grandparent_opt,
                    parent.uid,
                    parent.euid,
                    parent.suid,
                    parent.gid,
                    parent.egid,
                    parent.sgid,
                    parent.pgid,
                    if (_flags & CLONE_FS) != 0 {
                        parent.fs_context.clone()
                    } else {
                        Arc::new(SpinNoIrqLock::new(parent.fs_context.lock().clone()))
                    },
                    parent.executable_path.clone(),
                    parent_blocked_signals_for_process,
                    if (_flags & CLONE_SIGHAND) != 0 {
                        parent.signals_handler.clone()
                    } else {
                        let handlers =
                            Arc::new(SpinNoIrqLock::new(parent.signals_handler.lock().clone()));
                        if _clear_sighand {
                            handlers.lock().reset_all();
                        }
                        handlers
                    },
                    parent.rlimit_fsize,
                    parent.rlimit_nofile,
                    parent.no_new_privs,
                    parent.has_cap_sys_admin,
                    parent.landlock.clone(),
                    parent.net_ns_id,
                )
            };
            fork_trace.phase(4);
            let child_user_token = memory_set.lock().token();
            let pid = pid_alloc();
            let sockets_to_clone: Vec<(usize, SocketInner)> = {
                let _files_guard = parent_files.gate.lock();
                let socket_fds: Vec<usize> = unsafe { &*parent_files.data.get() }
                    .fd_table
                    .iter()
                    .enumerate()
                    .filter_map(|(fd, file)| {
                        file.as_ref().filter(|file| file.is_socket()).map(|_| fd)
                    })
                    .collect();
                if socket_fds.is_empty() {
                    Vec::new()
                } else {
                    let manager = SOCKET_MANAGER.lock();
                    socket_fds
                        .into_iter()
                        .filter_map(|fd| {
                            manager
                                .get_socket(fd, parent_pid)
                                .map(|sock| (fd, sock.inner.clone()))
                        })
                        .collect()
                }
            };

            let child = Arc::new(Self {
                pid,
                user_token: AtomicUsize::new(child_user_token),
                dumpable: AtomicBool::new(self.dumpable()),
                inner_owner_cpu: AtomicUsize::new(usize::MAX),
                inner_owner_line: AtomicUsize::new(0),
                final_exit_cleanup_pending: AtomicBool::new(false),
                child_event_seq: AtomicUsize::new(0),
                vm_set: SpinNoIrqLock::new(memory_set),
                files_handle: SpinNoIrqLock::new(parent_files.clone()),
                inner: SpinNoIrqLock::new(ProcessControlBlockInner {
                    uid: parent_uid,
                    euid: parent_euid,
                    suid: parent_suid,
                    gid: parent_gid,
                    egid: parent_egid,
                    sgid: parent_sgid,
                    is_zombie: false,
                    is_stopped: false,
                    was_continued: false,
                    zombie_flag: AtomicBool::new(false),
                    pgid: parent_pgid,
                    parent: child_parent_weak,
                    children: Vec::new(),
                    exit_code: 0,
                    term_status: TermStatus::Running,
                    fd_table: SharedFdTable(parent_files.clone()),
                    fd_flags: SharedFdFlags(parent_files),
                    tasks: Vec::new(),
                    task_res_allocator: RecycleAllocator::new(),
                    fs_context: parent_fs_context,
                    executable_path: parent_executable_path,
                    time: Tms::new(),
                    ustart: 0,
                    kstart: current_time().as_micros() as usize,
                    state: ProcessStatus::Ready,
                    pending_signals: SignalSet::empty(),
                    pending_signal_queue: alloc::collections::VecDeque::new(),
                    blocked_signals: parent_blocked_signals_for_process,
                    signals_handler: parent_signal_handlers,
                    need_signal_handle: false,
                    itimer_real_deadline: None,
                    itimer_real_interval: None,
                    wait_waker: None,
                    sig_context_stack: Vec::new(),
                    alarm_deadline_us: None,
                    alarm_interval_us: None,
                    rlimit_fsize: parent_rlimit_fsize,
                    rlimit_nofile: parent_rlimit_nofile,
                    no_new_privs: parent_no_new_privs,
                    has_cap_sys_admin: parent_has_cap_sys_admin,
                    landlock: parent_landlock,
                    alive_thread_count: 1,
                    user_space_released: false,
                    files_released: false,
                    exec_owner_tid: None,
                    vfork_parent: None,
                    net_ns_id: if (_flags & CLONE_NEWNET) != 0 {
                        crate::fs::procfs::net_ipv4_conf::alloc_net_ns(parent_net_ns_id)
                    } else {
                        parent_net_ns_id
                    },
                    needs_post_wait_network_quiesce: false,
                    exit_signal: _exit_signal,
                    last_siginfo: None,
                }),
            });
            register_process(&child);
            {
                let child_vm_handle = child.vm_handle();
                let child_vm = child_vm_handle.lock();
                fork_inherit_shm_attach(&child_vm.areas, child.getpid());
            }
            {
                let mut manager = SOCKET_MANAGER.lock();
                for (fd, inner) in sockets_to_clone {
                    let new_socket = Socket::new(inner, fd, child.getpid());
                    let _ = manager.add_socket(fd, new_socket, child.getpid());
                }
            }
            let kstack = kstack_alloc();
            let (
                ustack_base,
                parent_trap_cx,
                parent_blocked_signals,
                parent_rseq,
                parent_signal_alt_stack,
                parent_sched_policy,
                parent_sched_priority,
                parent_sched_reset_on_fork,
                parent_comm,
            ) = {
                let parent_task_inner = parent_task.inner_exclusive_access();
                (
                    parent_task_inner.res.as_ref().unwrap().ustack_base(),
                    parent_task_inner.trap_cx.clone(),
                    parent_task_inner.blocked_signals.clone(),
                    (
                        parent_task_inner.rseq_address,
                        parent_task_inner.rseq_len,
                        parent_task_inner.rseq_signature,
                    ),
                    parent_task_inner.signal_alt_stack,
                    parent_task.sched_policy(),
                    parent_task.sched_priority(),
                    parent_task.sched_reset_on_fork(),
                    parent_task_inner.comm,
                )
            };
            let task = Arc::new(TaskControlBlock::new(
                Arc::clone(&child),
                ustack_base,
                false,
                kstack,
                child.getpid(),
            ));
            fork_trace.phase(5);
            if parent_sched_reset_on_fork {
                task.set_sched(0, 0);
            } else {
                task.set_sched(parent_sched_policy, parent_sched_priority);
            }
            task.set_sched_reset_on_fork(false);
            task.set_affinity_mask(parent_task.affinity_mask());
            let mut child_inner = child.inner_exclusive_access();
            // The copied VM still contains every parent thread's old stack
            // mapping. Keep those local stack slots reserved so a later
            // pthread_create in the child cannot map over inherited memory.
            child_inner.task_res_allocator = RecycleAllocator::with_start(inherited_task_slots);
            child_inner.tasks.push(Some(Arc::clone(&task)));
            if (_flags & CLONE_VFORK) != 0 {
                let caller_task = crate::task::current_task().unwrap();
                child_inner.vfork_parent = Some(caller_task);
            }
            drop(child_inner);

            // CLONE_CHILD_CLEARTID：设置 clear_child_tid
            if _ctid != 0 && (_flags & CLONE_CHILD_CLEARTID) != 0 {
                let mut t_inner = task.inner_exclusive_access();
                t_inner.clear_child_tid = _ctid;
            }

            let mut task_inner = task.inner_exclusive_access();
            task_inner.comm = parent_comm;
            task_inner.blocked_signals = parent_blocked_signals;
            // fork/vfork 继承备用栈；共享 VM 且非 vfork 的 clone 子任务必须禁用它。
            if (_flags & CLONE_VM) == 0 || (_flags & CLONE_VFORK) != 0 {
                task_inner.signal_alt_stack = parent_signal_alt_stack;
            }
            // Linux preserves rseq across fork (a private/COW address space),
            // but clears it for CLONE_VM children such as newly created
            // threads. The CLONE_THREAD path above starts with an empty TCB.
            if (_flags & CLONE_VM) == 0 {
                task_inner.rseq_address = parent_rseq.0;
                task_inner.rseq_len = parent_rseq.1;
                task_inner.rseq_signature = parent_rseq.2;
                if parent_rseq.0 != 0 {
                    task.request_rseq_resume_update();
                }
            }
            let trap_cx = task_inner.get_trap_cx();
            trap_cx.clone_from(&parent_trap_cx);
            if _stack != 0 {
                info!("_clone fork: set sp to {:#x}", _stack);
                trap_cx[TrapFrameArgs::SP] = _stack;
            }
            if (_flags & CLONE_SETTLS) != 0 {
                trap_cx[TrapFrameArgs::TLS] = _tls;
            }
            trap_cx[TrapFrameArgs::RET] = 0;
            #[cfg(target_arch = "loongarch64")]
            warn!(
                "[la64 fork] prepared child: parent_pid={} child_pid={} flags={:#x} parent_era={:#x} parent_sp={:#x} parent_ret={:#x} child_era={:#x} child_sp={:#x} child_ret={:#x} child_kstack={:#x}",
                self.getpid(),
                child.getpid(),
                _flags,
                parent_trap_cx.era,
                parent_trap_cx[TrapFrameArgs::SP],
                parent_trap_cx[TrapFrameArgs::RET],
                trap_cx.era,
                trap_cx[TrapFrameArgs::SP],
                trap_cx[TrapFrameArgs::RET],
                task.kstack.get_top(),
            );
            drop(task_inner);
            if (_flags & CLONE_PARENT) == 0 {
                self.inner_exclusive_access()
                    .children
                    .push(Arc::clone(&child));
            }
            if let Some(gp) = grandparent_opt.as_ref() {
                gp.inner_exclusive_access()
                    .children
                    .push(Arc::clone(&child));
            }
            insert_into_tid2task(child.getpid(), Arc::clone(&task));
            insert_into_pid2process(child.getpid(), Arc::clone(&child));

            // CLONE_PARENT_SETTID：在父进程中写入 ptid
            // Linux deliberately ignores put_user() failure for parent_tid;
            // the task has already been created and must not be rolled back.
            if _ptid != 0 && (_flags & CLONE_PARENT_SETTID) != 0 {
                let token = crate::task::current_user_token();
                if let Err(err) = Self::write_tid_to_user(token, _ptid, child.getpid()) {
                    warn!(
                        "[clone] parent_tid write failed: mode=fork flags={:#x} ptid={:#x} ctid={:#x} tls={:#x} pid={} err={:?}",
                        _flags,
                        _ptid,
                        _ctid,
                        _tls,
                        child.getpid(),
                        err
                    );
                    error!(
                        "[CLONE_TID_STORE_IGNORED] mode=fork kind=parent flags={:#x} ptr={:#x} pid={} err={:?}",
                        _flags,
                        _ptid,
                        child.getpid(),
                        err
                    );
                }
            }

            // CLONE_CHILD_SETTID：在子进程中写入 ctid
            // The store is best-effort in Linux schedule_tail(). Since this
            // child is not runnable yet, doing it here preserves the ordering
            // guarantee without turning a bad pointer into clone failure.
            if _ctid != 0 && (_flags & CLONE_CHILD_SETTID) != 0 {
                let err = {
                    let child_vm_handle = child.vm_handle();
                    let mut child_vm = child_vm_handle.lock();
                    Self::write_tid_to_vm_set(&mut child_vm, _ctid, child.getpid()).err()
                };
                if let Some(err) = err {
                    warn!(
                        "[clone] child_tid write failed: mode=fork flags={:#x} ptid={:#x} ctid={:#x} tls={:#x} pid={} err={:?}",
                        _flags,
                        _ptid,
                        _ctid,
                        _tls,
                        child.getpid(),
                        err
                    );
                    error!(
                        "[CLONE_TID_STORE_IGNORED] mode=fork kind=child flags={:#x} ptr={:#x} pid={} err={:?}",
                        _flags,
                        _ctid,
                        child.getpid(),
                        err
                    );
                }
            }

            // Publish the completion predicate before the vfork child becomes
            // runnable.  The parent-side wait checks this field under the same
            // task lock it uses to transition to Blocked.
            if (_flags & CLONE_VFORK) != 0 {
                let caller_task = crate::task::current_task().unwrap();
                let mut caller_inner = caller_task.inner_exclusive_access();
                if let Some(active_child) = caller_inner.vfork_child_pid {
                    error!(
                        "[VFORK_STATE_INVARIANT] parent_pid={} parent_tid={} active_child={} new_child={}",
                        self.getpid(),
                        caller_inner.global_tid,
                        active_child,
                        child.getpid(),
                    );
                }
                caller_inner.vfork_child_pid = Some(child.getpid());
            }

            {
                let mut task_inner = task.inner_exclusive_access();
                task_inner.task_status = TaskStatus::Ready;
            }
            // add_task(Arc::clone(&task));
            #[cfg(target_arch = "loongarch64")]
            warn!(
                "[la64 fork] queued child: parent_pid={} child_pid={} ready_queued={} on_cpu={}",
                self.getpid(),
                child.getpid(),
                task.is_ready_queued(),
                task.is_on_cpu(),
            );
            enqueue_new_clone_task(task);
            fork_trace.phase(6);
            warn!(
                "fork a new process with pid {}, parent pid = {}",
                child.getpid(),
                self.getpid()
            );
            child.getpid() as isize
        }
    }
}

pub const CLONE_VM: u32 = 0x00000100; // 共享内存描述符
pub const CLONE_FS: u32 = 0x00000200; // 共享文件系统信息
pub const CLONE_FILES: u32 = 0x00000400; // 共享文件描述符表
pub const CLONE_SIGHAND: u32 = 0x00000800; // 共享信号处理函数表
pub const CLONE_PARENT: u32 = 0x00008000; // 子进程与调用者共享父进程
pub const CLONE_PARENT_SETTID: u32 = 0x00100000; // 父进程设置 tid
pub const CLONE_CHILD_SETTID: u32 = 0x01000000; // 子进程设置 ctid
pub const CLONE_CHILD_CLEARTID: u32 = 0x00200000; // 子进程退出时清零 tid
pub const CLONE_SETTLS: u32 = 0x00080000; // 设置 TLS
pub const CLONE_THREAD: u32 = 0x00010000; // 创建线程（同一线程组）
pub const CLONE_NEWNS: u32 = 0x00020000; // 新的挂载命名空间
pub const CLONE_NEWNET: u32 = 0x40000000; // 新的网络命名空间
pub const CLONE_VFORK: u32 = 0x00004000; // 父进程挂起直到子进程退出或exec
pub const CLONE_INTO_CGROUP: u64 = 0x200000000; // 将新进程放入指定 cgroup
pub const CLONE_PIDFD: u32 = 0x00001000; // 返回 pidfd
pub const CLONE_NEWPID: u32 = 0x20000000; // 新的 PID 命名空间

pub const CLONE_THREAD_FLAGS: u32 =
    CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD;
