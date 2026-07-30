use super::manager::TaskStateStats;
use super::task::UserContextSnapshot;
use super::task_entry;
use super::{ProcessControlBlock, TaskControlBlock};
use super::{TaskStatus, fetch_task};
use crate::config::MAX_CPU_NUM;
#[cfg(target_arch = "riscv64")]
use crate::sbi::*;
use crate::set_init_completed;
use crate::sync::{IrqGuard, SpinNoIrqLock};
use crate::task::check_timers;
use crate::wait_for_init;
use alloc::sync::Arc;
#[cfg(target_arch = "loongarch64")]
use core::arch::asm;
use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::panic::Location;
use core::sync::atomic::{AtomicUsize, Ordering};
use lazy_static::*;
use log::{debug, error, info, warn};
use polyhal::VirtAddr;
use polyhal::consts::KERNEL_STACK_SIZE;
use polyhal::irq::IRQ;
use polyhal::kcontext::{KContext, context_switch};
use polyhal_trap::trapframe::{TrapFrame, TrapFrameArgs};

#[cfg(target_arch = "loongarch64")]
use crate::sbi_la::*;

pub struct Processor {
    current: Option<Arc<TaskControlBlock>>,
    idle_task_cx: KContext,
}

#[derive(Debug, Clone, Copy)]
pub struct ProcessorTaskStats {
    pub current_tasks: usize,
    pub locked_processors: usize,
    /// Lock-free TCB identities used by lwext4 C-layer owner fields.
    pub current_task_owners: [usize; MAX_CPU_NUM],
    pub current_samples: [Option<(usize, Option<usize>, UserContextSnapshot)>; MAX_CPU_NUM],
    pub idle_contexts: [Option<(usize, usize)>; MAX_CPU_NUM],
    pub scheduler_phases: [usize; MAX_CPU_NUM],
    pub scheduler_pids: [usize; MAX_CPU_NUM],
    pub scheduler_irq_enabled: [bool; MAX_CPU_NUM],
    pub scheduler_sps: [usize; MAX_CPU_NUM],
    pub scheduler_ras: [usize; MAX_CPU_NUM],
    pub scheduler_stack_cpus: [usize; MAX_CPU_NUM],
}
impl Processor {
    pub fn new() -> Self {
        Self {
            current: None,
            idle_task_cx: KContext::blank(),
        }
    }
    fn get_idle_task_cx_ptr(&mut self) -> *mut KContext {
        &mut self.idle_task_cx as *mut _
    }
    pub fn take_current(&mut self, cpu: usize) -> Option<Arc<TaskControlBlock>> {
        let current = self.current.take();
        publish_current_task_owner(cpu, None);
        current
    }
    pub fn current(&self) -> Option<Arc<TaskControlBlock>> {
        self.current.as_ref().map(Arc::clone)
    }
}

pub static mut PROCESSORS: [Option<SpinNoIrqLock<Processor>>; MAX_CPU_NUM] =
    [const { None }; MAX_CPU_NUM];
#[cfg(target_arch = "loongarch64")]
static LA64_SCHED_DEBUG_COUNT: AtomicUsize = AtomicUsize::new(0);
#[cfg(target_arch = "loongarch64")]
static LA64_PID2_SCHED_DEBUG_COUNT: AtomicUsize = AtomicUsize::new(0);
#[cfg(target_arch = "loongarch64")]
static LA64_SKIP_DEBUG_COUNT: AtomicUsize = AtomicUsize::new(0);

static IDLE_SPINS: [AtomicUsize; MAX_CPU_NUM] = [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static CURRENT_TASK_OWNERS: [AtomicUsize; MAX_CPU_NUM] =
    [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static CURRENT_TASK_SYSCALLS: [AtomicUsize; MAX_CPU_NUM] =
    [const { AtomicUsize::new(usize::MAX) }; MAX_CPU_NUM];
static CURRENT_TASK_SYSCALL_STAGES: [AtomicUsize; MAX_CPU_NUM] =
    [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static IDLE_TIME_NS: [AtomicUsize; MAX_CPU_NUM] = [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static STALL_DUMP_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Aggregate time spent by all CPUs in the scheduler's idle wait state.
pub fn total_idle_time_ns() -> usize {
    IDLE_TIME_NS.iter().fold(0usize, |total, value| {
        total.saturating_add(value.load(Ordering::Relaxed))
    })
}
/// Assembly-visible progress slots used by the final user-return path. The
/// RISC-V restore routine cannot call Rust after it has restored user
/// registers, so it writes the same per-CPU phase table directly.
#[unsafe(no_mangle)]
pub static __KAIRIX_SCHEDULER_PHASES: [AtomicUsize; MAX_CPU_NUM] =
    [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static SCHEDULER_PIDS: [AtomicUsize; MAX_CPU_NUM] =
    [const { AtomicUsize::new(usize::MAX) }; MAX_CPU_NUM];
static SCHEDULER_IRQ_ENABLED: [AtomicUsize; MAX_CPU_NUM] =
    [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static SCHEDULER_SPS: [AtomicUsize; MAX_CPU_NUM] = [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static SCHEDULER_RAS: [AtomicUsize; MAX_CPU_NUM] = [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static SCHEDULER_PROGRESS_SEQUENCES: [AtomicUsize; MAX_CPU_NUM] =
    [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static SCHEDULER_STACK_CPUS: [AtomicUsize; MAX_CPU_NUM] =
    [const { AtomicUsize::new(usize::MAX) }; MAX_CPU_NUM];
static SCHEDULER_HEARTBEATS_NS: [AtomicUsize; MAX_CPU_NUM] =
    [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static IO_PROGRESS_TOTAL: AtomicUsize = AtomicUsize::new(0);
static IO_PROGRESS_LAST_NS: AtomicUsize = AtomicUsize::new(0);
static IO_PROGRESS_LAST_DUMP_NS: AtomicUsize = AtomicUsize::new(0);
static IO_PROGRESS_DUMP_SEQUENCE: AtomicUsize = AtomicUsize::new(0);
static TASK_STATE_BUFFER_BUSY_REPORTS: AtomicUsize = AtomicUsize::new(0);

fn publish_current_task_owner(cpu: usize, task: Option<&Arc<TaskControlBlock>>) {
    if cpu >= MAX_CPU_NUM {
        return;
    }
    let owner = task.map_or(0, |task| Arc::as_ptr(task) as usize);
    let syscall = task
        .and_then(|task| task.active_syscall())
        .unwrap_or(usize::MAX);
    let syscall_stage = task.map_or(0, |task| task.active_syscall_stage());
    CURRENT_TASK_SYSCALLS[cpu].store(syscall, Ordering::Relaxed);
    CURRENT_TASK_SYSCALL_STAGES[cpu].store(syscall_stage, Ordering::Relaxed);
    CURRENT_TASK_OWNERS[cpu].store(owner, Ordering::Release);
}

/// Refresh the current CPU's lock-free syscall publication if `task` is the
/// task actually executing there. This avoids taking PROCESSORS from a remote
/// timer-stall observer.
pub(crate) fn publish_current_syscall_nolock(
    task: *const TaskControlBlock,
    syscall: Option<usize>,
    stage: usize,
) {
    let cpu = get_tp();
    if cpu >= MAX_CPU_NUM || CURRENT_TASK_OWNERS[cpu].load(Ordering::Acquire) != task as usize {
        return;
    }
    CURRENT_TASK_SYSCALLS[cpu].store(syscall.unwrap_or(usize::MAX), Ordering::Relaxed);
    CURRENT_TASK_SYSCALL_STAGES[cpu].store(stage, Ordering::Release);
}

/// Current syscall and syscall-specific stage last published by one CPU.
pub(crate) fn scheduler_syscall_progress(cpu: usize) -> (Option<usize>, usize) {
    if cpu >= MAX_CPU_NUM {
        return (None, 0);
    }
    let syscall = CURRENT_TASK_SYSCALLS[cpu].load(Ordering::Acquire);
    (
        (syscall != usize::MAX).then_some(syscall),
        CURRENT_TASK_SYSCALL_STAGES[cpu].load(Ordering::Acquire),
    )
}

/// Return a stable lock owner without acquiring the per-CPU processor lock.
///
/// Filesystem lock waits may cooperatively switch tasks on the same CPU. The
/// published TCB address distinguishes those tasks, while the idle fallback
/// remains unique across CPUs for mount lifecycle work outside task context.
pub fn current_task_owner_nolock() -> usize {
    let cpu = get_tp();
    if cpu >= MAX_CPU_NUM {
        return usize::MAX;
    }
    let owner = CURRENT_TASK_OWNERS[cpu].load(Ordering::Acquire);
    if owner == 0 {
        usize::MAX.wrapping_sub(cpu)
    } else {
        owner
    }
}

/// Whether this CPU currently publishes a task, without taking processor lock.
pub fn has_current_task_nolock() -> bool {
    let cpu = get_tp();
    cpu < MAX_CPU_NUM && CURRENT_TASK_OWNERS[cpu].load(Ordering::Acquire) != 0
}

/// Per-CPU diagnostic storage that never performs an atomic read-modify-write.
///
/// Every caller indexes this array with the current CPU and holds `IrqGuard`
/// for the lifetime of the mutable borrow. Consequently there can be only one
/// writer to a slot: another CPU uses another slot, and an interrupt cannot
/// re-enter diagnostics on the owning CPU. The atomics below are only
/// lock-free observability for an unexpected synchronous recursion.
struct TaskStateStatsBuffer {
    in_use: AtomicUsize,
    owner_hart: AtomicUsize,
    owner_line: AtomicUsize,
    stats: UnsafeCell<TaskStateStats>,
}

struct TaskStateStatsGuard<'a> {
    buffer: &'a TaskStateStatsBuffer,
    _irq_guard: IrqGuard,
}

unsafe impl Sync for TaskStateStatsBuffer {}

impl TaskStateStatsBuffer {
    const fn new() -> Self {
        Self {
            in_use: AtomicUsize::new(0),
            owner_hart: AtomicUsize::new(usize::MAX),
            owner_line: AtomicUsize::new(0),
            stats: UnsafeCell::new(TaskStateStats::empty()),
        }
    }

    #[track_caller]
    fn try_lock(&self) -> Option<TaskStateStatsGuard<'_>> {
        let irq_guard = IrqGuard::new();
        if self.in_use.load(Ordering::Acquire) != 0 {
            return None;
        }
        let caller = Location::caller();
        self.owner_hart
            .store(polyhal::arch::hart_id(), Ordering::Relaxed);
        self.owner_line
            .store(caller.line() as usize, Ordering::Relaxed);
        self.in_use.store(1, Ordering::Release);
        Some(TaskStateStatsGuard {
            buffer: self,
            _irq_guard: irq_guard,
        })
    }

    fn owner_hart(&self) -> usize {
        self.owner_hart.load(Ordering::Acquire)
    }

    fn owner_line(&self) -> usize {
        self.owner_line.load(Ordering::Acquire)
    }
}

impl Deref for TaskStateStatsGuard<'_> {
    type Target = TaskStateStats;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.buffer.stats.get() }
    }
}

impl DerefMut for TaskStateStatsGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.buffer.stats.get() }
    }
}

impl Drop for TaskStateStatsGuard<'_> {
    fn drop(&mut self) {
        self.buffer.owner_hart.store(usize::MAX, Ordering::Relaxed);
        self.buffer.owner_line.store(0, Ordering::Relaxed);
        self.buffer.in_use.store(0, Ordering::Release);
    }
}

// Runtime snapshots execute on each CPU's idle/boot stack. TaskStateStats is
// deliberately large enough to retain several task/context samples, so keep
// one protected buffer per CPU instead of repeatedly materializing it on that
// stack. A CPU cannot concurrently enter this path twice in the current
// non-preemptible kernel; TaskStateStatsBuffer keeps that invariant explicit
// without putting a spinlock RMW in the scheduler path.
static TASK_STATE_STATS_BUFFERS: [TaskStateStatsBuffer; MAX_CPU_NUM] =
    [const { TaskStateStatsBuffer::new() }; MAX_CPU_NUM];

const IO_PROGRESS_STALL_NS: usize = 5_000_000_000;

/// Syscalls that can legitimately remain active while the whole system is
/// quiescent.  They must not turn a normal idle interval into a scheduler
/// stall dump; an overdue timer is checked separately below.
fn is_waiting_syscall(syscall_id: usize) -> bool {
    matches!(syscall_id, 22 | 63 | 72 | 73 | 98 | 101 | 115 | 260)
}

/// A queue owner that has not returned to its scheduler loop for this long may
/// still run its current task, but other CPUs must be allowed to rescue its queued work.
pub(crate) const SCHEDULER_STALL_NS: usize = 100_000_000;

pub(crate) fn scheduler_heartbeat_ns(cpu: usize) -> usize {
    SCHEDULER_HEARTBEATS_NS
        .get(cpu)
        .map(|heartbeat| heartbeat.load(Ordering::Acquire))
        .unwrap_or(0)
}

pub(crate) fn scheduler_cpu_stalled(cpu: usize, now_ns: usize) -> bool {
    let heartbeat = scheduler_heartbeat_ns(cpu);
    heartbeat != 0 && now_ns.saturating_sub(heartbeat) >= SCHEDULER_STALL_NS
}

/// Return the latest lock-free scheduler marker for interrupt-side diagnosis.
pub(crate) fn scheduler_progress(
    cpu: usize,
) -> (usize, usize, usize, bool, usize, usize, usize, usize) {
    if cpu >= MAX_CPU_NUM {
        return (0, 0, usize::MAX, false, 0, usize::MAX, 0, 0);
    }
    (
        SCHEDULER_HEARTBEATS_NS[cpu].load(Ordering::Acquire),
        __KAIRIX_SCHEDULER_PHASES[cpu].load(Ordering::Acquire),
        SCHEDULER_PIDS[cpu].load(Ordering::Relaxed),
        SCHEDULER_IRQ_ENABLED[cpu].load(Ordering::Acquire) != 0,
        SCHEDULER_SPS[cpu].load(Ordering::Acquire),
        SCHEDULER_STACK_CPUS[cpu].load(Ordering::Acquire),
        SCHEDULER_PROGRESS_SEQUENCES[cpu].load(Ordering::Acquire),
        SCHEDULER_RAS[cpu].load(Ordering::Acquire),
    )
}

fn record_scheduler_heartbeat(cpu: usize) {
    if cpu < MAX_CPU_NUM {
        let now_ns = polyhal::timer::current_time().as_nanos() as usize;
        SCHEDULER_HEARTBEATS_NS[cpu].store(now_ns, Ordering::Release);
    }
}

#[inline(always)]
fn current_stack_pointer() -> usize {
    let sp: usize;
    unsafe {
        #[cfg(target_arch = "riscv64")]
        core::arch::asm!("mv {}, sp", out(reg) sp, options(nomem, nostack, preserves_flags));
        #[cfg(target_arch = "loongarch64")]
        core::arch::asm!("move {}, $sp", out(reg) sp, options(nomem, nostack, preserves_flags));
    }
    sp
}

#[inline(always)]
fn current_return_address() -> usize {
    let ra: usize;
    unsafe {
        #[cfg(target_arch = "riscv64")]
        core::arch::asm!("mv {}, ra", out(reg) ra, options(nomem, nostack, preserves_flags));
        #[cfg(target_arch = "loongarch64")]
        core::arch::asm!("move {}, $ra", out(reg) ra, options(nomem, nostack, preserves_flags));
    }
    ra
}

/// Infer the physical scheduler stack owner without relying on RISC-V `tp`.
///
/// The RISC-V entry code assigns a fixed slice of BOOT_STACK to each hart, so
/// the current SP is an independent check of the CPU identity kept in `tp`.
/// LoongArch secondary stacks are allocated dynamically by polyhal-boot and do
/// not currently have a corresponding address-to-CPU mapping.
#[inline(always)]
fn scheduler_stack_cpu(_sp: usize) -> usize {
    #[cfg(target_arch = "riscv64")]
    {
        use crate::arch::riscv_dir::{BOOT_STACK, BOOT_STACK_SIZE};

        let base = core::ptr::addr_of!(BOOT_STACK) as usize;
        let size = BOOT_STACK_SIZE.saturating_mul(MAX_CPU_NUM);
        if _sp > base && _sp <= base.saturating_add(size) {
            return (_sp - base - 1) / BOOT_STACK_SIZE;
        }
    }
    usize::MAX
}

#[inline(always)]
fn record_scheduler_stack(cpu: usize) {
    if cpu >= MAX_CPU_NUM {
        return;
    }
    let sp = current_stack_pointer();
    SCHEDULER_SPS[cpu].store(sp, Ordering::Release);
    SCHEDULER_STACK_CPUS[cpu].store(scheduler_stack_cpu(sp), Ordering::Release);
}

fn report_task_state_buffer_busy(cpu: usize, site: &'static str) {
    if cpu >= MAX_CPU_NUM || TASK_STATE_BUFFER_BUSY_REPORTS.fetch_add(1, Ordering::Relaxed) >= 32 {
        return;
    }
    let buffer = &TASK_STATE_STATS_BUFFERS[cpu];
    log::error!(
        "[SCHED_DIAG_BUFFER_BUSY_VISIBLE] cpu={} site={} owner_hart={} owner_line={}",
        cpu,
        site,
        buffer.owner_hart(),
        buffer.owner_line(),
    );
    warn!(
        "[SCHED_DIAG_BUFFER_BUSY] cpu={} site={} owner_hart={} owner_line={}",
        cpu,
        site,
        buffer.owner_hart(),
        buffer.owner_line(),
    );
}

#[inline(always)]
fn validate_scheduler_identity(expected_cpu: usize, boundary: usize) {
    let reported_cpu = get_tp();
    let sp = current_stack_pointer();
    let stack_cpu = scheduler_stack_cpu(sp);
    if reported_cpu != expected_cpu || (stack_cpu != usize::MAX && stack_cpu != expected_cpu) {
        panic!(
            "[SCHED_IDENTITY_CORRUPTION] boundary={} expected_cpu={} reported_cpu={} stack_cpu={} sp={:#x}",
            boundary, expected_cpu, reported_cpu, stack_cpu, sp,
        );
    }
}

/// Record a lock-free scheduler progress marker for cross-CPU stall dumps.
#[inline(never)]
pub(crate) fn record_scheduler_phase(phase: usize, task: Option<&Arc<TaskControlBlock>>) {
    let cpu = get_tp();
    if cpu >= MAX_CPU_NUM {
        return;
    }
    record_scheduler_stack(cpu);
    SCHEDULER_IRQ_ENABLED[cpu].store(IRQ::int_enabled() as usize, Ordering::Release);
    // Publish the boundary before optional metadata. A stalled diagnostic
    // helper must never leave observers seeing the preceding phase.
    __KAIRIX_SCHEDULER_PHASES[cpu].store(phase, Ordering::Release);
    SCHEDULER_RAS[cpu].store(current_return_address(), Ordering::Release);
    let pid = task.map(|task| task.process_id()).unwrap_or(usize::MAX);
    SCHEDULER_PIDS[cpu].store(pid, Ordering::Relaxed);
    // Increment only after the accompanying phase metadata is visible. A
    // remote stall observer can compare this sequence across reports to tell
    // a fixed instruction stall from a scheduler loop that is still moving.
    SCHEDULER_PROGRESS_SEQUENCES[cpu].fetch_add(1, Ordering::Release);
}

fn print_runtime_snapshot(tag: &str, cpu: usize, sequence: usize) {
    record_scheduler_phase(30, None);
    let processors = processor_task_stats();
    record_scheduler_phase(31, None);
    let load_balance = crate::task::manager::load_balance_stats();
    if load_balance.stalled_mask != 0 {
        log::error!(
            "[SCHEDULER_CPU_STALLED_VISIBLE] observer_cpu={} stalled_mask={:#x} heartbeats_ns={:?} phases={:?} scheduler_sps={:?} scheduler_ras={:?} scheduler_stack_cpus={:?} idle_contexts={:?}",
            get_tp(),
            load_balance.stalled_mask,
            load_balance.scheduler_heartbeats_ns,
            processors.scheduler_phases,
            processors.scheduler_sps,
            processors.scheduler_ras,
            processors.scheduler_stack_cpus,
            processors.idle_contexts,
        );
        warn!(
            "[SCHEDULER_CPU_STALLED] observer_cpu={} stalled_mask={:#x} heartbeats_ns={:?} phases={:?} scheduler_sps={:?} scheduler_stack_cpus={:?} idle_contexts={:?}",
            get_tp(),
            load_balance.stalled_mask,
            load_balance.scheduler_heartbeats_ns,
            processors.scheduler_phases,
            processors.scheduler_sps,
            processors.scheduler_stack_cpus,
            processors.idle_contexts,
        );
    }
    record_scheduler_phase(32, None);
    let buffer_cpu = get_tp();
    let Some(mut task_states) = TASK_STATE_STATS_BUFFERS[buffer_cpu].try_lock() else {
        report_task_state_buffer_busy(buffer_cpu, "runtime_snapshot");
        record_scheduler_phase(38, None);
        return;
    };
    crate::task::manager::fill_task_state_stats(&mut task_states);
    record_scheduler_phase(33, None);
    let page_cache = crate::fs::page::pagecache::atomic_stats();
    let page_cache_lock = crate::fs::page::pagecache::PAGE_CACHE.stats();
    let lwext4_lock = crate::fs::lwext4::lwext4_lock_stats();
    let lwext4_c = crate::fs::lwext4::lwext4_c_progress();
    let ext4_flush = crate::fs::lwext4::file::ext4_flush_stats();
    let block_io = crate::drivers::block::virtio_blk::virtio_block_io_stats();
    let writeback_pending = crate::fs::writeback::try_pending_count();
    let io_activity = crate::syscall::io_activity_stats();
    let timers = crate::task::timer_queue_stats();
    let frame_allocator = &crate::mm::frame_allocator::FRAME_ALLOCATOR;
    record_scheduler_phase(34, None);
    for mount in &lwext4_lock.mounts {
        let Some(stage3) = mount.stage3.as_ref() else {
            continue;
        };
        if stage3.active_transactions == 0
            && stage3.active_inode_readers == 0
            && stage3.active_inode_writers == 0
            && stage3.inode_sample_count == 0
        {
            continue;
        }
        log::error!(
            "[LWEXT4_STAGE3_STALL] cpu={} sequence={} mount_id={} mount={} current_task_owners={:#x?} active_transactions={} transaction_sample_count={} transaction_samples_truncated={} transaction_owners={:#x?} transaction_ptrs={:#x?} transaction_depths={:?} active_inode_readers={} active_inode_writers={} inode_sample_count={} inode_samples_truncated={} inode_shards={:?} inode_states={:#x?} inode_writer_owners={:#x?} inode_writer_depths={:?} inode_writer_inodes={:?} inode_reader_waiters={:#x?} inode_reader_wait_inodes={:?} inode_waiting_readers={:?} inode_writer_waiters={:#x?} inode_writer_wait_inodes={:?} inode_waiting_writers={:?}",
            cpu,
            sequence,
            mount.mount_id,
            mount.mount_point,
            processors.current_task_owners,
            stage3.active_transactions,
            stage3.transaction_sample_count,
            stage3.transaction_samples_truncated,
            stage3.transaction_owners,
            stage3.transaction_ptrs,
            stage3.transaction_depths,
            stage3.active_inode_readers,
            stage3.active_inode_writers,
            stage3.inode_sample_count,
            stage3.inode_samples_truncated,
            stage3.inode_shards,
            stage3.inode_states,
            stage3.inode_writer_owners,
            stage3.inode_writer_depths,
            stage3.inode_writer_inodes,
            stage3.inode_reader_waiters,
            stage3.inode_reader_wait_inodes,
            stage3.inode_waiting_readers,
            stage3.inode_writer_waiters,
            stage3.inode_writer_wait_inodes,
            stage3.inode_waiting_writers,
        );
    }
    log::error!(
        "[{}] cpu={} sequence={} processors={:?} load_balance={:?} task_states={:?} timers={:?} io_activity={{reads:{},writes:{},preads:{},pwrites:{},fsyncs:{}}} page_cache={:?} page_cache_lock={:?} lwext4_lock={:?} lwext4_c={:?} ext4_flush={:?} block_io={:?} frame_allocator_lock={{locked:{},owner_hart:{},owner_line:{}}} writeback_pending={:?}",
        tag,
        cpu,
        sequence,
        processors,
        load_balance,
        &*task_states,
        timers,
        io_activity.reads,
        io_activity.writes,
        io_activity.preads,
        io_activity.pwrites,
        io_activity.fsyncs,
        page_cache,
        page_cache_lock,
        lwext4_lock,
        lwext4_c,
        ext4_flush,
        block_io,
        frame_allocator.is_locked(),
        frame_allocator.owner_hart(),
        frame_allocator.owner_line(),
        writeback_pending,
    );
    record_scheduler_phase(35, None);
}

fn print_fork_clone_snapshot() {
    let clone = crate::task::process::fork_clone_stats();
    if !clone.active {
        return;
    }
    let cow = crate::mm::vm_set::fork_cow_stats();
    let kernel_vmset = &crate::mm::KERNEL_VMSET;
    log::error!(
        "[FORK_CLONE_STATE] clone={:?} cow={:?} kernel_vmset_locked={} kernel_vmset_owner={} kernel_vmset_owner_line={} frame_allocator={:?}",
        clone,
        cow,
        kernel_vmset.is_locked(),
        kernel_vmset.owner_hart(),
        kernel_vmset.owner_line(),
        crate::mm::try_frame_stats(),
    );
}

/// Emit a scheduler-side snapshot when a concurrent workload remains in
/// syscalls but no read/write/fsync entry has advanced for several seconds.
/// Unlike the pselect and idle watchdogs, this also covers a workload whose
/// coordination loops keep every CPU runnable.
fn check_io_progress_watchdog(cpu: usize) {
    let now_ns = polyhal::timer::current_time().as_nanos() as usize;
    let io = crate::syscall::io_activity_stats();
    let total = io
        .reads
        .wrapping_add(io.writes)
        .wrapping_add(io.preads)
        .wrapping_add(io.pwrites)
        .wrapping_add(io.fsyncs);
    let previous = IO_PROGRESS_TOTAL.swap(total, Ordering::AcqRel);
    if total != previous {
        IO_PROGRESS_LAST_NS.store(now_ns, Ordering::Release);
        return;
    }

    let last_progress_ns = IO_PROGRESS_LAST_NS.load(Ordering::Acquire);
    if last_progress_ns == 0 {
        let _ =
            IO_PROGRESS_LAST_NS.compare_exchange(0, now_ns, Ordering::AcqRel, Ordering::Acquire);
        return;
    }
    if now_ns.saturating_sub(last_progress_ns) < IO_PROGRESS_STALL_NS {
        return;
    }

    let last_dump_ns = IO_PROGRESS_LAST_DUMP_NS.load(Ordering::Acquire);
    if now_ns.saturating_sub(last_dump_ns) < IO_PROGRESS_STALL_NS {
        return;
    }
    let should_dump = {
        let buffer_cpu = get_tp();
        let Some(mut states) = TASK_STATE_STATS_BUFFERS[buffer_cpu].try_lock() else {
            report_task_state_buffer_busy(buffer_cpu, "io_progress_watchdog");
            return;
        };
        crate::task::manager::fill_task_state_stats(&mut states);
        let has_non_waiting_syscall = states
            .active_samples
            .iter()
            .flatten()
            .any(|(_, syscall_id, _)| !is_waiting_syscall(*syscall_id));
        !states.process_table_busy
            && states.total > 3
            && states.active_syscalls >= 4
            && has_non_waiting_syscall
    };
    if !should_dump {
        return;
    }
    if IO_PROGRESS_LAST_DUMP_NS
        .compare_exchange(last_dump_ns, now_ns, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }

    let sequence = IO_PROGRESS_DUMP_SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1;
    print_runtime_snapshot("IO_PROGRESS_STALL", cpu, sequence);
    print_fork_clone_snapshot();
}

fn dump_stall_snapshot(cpu: usize, idle_spins: usize) {
    record_scheduler_phase(26, None);
    let (has_non_waiting_syscall, has_execve, state_is_quiescent) = {
        let Some(mut task_states) = TASK_STATE_STATS_BUFFERS[cpu].try_lock() else {
            report_task_state_buffer_busy(cpu, "idle_stall_snapshot");
            record_scheduler_phase(28, None);
            return;
        };
        crate::task::manager::fill_task_state_stats(&mut task_states);
        let has_non_waiting_syscall = task_states
            .active_samples
            .iter()
            .flatten()
            .any(|(_, syscall_id, _)| !is_waiting_syscall(*syscall_id));
        let has_execve = task_states
            .active_samples
            .iter()
            .flatten()
            .any(|(_, syscall_id, _)| *syscall_id == 221);
        let state_is_quiescent = !task_states.process_table_busy
            && task_states.process_locks_busy == 0
            && task_states.task_locks_busy == 0
            && task_states.ready == 0
            && task_states.running == 0;
        (has_non_waiting_syscall, has_execve, state_is_quiescent)
    };
    record_scheduler_phase(27, None);
    // A normal clock_nanosleep interval has no runnable tasks and no overdue
    // timer.  The previous `total <= 3` heuristic misclassified the fourth
    // (sleeping) shell/iozone coordinator as a kernel stall and sent every idle
    // CPU through the heavyweight diagnostic path at the same time.
    let timers = crate::task::timer_queue_stats();
    let is_quiescent = state_is_quiescent
        && !has_non_waiting_syscall
        && !timers.lock_busy
        && timers.overdue_tasks == 0;
    if is_quiescent {
        return;
    }
    if STALL_DUMP_COUNT.fetch_add(1, Ordering::Relaxed) >= 16 {
        return;
    }
    record_scheduler_phase(29, None);
    print_runtime_snapshot("KERNEL_STALL", cpu, idle_spins);
    record_scheduler_phase(36, None);
    print_fork_clone_snapshot();
    record_scheduler_phase(37, None);
    if has_execve {
        log::error!(
            "[EXECVE_STALL] cpu={} frame_allocator={:?}",
            cpu,
            crate::mm::try_frame_stats()
        );
    }
}

/// Print a lock-free-biased snapshot when a zero-fd pselect poll persists.
pub(crate) fn dump_pselect_stall_snapshot(sequence: usize) {
    print_runtime_snapshot("PSELECT_STALL", get_tp(), sequence);
    print_fork_clone_snapshot();
}

/// Continue low-frequency snapshots after the initial pselect dump budget.
pub(crate) fn dump_pselect_long_stall_snapshot(sequence: usize) {
    print_runtime_snapshot("PSELECT_LONG_STALL", get_tp(), sequence);
    print_fork_clone_snapshot();
}

pub fn init_processors() {
    unsafe {
        for i in 0..MAX_CPU_NUM {
            PROCESSORS[i] = Some(SpinNoIrqLock::new(Processor::new()));
        }
    }
}

pub(crate) fn processor_task_stats() -> ProcessorTaskStats {
    let mut current_tasks = 0usize;
    let mut locked_processors = 0usize;
    let mut current_samples = [None; MAX_CPU_NUM];
    let mut idle_contexts = [None; MAX_CPU_NUM];
    unsafe {
        for cpu in 0..MAX_CPU_NUM {
            if let Some(processor) = PROCESSORS[cpu].as_ref() {
                if let Some(processor) = processor.try_lock() {
                    idle_contexts[cpu] =
                        Some((processor.idle_task_cx.sp(), processor.idle_task_cx.ra()));
                    if let Some(task) = processor.current.as_ref() {
                        current_tasks += 1;
                        let pid = task
                            .process
                            .upgrade()
                            .map(|process| process.getpid())
                            .unwrap_or(usize::MAX);
                        current_samples[cpu] =
                            Some((pid, task.active_syscall(), task.user_context_snapshot()));
                    }
                } else {
                    locked_processors += 1;
                }
            }
        }
    }
    ProcessorTaskStats {
        current_tasks,
        locked_processors,
        current_task_owners: core::array::from_fn(|cpu| {
            CURRENT_TASK_OWNERS[cpu].load(Ordering::Acquire)
        }),
        current_samples,
        idle_contexts,
        scheduler_phases: core::array::from_fn(|cpu| {
            __KAIRIX_SCHEDULER_PHASES[cpu].load(Ordering::Acquire)
        }),
        scheduler_pids: core::array::from_fn(|cpu| SCHEDULER_PIDS[cpu].load(Ordering::Relaxed)),
        scheduler_irq_enabled: core::array::from_fn(|cpu| {
            SCHEDULER_IRQ_ENABLED[cpu].load(Ordering::Acquire) != 0
        }),
        scheduler_sps: core::array::from_fn(|cpu| SCHEDULER_SPS[cpu].load(Ordering::Acquire)),
        scheduler_ras: core::array::from_fn(|cpu| SCHEDULER_RAS[cpu].load(Ordering::Acquire)),
        scheduler_stack_cpus: core::array::from_fn(|cpu| {
            SCHEDULER_STACK_CPUS[cpu].load(Ordering::Acquire)
        }),
    }
}
#[allow(missing_docs)]
pub fn run_tasks() {
    let id: usize = get_tp();
    validate_scheduler_identity(id, 0);
    #[cfg(target_arch = "riscv64")]
    crate::syscall::hwprobe::record_current_cpu(id);
    crate::task::manager::mark_cpu_online(id);
    //println!("cpu {} run tasks", id);
    if id == 0 {
        set_init_completed();
        // loop{}
    }
    // Keeping the last process alive lets the scheduler safely execute through
    // its kernel-half mappings until another address space is selected. This
    // avoids a user->kernel->user root switch on every voluntary yield while
    // still preventing the active root frame from being recycled.
    let mut active_user_process = None;
    loop {
        // Kairix currently has a non-preemptible kernel: a timer trap may
        // context-switch a user task, but must not re-enter the idle scheduler
        // at an arbitrary instruction.  schedule() reaches this stack directly
        // from a trap/syscall continuation without an sret, so make the required
        // interrupt state explicit on every iteration.  check_timers() below
        // advances wall-clock sleepers while idle; once a task is restored,
        // sret re-enables delivery according to that task's saved status.
        IRQ::int_disable();
        validate_scheduler_identity(id, 1);
        // This marker does not read the timer. If phase 7 is visible but phase
        // 8 is not, the CPU stopped while sampling the hardware clock for its
        // heartbeat rather than while returning from the previous phase.
        record_scheduler_phase(7, None);
        record_scheduler_heartbeat(id);
        record_scheduler_phase(8, None);
        // Timer traps only account, re-arm, and request a reschedule.  Global
        // timeout scans and cross-CPU diagnostics belong on this idle stack,
        // where they cannot strand a hart inside an IRQ-off trap continuation.
        // The before/after phase pairs below localize a stall without logging
        // on every scheduler iteration: futex=114/115, POSIX timer=116/117,
        // cross-CPU timer diagnosis=118/119.
        record_scheduler_phase(114, None);
        crate::syscall::futex::check_futex_timeouts();
        record_scheduler_phase(115, None);
        record_scheduler_phase(116, None);
        crate::syscall::time::check_posix_timers();
        record_scheduler_phase(117, None);
        record_scheduler_phase(118, None);
        crate::interrupts::diagnose_scheduler_stall_from_timer_interrupt();
        record_scheduler_phase(119, None);
        // Timer wakeups are scheduler correctness work, while watchdogs and
        // deferred destruction are auxiliary maintenance. Service an expired
        // sleeper first so neither diagnostic formatting nor a long resource
        // drop can postpone the only runnable task in the system.
        record_scheduler_phase(10, None);
        check_timers();
        validate_scheduler_identity(id, 2);
        // Keep a distinct return-boundary marker before the following watchdog
        // so a corrupted check_timers return cannot be mistaken for pselect.
        record_scheduler_phase(9, None);
        record_scheduler_phase(12, None);
        // The watchdog only produces diagnostic snapshots.  On the LS2K1000
        // the boot CPU continues to use the boot/idle stack while the other
        // harts are being brought up; walking every PCB/TCB from that stack
        // has been observed to corrupt the interrupted kernel continuation.
        // Keep normal scheduling and timer wakeups independent from this
        // optional reporting path on the board.
        if !cfg!(board = "2k1000") {
            check_io_progress_watchdog(id);
        }
        record_scheduler_phase(1, None);
        crate::task::reap_deferred_exited_tasks();
        record_scheduler_phase(11, None);
        crate::service_deferred_timer_maintenance();
        record_scheduler_phase(112, None);
        crate::net::poll_rx_all();
        record_scheduler_phase(113, None);
        record_scheduler_phase(13, None);
        unsafe {
            record_scheduler_phase(14, None);
            if let Some(task) = fetch_task(id) {
                record_scheduler_phase(2, Some(&task));
                IDLE_SPINS[id].store(0, Ordering::Relaxed);
                STALL_DUMP_COUNT.store(0, Ordering::Relaxed);
                // Clone the task before moving ownership
                //println!("cpu {} enter fetch task", id);
                let task_clone = Arc::clone(&task);
                let should_skip = {
                    let task_inner = task.inner_exclusive_access();
                    task_inner.task_status == TaskStatus::Zombie
                };
                if should_skip {
                    let mut processor = PROCESSORS[id].as_mut().unwrap().lock();
                    processor.current = None;
                    publish_current_task_owner(id, None);
                    task.clear_on_cpu();
                    continue;
                }
                //println!("cpu {} get processor", id);
                let mut processor = PROCESSORS[id].as_mut().unwrap().lock();
                //println!("cpu {} get processor success", id);
                let mut task_inner = task.inner_exclusive_access();
                if task_inner.task_status == TaskStatus::Zombie {
                    drop(task_inner);
                    processor.current = None;
                    publish_current_task_owner(id, None);
                    task.clear_on_cpu();
                    continue;
                }
                if task_inner.task_status != TaskStatus::Ready {
                    #[cfg(target_arch = "loongarch64")]
                    {
                        let pid = task
                            .process
                            .upgrade()
                            .map(|process| process.getpid())
                            .unwrap_or(usize::MAX);
                        if (pid == 1 || pid == 2 || pid == 3)
                            && LA64_SKIP_DEBUG_COUNT.fetch_add(1, Ordering::Relaxed) < 64
                        {
                            warn!(
                                "[la64 sched] skip non-ready: cpu={} pid={} tid={} status={:?} ready_queued={} on_cpu={}",
                                id,
                                pid,
                                task_inner.global_tid,
                                task_inner.task_status,
                                task.is_ready_queued(),
                                task.is_on_cpu(),
                            );
                        }
                    }
                    drop(task_inner);
                    processor.current = None;
                    publish_current_task_owner(id, None);
                    task.clear_on_cpu();
                    continue;
                }
                if !task.is_on_cpu_at(id) {
                    drop(task_inner);
                    processor.current = None;
                    publish_current_task_owner(id, None);
                    continue;
                }
                let idle_task_cx_ptr = processor.get_idle_task_cx_ptr();
                // access coming task TCB exclusively
                let next_task_cx_ptr = &task_inner.task_cx as *const KContext;
                task_inner.task_status = TaskStatus::Running;
                #[cfg(target_arch = "loongarch64")]
                {
                    let n = LA64_SCHED_DEBUG_COUNT.fetch_add(1, Ordering::Relaxed);
                    let pid = task
                        .process
                        .upgrade()
                        .map(|process| process.getpid())
                        .unwrap_or(usize::MAX);
                    if pid == 2 && LA64_PID2_SCHED_DEBUG_COUNT.fetch_add(1, Ordering::Relaxed) < 4 {
                        warn!(
                            "[la64 sched] cpu={} switch#{} pid={} tid={} status=Running era={:#x} sp={:#x} ret={:#x}",
                            id,
                            n,
                            pid,
                            task_inner.global_tid,
                            task_inner.trap_cx.era,
                            task_inner.trap_cx[TrapFrameArgs::SP],
                            task_inner.trap_cx[TrapFrameArgs::RET],
                        );
                    }
                }
                //println!("pid:{}", task.process.upgrade().unwrap().getpid());
                drop(task_inner);
                // release coming task TCB manually
                processor.current = Some(task);
                publish_current_task_owner(id, processor.current.as_ref());
                // release processor manually
                drop(processor);
                record_scheduler_phase(3, Some(&task_clone));
                // Use the cloned task instead of calling current_task() to avoid extra lock acquisition

                let process = match task_clone.process.upgrade() {
                    Some(p) => p,
                    None => {
                        // PCB has been freed (e.g. process killed by signal and reaped by waitpid),
                        // but this orphan task is still in the ready queue. Drop it and continue.
                        let mut processor = PROCESSORS[id].as_mut().unwrap().lock();
                        processor.current = None;
                        publish_current_task_owner(id, None);
                        task_clone.clear_on_cpu();
                        continue;
                    }
                };

                // Page-table activation is the only architecture-specific
                // operation between selecting a task and switching to it. Keep
                // distinct lock-free markers so an interrupt-side stall report
                // can distinguish it from the following context preparation.
                record_scheduler_phase(150, Some(&task_clone));
                let activation_skipped = process.activate_user_page_table();
                crate::task::perf_stats::record_page_table_activation(activation_skipped);
                active_user_process = Some(process.clone());
                record_scheduler_phase(151, Some(&task_clone));

                // `process` is already the PCB associated with task_clone. Do
                // not call current_task() here: it would reacquire this CPU's
                // PROCESSORS lock after we deliberately released it above.
                debug!("cpu {} switch to task {}", id, process.getpid());

                record_scheduler_phase(4, Some(&task_clone));
                context_switch(idle_task_cx_ptr, next_task_cx_ptr);
                record_scheduler_phase(5, Some(&task_clone));
                let (requeue_after_switch, requeue_front_after_switch) = {
                    let mut task_inner = task_clone.inner_exclusive_access();
                    // Serialize the final on-CPU release with wakeup_task(),
                    // which checks on_cpu while holding the same task lock.
                    // Clearing on_cpu before taking this lock leaves a window
                    // where another CPU can enqueue and start this task, after
                    // which this CPU would still consume requeue_after_switch
                    // and change that running task back to Ready.
                    task_clone.clear_on_cpu();
                    let requeue = task_inner.requeue_after_switch;
                    let requeue_front = task_inner.requeue_front_after_switch;
                    if requeue {
                        task_inner.requeue_after_switch = false;
                        task_inner.requeue_front_after_switch = false;
                        task_inner.pending_wakeup = false;
                        if task_inner.task_status != TaskStatus::Zombie {
                            task_inner.task_status = TaskStatus::Ready;
                        }
                    }
                    (requeue, requeue_front)
                };
                if requeue_after_switch {
                    if requeue_front_after_switch {
                        crate::task::add_task_to_cpu_front(task_clone, id);
                    } else {
                        crate::task::add_task_to_cpu(task_clone, id);
                    }
                }
                record_scheduler_phase(6, None);
            } else {
                record_scheduler_phase(1, None);
                if active_user_process.is_some() {
                    record_scheduler_phase(152, None);
                    let activation_skipped = crate::mm::activate_kernel_page_table();
                    crate::task::perf_stats::record_page_table_activation(activation_skipped);
                    active_user_process = None;
                    record_scheduler_phase(153, None);
                }
                let spins = IDLE_SPINS[id].fetch_add(1, Ordering::Relaxed) + 1;
                record_scheduler_phase(20, None);
                #[cfg(not(board = "visionfive2"))]
                if spins == 1 || spins == 1000 || spins % 100_000 == 0 {
                    record_scheduler_phase(21, None);
                    let ready_queues = crate::task::manager::ready_queue_lengths();
                    let writeback_pending =
                        crate::fs::writeback::try_has_pending_writeback().unwrap_or(true);
                    let writeback_queued =
                        crate::fs::writeback::try_pending_count().unwrap_or(usize::MAX);
                    record_scheduler_phase(22, None);
                    warn!(
                        "[IOZONE_HANG sched_idle] cpu={} idle_spins={} ready_queues={:?} writeback_pending={} writeback_queued={}",
                        id, spins, ready_queues, writeback_pending, writeback_queued
                    );
                    record_scheduler_phase(23, None);
                }
                if !cfg!(board = "2k1000") && (spins == 100_000 || spins % 10_000_000 == 0) {
                    record_scheduler_phase(24, None);
                    dump_stall_snapshot(id, spins);
                    record_scheduler_phase(25, None);
                }

                crate::request_timer_maintenance();
                crate::trap::enable_timer_interrupt();
                // A RISC-V one-shot timer is armed at CPU startup and renewed
                // only after a real timer trap.  If that deadline expires while
                // scheduler work has interrupts masked, it must remain pending:
                // re-arming it here would let idle/IPI churn postpone STIP
                // indefinitely.
                // Publish idle only after the empty queue scan, then recheck the
                // lock-free ready count. A remote enqueue either observes this
                // marker and sends an IPI, or is observed here before WFI.
                crate::task::manager::mark_cpu_idle(id);
                if crate::task::manager::cpu_has_ready_tasks(id) {
                    crate::task::manager::mark_cpu_active(id);
                    continue;
                }
                record_scheduler_phase(110, None);
                crate::task::perf_stats::record_idle_wfi();
                let idle_started_ns = polyhal::timer::current_time().as_nanos() as usize;
                IRQ::int_enable();
                // If an already-pending kick was handled as interrupts became
                // enabled, its queue publication is visible now. Avoid entering
                // WFI after consuming the interrupt that was meant to wake us.
                if crate::task::manager::cpu_has_ready_tasks(id) {
                    IRQ::int_disable();
                    crate::task::manager::mark_cpu_active(id);
                    continue;
                }
                polyhal::instruction::wait_for_interrupt();
                IRQ::int_disable();
                crate::task::manager::mark_cpu_active(id);
                let idle_elapsed_ns = (polyhal::timer::current_time().as_nanos() as usize)
                    .saturating_sub(idle_started_ns);
                IDLE_TIME_NS[id].fetch_add(idle_elapsed_ns, Ordering::Relaxed);
                record_scheduler_phase(111, None);
                #[cfg(board = "visionfive2")]
                let _ = spins;
            }
        }
    }
}
#[allow(missing_docs)]
pub fn take_current_task() -> Option<Arc<TaskControlBlock>> {
    let id: usize = get_tp();
    if id >= MAX_CPU_NUM {
        return None;
    }
    unsafe { PROCESSORS[id].as_mut()?.lock().take_current(id) }
}
#[allow(missing_docs)]
pub fn current_task() -> Option<Arc<TaskControlBlock>> {
    let id: usize = get_tp();
    if id >= MAX_CPU_NUM {
        return None;
    }
    unsafe { PROCESSORS[id].as_mut()?.lock().current() }
}

/// Clone the current task without waiting for the per-CPU processor lock.
///
/// Non-blocking mutex acquisition uses this to attribute the guard lifetime to
/// its task. If the processor lock is busy, the caller must report contention
/// rather than create a guard that sibling exec could abandon.
pub(crate) fn try_current_task() -> Option<Arc<TaskControlBlock>> {
    let id: usize = get_tp();
    if id >= MAX_CPU_NUM {
        return None;
    }
    unsafe { PROCESSORS[id].as_ref()?.try_lock()?.current() }
}

pub(crate) fn cpu_has_current_task(cpu: usize) -> bool {
    if cpu >= MAX_CPU_NUM {
        return false;
    }
    unsafe {
        PROCESSORS[cpu]
            .as_ref()
            .and_then(|processor| processor.try_lock())
            .map_or(true, |processor| processor.current.is_some())
    }
}
#[allow(missing_docs)]
pub fn set_current_task(task: Arc<TaskControlBlock>) {
    let id: usize = get_tp();
    unsafe {
        let mut processor = PROCESSORS[id].as_mut().unwrap().lock();
        processor.current = Some(task);
        publish_current_task_owner(id, processor.current.as_ref());
    }
}
#[allow(missing_docs)]
pub fn current_process() -> Arc<ProcessControlBlock> {
    current_task().unwrap().process.upgrade().unwrap()
}
#[allow(missing_docs)]
pub fn current_user_token() -> usize {
    let task = current_task().unwrap();
    task.get_user_token()
}
#[allow(missing_docs)]
pub fn current_trap_cx() -> &'static mut TrapFrame {
    current_task()
        .unwrap()
        .inner_exclusive_access()
        .get_trap_cx()
}
#[allow(missing_docs)]
pub fn current_trap_cx_user_va() -> usize {
    current_task()
        .unwrap()
        .inner_exclusive_access()
        .res
        .as_ref()
        .unwrap()
        .trap_cx_user_va()
}
#[allow(missing_docs)]
pub fn current_kstack_top() -> usize {
    current_task().unwrap().kstack.get_top()
}
#[allow(missing_docs)]
pub fn schedule(switched_task_cx_ptr: *mut KContext) {
    // Note: check_timers() is called in run_tasks() loop, so no need to call it here
    // Calling check_timers() in schedule() (which runs in interrupt context) can cause
    // deadlock when another CPU is holding the TASK_MANAGER lock
    let id: usize = get_tp();
    // IRQ enablement belongs to the suspended kernel continuation, not to the
    // physical CPU. In particular, syscall-return writeback deliberately admits
    // timer/IPI delivery and may then yield on an lwext4 lock. Always enter the
    // scheduler with IRQs masked and restore the caller's state only after this
    // exact continuation has been selected again, including after migration.
    let irq_was_enabled = IRQ::int_enabled();
    // A restricted writeback window contains per-CPU interrupt-mask state.
    // Restore the old CPU before switching away, then derive a fresh mask from
    // the CPU on which this exact continuation eventually resumes.
    let restricted_kernel_interrupts = crate::suspend_kernel_progress_interrupts();
    unsafe {
        let mut processor = PROCESSORS[id].as_mut().unwrap().lock();
        let idle_task_cx_ptr = processor.get_idle_task_cx_ptr();
        drop(processor);
        context_switch(switched_task_cx_ptr, idle_task_cx_ptr);
    }
    if restricted_kernel_interrupts {
        crate::resume_kernel_progress_interrupts();
    }
    if irq_was_enabled {
        IRQ::int_enable();
    }
}
