use super::task::{MLFQ_BOTTOM_LEVEL, MLFQ_LEVELS, UserContextSnapshot};
use super::{ProcessControlBlock, TaskControlBlock, TaskStatus};
use crate::config::MAX_CPU_NUM;
use crate::mm::{MapArea, UserMapAreaType, VMSpace, VirtAddr};
use crate::sync::SpinNoIrqLock;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::ops::Bound::{Excluded, Unbounded};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use lazy_static::*;
use log::{error, warn};
#[allow(unused)]
const MAX_SCHED_PRIORITY: usize = 99;
#[allow(unused)]
const HIGH_PRIORITY_BUDGET: usize = 32;
const WAKE_AFFINITY_LOAD_MARGIN: usize = 1;
#[cfg(target_arch = "loongarch64")]
static LA64_RQ_DEBUG_COUNT: AtomicUsize = AtomicUsize::new(0);
static READY_TASKS: [AtomicUsize; MAX_CPU_NUM] = [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static ONLINE_CPUS: [AtomicBool; MAX_CPU_NUM] = [const { AtomicBool::new(false) }; MAX_CPU_NUM];
static ONLINE_CPU_SINCE_NS: [AtomicUsize; MAX_CPU_NUM] =
    [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static IDLE_CPUS: [AtomicBool; MAX_CPU_NUM] = [const { AtomicBool::new(false) }; MAX_CPU_NUM];
// Remote thieves must yield once the owning CPU starts fetching its own queue.
// Without owner priority, a tight try_lock() loop can repeatedly reacquire the
// queue between owner attempts. A contended owner leaves this flag set across
// idle-loop retries, so thieves back off without either CPU blocking in no-IRQ
// spinlock code.
static LOCAL_FETCH_PENDING: [AtomicBool; MAX_CPU_NUM] =
    [const { AtomicBool::new(false) }; MAX_CPU_NUM];
// Each source CPU owns one counter per target CPU. Remote enqueuers publish
// their intent without contending with unrelated CPUs; the target scheduler
// yields its next local fetch so a tight idle loop cannot starve task delivery.
// This must be a counter rather than a flag because a task can be preempted
// while waiting and another task on the same source CPU can then target the
// same queue. Either waiter clearing a shared flag would hide the other one.
static REMOTE_QUEUE_MUTATION_PENDING: [[AtomicUsize; MAX_CPU_NUM]; MAX_CPU_NUM] =
    [const { [const { AtomicUsize::new(0) }; MAX_CPU_NUM] }; MAX_CPU_NUM];
static REMOTE_ENQUEUES: AtomicUsize = AtomicUsize::new(0);
static REMOTE_IDLE_KICKS: AtomicUsize = AtomicUsize::new(0);
static REMOTE_IDLE_KICK_FAILURES: AtomicUsize = AtomicUsize::new(0);
static STEAL_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
static STEAL_SUCCESSES: AtomicUsize = AtomicUsize::new(0);
static LOCAL_FETCH_CONTENTIONS: AtomicUsize = AtomicUsize::new(0);
static LOCAL_EMPTY_MISMATCHES: AtomicUsize = AtomicUsize::new(0);
static RUN_QUEUE_POP_CANDIDATES: [AtomicUsize; MAX_CPU_NUM] =
    [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static RUN_QUEUE_POP_LEVEL: [AtomicUsize; MAX_CPU_NUM] =
    [const { AtomicUsize::new(usize::MAX) }; MAX_CPU_NUM];
static RUN_QUEUE_POP_LEN: [AtomicUsize; MAX_CPU_NUM] = [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static RUN_QUEUE_POP_CAPACITY: [AtomicUsize; MAX_CPU_NUM] =
    [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static RUN_QUEUE_POP_FIRST_PTR: [AtomicUsize; MAX_CPU_NUM] =
    [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static RUN_QUEUE_POP_FIRST_LEN: [AtomicUsize; MAX_CPU_NUM] =
    [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static RUN_QUEUE_POP_SECOND_PTR: [AtomicUsize; MAX_CPU_NUM] =
    [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static RUN_QUEUE_POP_SECOND_LEN: [AtomicUsize; MAX_CPU_NUM] =
    [const { AtomicUsize::new(0) }; MAX_CPU_NUM];

struct LocalQueueOwnerIntent {
    cpu: usize,
}

impl LocalQueueOwnerIntent {
    fn publish_if_local(cpu: usize) -> Option<Self> {
        if current_cpu() != cpu {
            return None;
        }
        LOCAL_FETCH_PENDING[cpu].store(true, Ordering::SeqCst);
        Some(Self { cpu })
    }
}

impl Drop for LocalQueueOwnerIntent {
    fn drop(&mut self) {
        LOCAL_FETCH_PENDING[self.cpu].store(false, Ordering::SeqCst);
    }
}

struct RemoteQueueMutationIntent {
    target_cpu: usize,
    source_cpu: usize,
}

impl RemoteQueueMutationIntent {
    fn publish_if_remote(target_cpu: usize) -> Option<Self> {
        let source_cpu = current_cpu();
        if source_cpu >= MAX_CPU_NUM || source_cpu == target_cpu {
            return None;
        }
        REMOTE_QUEUE_MUTATION_PENDING[target_cpu][source_cpu].fetch_add(1, Ordering::SeqCst);
        Some(Self {
            target_cpu,
            source_cpu,
        })
    }
}

impl Drop for RemoteQueueMutationIntent {
    fn drop(&mut self) {
        let previous = REMOTE_QUEUE_MUTATION_PENDING[self.target_cpu][self.source_cpu]
            .fetch_sub(1, Ordering::SeqCst);
        debug_assert!(previous > 0, "remote queue mutation intent underflow");
    }
}

fn remote_queue_mutation_pending(target_cpu: usize) -> bool {
    REMOTE_QUEUE_MUTATION_PENDING[target_cpu]
        .iter()
        .enumerate()
        .any(|(source_cpu, pending)| {
            source_cpu != target_cpu && pending.load(Ordering::SeqCst) != 0
        })
}

#[cfg(target_arch = "loongarch64")]
fn la64_rq_debug_enabled(pid: usize) -> bool {
    (pid == 2 || pid == 3) && LA64_RQ_DEBUG_COUNT.fetch_add(1, Ordering::Relaxed) < 32
}

lazy_static! {
    pub static ref TASK_MANAGER: [SpinNoIrqLock<TaskManager>; MAX_CPU_NUM] =
        core::array::from_fn(|_| SpinNoIrqLock::new(TaskManager::new()));
    pub static ref PID2PCB: SpinNoIrqLock<BTreeMap<usize, Arc<ProcessControlBlock>>> =
        SpinNoIrqLock::new(BTreeMap::new());
    /// 全局 TID -> TaskControlBlock 映射（弱引用，由 process.tasks 保持强引用）
    pub static ref TID2TASK: SpinNoIrqLock<BTreeMap<usize, Weak<TaskControlBlock>>> =
        SpinNoIrqLock::new(BTreeMap::new());
    /// 维护设置了 alarm/itimer 的进程，避免 timer 中断遍历所有进程
    pub static ref TIMER_PROCS: SpinNoIrqLock<BTreeMap<usize, Arc<ProcessControlBlock>>> =
        SpinNoIrqLock::new(BTreeMap::new());
}

#[allow(missing_docs)]
pub struct Tid2TaskStats {
    pub entries: usize,
    pub live: usize,
    pub dead: usize,
    pub lock_busy: bool,
}

#[allow(missing_docs)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct ProcessMemoryRetentionStats {
    pub processes: usize,
    pub lock_busy: bool,
    pub locked_processes: usize,
    pub zombie_processes: usize,
    pub user_areas: usize,
    pub user_data_frames: usize,
    pub elf_frames: usize,
    pub heap_frames: usize,
    pub stack_frames: usize,
    pub mmap_frames: usize,
    pub shm_frames: usize,
    pub other_frames: usize,
    pub fd_slots: usize,
    pub open_files: usize,
    pub child_refs: usize,
    pub max_data_frames: usize,
    pub max_data_frames_pid: usize,
    pub max_data_frames_zombie: bool,
    pub max_open_files: usize,
    pub max_open_files_pid: usize,
    pub max_fd_slots: usize,
    pub max_fd_slots_pid: usize,
    pub max_process_strong_count: usize,
    pub max_process_strong_count_pid: usize,
}

impl ProcessMemoryRetentionStats {
    fn lock_busy() -> Self {
        Self {
            processes: 0,
            lock_busy: true,
            locked_processes: 0,
            zombie_processes: 0,
            user_areas: 0,
            user_data_frames: 0,
            elf_frames: 0,
            heap_frames: 0,
            stack_frames: 0,
            mmap_frames: 0,
            shm_frames: 0,
            other_frames: 0,
            fd_slots: 0,
            open_files: 0,
            child_refs: 0,
            max_data_frames: 0,
            max_data_frames_pid: 0,
            max_data_frames_zombie: false,
            max_open_files: 0,
            max_open_files_pid: 0,
            max_fd_slots: 0,
            max_fd_slots_pid: 0,
            max_process_strong_count: 0,
            max_process_strong_count_pid: 0,
        }
    }

    fn empty(processes: usize) -> Self {
        Self {
            processes,
            lock_busy: false,
            ..Self::lock_busy()
        }
    }
}

pub struct TaskManager {
    ready_queues: [VecDeque<Arc<TaskControlBlock>>; MLFQ_LEVELS],
    /// Realtime tasks sorted by descending static priority. Insertion after
    /// existing equal-priority entries provides FIFO/RR ordering.
    realtime_queue: VecDeque<Arc<TaskControlBlock>>,
    sched_epoch: usize,
    aging_cursor_level: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct LoadBalanceStats {
    pub remote_enqueues: usize,
    pub remote_idle_kicks: usize,
    pub remote_idle_kick_failures: usize,
    pub steal_attempts: usize,
    pub steal_successes: usize,
    pub ready_tasks: [usize; MAX_CPU_NUM],
    pub online_mask: usize,
    pub idle_mask: usize,
    pub stalled_mask: usize,
    pub scheduler_heartbeats_ns: [usize; MAX_CPU_NUM],
    pub timer_interrupt_heartbeats_ns: [usize; MAX_CPU_NUM],
    pub timer_programming: crate::interrupts::TimerProgrammingStats,
    pub physical_ready_tasks: [Option<usize>; MAX_CPU_NUM],
    pub local_fetch_pending: [bool; MAX_CPU_NUM],
    pub remote_queue_mutation_pending_mask: [usize; MAX_CPU_NUM],
    pub run_queue_locked: [bool; MAX_CPU_NUM],
    pub run_queue_owner_harts: [usize; MAX_CPU_NUM],
    pub run_queue_owner_lines: [usize; MAX_CPU_NUM],
    pub local_fetch_contentions: usize,
    pub local_empty_mismatches: usize,
    pub run_queue_pop_candidates: [usize; MAX_CPU_NUM],
    pub run_queue_pop_level: [usize; MAX_CPU_NUM],
    pub run_queue_pop_len: [usize; MAX_CPU_NUM],
    pub run_queue_pop_capacity: [usize; MAX_CPU_NUM],
    pub run_queue_pop_first_ptr: [usize; MAX_CPU_NUM],
    pub run_queue_pop_first_len: [usize; MAX_CPU_NUM],
    pub run_queue_pop_second_ptr: [usize; MAX_CPU_NUM],
    pub run_queue_pop_second_len: [usize; MAX_CPU_NUM],
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TaskStateStats {
    pub process_table_busy: bool,
    pub process_locks_busy: usize,
    pub first_busy_process_pid: usize,
    pub first_busy_process_owner_cpu: usize,
    pub first_busy_process_owner_line: usize,
    pub task_locks_busy: usize,
    pub total: usize,
    pub ready: usize,
    pub running: usize,
    pub blocked: usize,
    pub zombie: usize,
    pub sleep: usize,
    pub ready_unowned: usize,
    pub running_not_on_cpu: usize,
    pub blocked_queued: usize,
    pub active_syscalls: usize,
    pub first_active_syscall: Option<usize>,
    pub first_active_pid: usize,
    pub active_samples: [Option<(usize, usize, usize)>; MAX_CPU_NUM],
    pub workload_sample_count: usize,
    pub workload_samples: [Option<(
        usize,
        TaskStatus,
        Option<usize>,
        Option<usize>,
        Option<usize>,
    )>; 8],
    pub workload_context_samples: [Option<(usize, UserContextSnapshot)>; 8],
    /// Task matching the C journal owner, if it is still registered.
    pub lwext4_journal_owner_task: Option<(
        usize,
        usize,
        usize,
        TaskStatus,
        Option<usize>,
        usize,
        Option<usize>,
        Option<usize>,
    )>,
    /// Task matching the C block-cache owner, if it is still registered.
    pub lwext4_bcache_owner_task: Option<(
        usize,
        usize,
        usize,
        TaskStatus,
        Option<usize>,
        usize,
        Option<usize>,
        Option<usize>,
    )>,
}

impl TaskStateStats {
    /// Empty storage suitable for per-CPU scheduler diagnostic buffers.
    ///
    /// Scheduler diagnostics run on the idle/boot stack.  Keeping their large
    /// result object in static per-CPU storage avoids growing that stack before
    /// the diagnostic walk has even recorded its first internal phase marker.
    pub const fn empty() -> Self {
        Self {
            process_table_busy: false,
            process_locks_busy: 0,
            first_busy_process_pid: 0,
            first_busy_process_owner_cpu: 0,
            first_busy_process_owner_line: 0,
            task_locks_busy: 0,
            total: 0,
            ready: 0,
            running: 0,
            blocked: 0,
            zombie: 0,
            sleep: 0,
            ready_unowned: 0,
            running_not_on_cpu: 0,
            blocked_queued: 0,
            active_syscalls: 0,
            first_active_syscall: None,
            first_active_pid: 0,
            active_samples: [None; MAX_CPU_NUM],
            workload_sample_count: 0,
            workload_samples: [None; 8],
            workload_context_samples: [None; 8],
            lwext4_journal_owner_task: None,
            lwext4_bcache_owner_task: None,
        }
    }
}

const MLFQ_AGING_SCAN_BUDGET: usize = 32;
const RUN_QUEUE_MIN_CAPACITY: usize = 16;

/// Multi-level feedback queues with round-robin order inside each level.
impl TaskManager {
    pub fn new() -> Self {
        Self {
            ready_queues: core::array::from_fn(|_| VecDeque::new()),
            realtime_queue: VecDeque::new(),
            sched_epoch: 0,
            aging_cursor_level: 1,
        }
    }
    fn queue_index(task: &TaskControlBlock) -> usize {
        task.mlfq_level().min(MLFQ_BOTTOM_LEVEL)
    }
    fn add(&mut self, task: Arc<TaskControlBlock>) {
        if task.is_realtime() {
            let priority = task.effective_sched_priority();
            let position = self
                .realtime_queue
                .iter()
                .position(|queued| queued.effective_sched_priority() < priority)
                .unwrap_or(self.realtime_queue.len());
            self.realtime_queue.insert(position, task);
            return;
        }
        task.note_mlfq_enqueued(self.sched_epoch);
        let level = Self::queue_index(&task);
        self.ready_queues[level].push_back(task);
    }
    fn add_front(&mut self, task: Arc<TaskControlBlock>) {
        if task.is_realtime() {
            let priority = task.effective_sched_priority();
            let position = self
                .realtime_queue
                .iter()
                .position(|queued| queued.effective_sched_priority() <= priority)
                .unwrap_or(self.realtime_queue.len());
            self.realtime_queue.insert(position, task);
            return;
        }
        task.note_mlfq_enqueued(self.sched_epoch);
        let level = Self::queue_index(&task);
        self.ready_queues[level].push_front(task);
    }
    fn complete_selection(&mut self) {
        self.sched_epoch = self.sched_epoch.wrapping_add(1);
    }
    fn pop_next(&mut self, cpu: usize, candidates: usize) -> Option<Arc<TaskControlBlock>> {
        let rt_len = self.realtime_queue.len();
        for _ in 0..rt_len {
            let task = self.realtime_queue.pop_front()?;
            if task.allows_cpu(cpu) {
                return Some(task);
            }
            self.realtime_queue.push_back(task);
        }
        for level in 0..MLFQ_LEVELS {
            let queue = &mut self.ready_queues[level];
            let len = queue.len();
            let capacity = queue.capacity();
            RUN_QUEUE_POP_CANDIDATES[cpu].store(candidates, Ordering::Release);
            RUN_QUEUE_POP_LEVEL[cpu].store(level, Ordering::Release);
            RUN_QUEUE_POP_LEN[cpu].store(len, Ordering::Release);
            RUN_QUEUE_POP_CAPACITY[cpu].store(capacity, Ordering::Release);

            // as_slices() only derives the two ring-buffer spans from the
            // VecDeque metadata; it does not read an element. Publish both
            // addresses before pop_front() so a different CPU can diagnose a
            // queue whose backing allocation or head metadata is corrupt even
            // when the owner faults while loading the first Arc.
            let (first, second) = queue.as_slices();
            RUN_QUEUE_POP_FIRST_PTR[cpu].store(first.as_ptr() as usize, Ordering::Release);
            RUN_QUEUE_POP_FIRST_LEN[cpu].store(first.len(), Ordering::Release);
            RUN_QUEUE_POP_SECOND_PTR[cpu].store(second.as_ptr() as usize, Ordering::Release);
            RUN_QUEUE_POP_SECOND_LEN[cpu].store(second.len(), Ordering::Release);

            if len > capacity || first.len().saturating_add(second.len()) != len {
                log::error!(
                    "[RUN_QUEUE_CORRUPT] cpu={} level={} candidates={} len={} capacity={} first_ptr={:#x} first_len={} second_ptr={:#x} second_len={}",
                    cpu,
                    level,
                    candidates,
                    len,
                    capacity,
                    first.as_ptr() as usize,
                    first.len(),
                    second.as_ptr() as usize,
                    second.len(),
                );
                panic!(
                    "[RUN_QUEUE_CORRUPT] cpu={} level={} candidates={} len={} capacity={} first_ptr={:#x} first_len={} second_ptr={:#x} second_len={}",
                    cpu,
                    level,
                    candidates,
                    len,
                    capacity,
                    first.as_ptr() as usize,
                    first.len(),
                    second.as_ptr() as usize,
                    second.len(),
                );
            }

            for _ in 0..len {
                crate::task::processor::record_scheduler_phase(140 + level * 2, None);
                let task = queue.pop_front();
                crate::task::processor::record_scheduler_phase(141 + level * 2, None);
                if let Some(task) = task {
                    if task.allows_cpu(cpu) {
                        return Some(task);
                    }
                    queue.push_back(task);
                }
            }
        }
        None
    }
    fn requeue_after_failed_claim(&mut self, task: Arc<TaskControlBlock>) {
        if task.is_realtime() {
            self.add_front(task);
            return;
        }
        let level = Self::queue_index(&task);
        self.ready_queues[level].push_back(task);
    }
    fn next_aging_level(&mut self) -> usize {
        let level = self.aging_cursor_level.clamp(1, MLFQ_BOTTOM_LEVEL);
        self.aging_cursor_level += 1;
        if self.aging_cursor_level >= MLFQ_LEVELS {
            self.aging_cursor_level = 1;
        }
        level
    }
    fn age_queued_tasks(&mut self) {
        let mut promoted = 0;
        for _ in 1..MLFQ_LEVELS {
            if promoted >= MLFQ_AGING_SCAN_BUDGET {
                break;
            }
            let level = self.next_aging_level();
            while promoted < MLFQ_AGING_SCAN_BUDGET {
                let should_promote = self.ready_queues[level]
                    .front()
                    .is_some_and(|task| task.mlfq_wait_expired(self.sched_epoch));
                if !should_promote {
                    break;
                }
                let new_level = level - 1;
                // A run-queue lock must never enter the global allocator. The
                // enqueue path reserves every level for the queue's total task
                // count, but keep the invariant explicit here so promotion
                // remains non-allocating even if an older queue has no reserve.
                if self.ready_queues[new_level].len() == self.ready_queues[new_level].capacity() {
                    break;
                }
                let task = self.ready_queues[level].pop_front().unwrap();
                task.set_mlfq_level(new_level);
                task.note_mlfq_enqueued(self.sched_epoch);
                self.ready_queues[new_level].push_back(task);
                promoted += 1;
            }
        }
    }
    fn remove(&mut self, task: &Arc<TaskControlBlock>) -> Option<Arc<TaskControlBlock>> {
        if let Some((id, _)) = self
            .realtime_queue
            .iter()
            .enumerate()
            .find(|(_, t)| Arc::as_ptr(t) == Arc::as_ptr(task))
        {
            return self.realtime_queue.remove(id);
        }
        for queue in self.ready_queues.iter_mut() {
            if let Some((id, _)) = queue
                .iter()
                .enumerate()
                .find(|(_, t)| Arc::as_ptr(t) == Arc::as_ptr(task))
            {
                return queue.remove(id);
            }
        }
        None
    }
    pub fn len(&self) -> usize {
        self.realtime_queue.len() + self.ready_queues.iter().map(VecDeque::len).sum::<usize>()
    }
}

/// Enqueue without allocating while a run-queue lock is held.
///
/// Every level is kept large enough to hold all tasks assigned to this CPU,
/// so aging can move every task into one level without growing a VecDeque.
fn enqueue_task_on_cpu(cpu: usize, task: Arc<TaskControlBlock>, front: bool) -> Option<usize> {
    let _owner_intent = LocalQueueOwnerIntent::publish_if_local(cpu);
    let _remote_intent = RemoteQueueMutationIntent::publish_if_remote(cpu);
    let mut replacement: Option<[VecDeque<Arc<TaskControlBlock>>; MLFQ_LEVELS]> = None;
    let mut realtime_replacement: Option<VecDeque<Arc<TaskControlBlock>>> = None;
    loop {
        let Some(mut manager) = TASK_MANAGER[cpu].try_lock() else {
            // Never enter the deadlock-detector spin loop for a run queue.
            // The holder's critical section is bounded; retrying restores IRQ
            // state between attempts and lets an owning CPU publish priority.
            core::hint::spin_loop();
            continue;
        };
        // The ownership marker and physical queue entry are committed while
        // holding the same target run-queue lock. This prevents remove_task()
        // from observing or deleting only one half of an enqueue operation.
        if task.is_ready_queued() {
            return None;
        }
        // A task must finish switching to its old CPU's idle stack before it
        // becomes visible in any ready queue. Since on_cpu can only be claimed
        // from an existing queue entry, observing NO_CPU here cannot race with
        // a new execution claim before try_mark_ready_queued() below.
        if task.is_on_cpu() {
            return None;
        }

        let required = manager.len().saturating_add(1);
        let capacity_ready = manager
            .ready_queues
            .iter()
            .all(|queue| queue.capacity() >= required)
            && manager.realtime_queue.capacity() >= required;
        let retired_queues = if !capacity_ready {
            let replacement_ready = replacement
                .as_ref()
                .is_some_and(|queues| queues.iter().all(|queue| queue.capacity() >= required))
                && realtime_replacement
                    .as_ref()
                    .is_some_and(|queue| queue.capacity() >= required);
            if !replacement_ready {
                drop(manager);
                let capacity = required
                    .max(RUN_QUEUE_MIN_CAPACITY)
                    .checked_next_power_of_two()
                    .unwrap_or(required);
                replacement = Some(core::array::from_fn(|_| VecDeque::with_capacity(capacity)));
                realtime_replacement = Some(VecDeque::with_capacity(capacity));
                continue;
            }

            let mut new_queues = replacement.take().unwrap();
            for (old_queue, new_queue) in manager.ready_queues.iter_mut().zip(new_queues.iter_mut())
            {
                while let Some(queued_task) = old_queue.pop_front() {
                    new_queue.push_back(queued_task);
                }
            }
            let mut new_realtime = realtime_replacement.take().unwrap();
            while let Some(queued_task) = manager.realtime_queue.pop_front() {
                new_realtime.push_back(queued_task);
            }
            // Retired buffers are released only after dropping the scheduler
            // lock; global deallocation may itself contend on the heap lock.
            Some((
                core::mem::replace(&mut manager.ready_queues, new_queues),
                core::mem::replace(&mut manager.realtime_queue, new_realtime),
            ))
        } else {
            None
        };

        if !task.try_mark_ready_queued(cpu) {
            drop(manager);
            drop(retired_queues);
            return None;
        }
        if front {
            manager.add_front(task);
        } else {
            manager.add(task);
        }
        READY_TASKS[cpu].fetch_add(1, Ordering::Release);
        let queue_len = manager.len();
        drop(manager);
        drop(retired_queues);
        return Some(queue_len);
    }
}

fn kick_remote_idle_cpu(cpu: usize) {
    if cpu == current_cpu() || !cpu_is_online(cpu) || !IDLE_CPUS[cpu].load(Ordering::Acquire) {
        return;
    }
    if polyhal::multicore::send_reschedule_ipi(cpu) {
        REMOTE_IDLE_KICKS.fetch_add(1, Ordering::Relaxed);
    } else {
        REMOTE_IDLE_KICK_FAILURES.fetch_add(1, Ordering::Relaxed);
    }
}

fn _task_can_enqueue(task: &Arc<TaskControlBlock>) -> bool {
    if task
        .process
        .upgrade()
        .map(|process| process.inner_exclusive_access().is_zombie)
        .unwrap_or(true)
    {
        return false;
    }
    {
        let task_inner = task.inner_exclusive_access();
        if task_inner.task_status == TaskStatus::Zombie {
            return false;
        }
    }
    true
}

#[allow(missing_docs)]
pub fn add_task(task: Arc<TaskControlBlock>) {
    let current = current_cpu();
    let target = select_enqueue_cpu(current, task.affinity_mask());
    if target != current {
        REMOTE_ENQUEUES.fetch_add(1, Ordering::Relaxed);
    }
    add_task_to_cpu(task, target);
}

pub fn add_task_front(task: Arc<TaskControlBlock>) {
    let current = current_cpu();
    let target = select_enqueue_cpu(current, task.affinity_mask());
    if target != current {
        REMOTE_ENQUEUES.fetch_add(1, Ordering::Relaxed);
    }
    add_task_to_cpu_front(task, target);
}

/// Requeue a task after it has switched back to its previous CPU's idle stack.
///
/// Queue-front and realtime requeues retain CPU locality so FIFO/RR ordering
/// and priority-inheritance behavior do not change. An ordinary queue-tail
/// requeue is a safe push-migration point: the task is no longer on a CPU and
/// the caller holds no run-queue lock, so the normal remote-enqueue protocol
/// can place it on a less loaded CPU without remote dequeue/stealing.
pub(crate) fn requeue_task_after_switch(
    task: Arc<TaskControlBlock>,
    previous_cpu: usize,
    front: bool,
) {
    let previous_cpu = valid_cpu(previous_cpu);
    if front {
        add_task_to_cpu_front(task, previous_cpu);
        return;
    }
    // A schedulable kernel continuation may still own filesystem or memory
    // locks whose diagnostics and recovery state are CPU-local. Keep that
    // continuation on the same CPU; ordinary user-mode time-slice/yield
    // requeues remain migratable.
    if task.is_realtime() || task.kernel_critical_section_active() {
        add_task_to_cpu(task, previous_cpu);
        return;
    }

    let target = select_enqueue_cpu(previous_cpu, task.affinity_mask());
    if target != previous_cpu {
        REMOTE_ENQUEUES.fetch_add(1, Ordering::Relaxed);
    }
    add_task_to_cpu(task, target);
}

pub fn add_task_to_cpu(task: Arc<TaskControlBlock>, cpu: usize) {
    #[cfg(target_arch = "loongarch64")]
    let pid = task
        .process
        .upgrade()
        .map(|process| process.getpid())
        .unwrap_or(usize::MAX);

    {
        let task_inner = task.inner_exclusive_access();
        if task_inner.task_status != TaskStatus::Ready {
            #[cfg(target_arch = "loongarch64")]
            if la64_rq_debug_enabled(pid) {
                warn!(
                    "[la64 rq] add reject: pid={} tid={} cpu={} ready_queued={} on_cpu={}",
                    pid,
                    task_inner.global_tid,
                    cpu,
                    task.is_ready_queued(),
                    task.is_on_cpu(),
                );
            }
            return;
        }
    }
    let cpu = if task.allows_cpu(valid_cpu(cpu)) {
        valid_cpu(cpu)
    } else {
        select_enqueue_cpu(current_cpu(), task.affinity_mask())
    };
    let Some(_queue_len) = enqueue_task_on_cpu(cpu, Arc::clone(&task), false) else {
        #[cfg(target_arch = "loongarch64")]
        if la64_rq_debug_enabled(pid) {
            warn!(
                "[la64 rq] add mark-ready failed: pid={} cpu={} ready_queued={} on_cpu={}",
                pid,
                cpu,
                task.is_ready_queued(),
                task.is_on_cpu(),
            );
        }
        return;
    };
    #[cfg(target_arch = "loongarch64")]
    if la64_rq_debug_enabled(pid) {
        warn!(
            "[la64 rq] add ok: pid={} cpu={} queue_len={} ready_queued={} on_cpu={}",
            pid,
            cpu,
            _queue_len,
            task.is_ready_queued(),
            task.is_on_cpu(),
        );
    }
    kick_remote_idle_cpu(cpu);
    // The local scheduler already observes its own run queue. Sending a
    // reschedule IPI to the enqueueing CPU makes a realtime task generate a
    // new self-IPI every time it is preempted and requeued.
    if cpu != current_cpu() && task.is_realtime() {
        let _ = polyhal::multicore::send_reschedule_ipi(cpu);
    }
}

pub fn add_task_to_cpu_front(task: Arc<TaskControlBlock>, cpu: usize) {
    {
        let task_inner = task.inner_exclusive_access();
        if task_inner.task_status != TaskStatus::Ready {
            return;
        }
    }
    let cpu = if task.allows_cpu(valid_cpu(cpu)) {
        valid_cpu(cpu)
    } else {
        select_enqueue_cpu(current_cpu(), task.affinity_mask())
    };
    let realtime = task.is_realtime();
    if enqueue_task_on_cpu(cpu, task, true).is_some() {
        kick_remote_idle_cpu(cpu);
        if cpu != current_cpu() && realtime {
            let _ = polyhal::multicore::send_reschedule_ipi(cpu);
        }
    }
}

fn enqueue_woken_task(task: Arc<TaskControlBlock>, front: bool) {
    let current = current_cpu();
    let preferred = task.last_cpu_index().unwrap_or(current);
    let target = select_wakeup_cpu(preferred, task.affinity_mask());
    if target != current {
        REMOTE_ENQUEUES.fetch_add(1, Ordering::Relaxed);
    }
    if front {
        add_task_to_cpu_front(task, target);
    } else {
        add_task_to_cpu(task, target);
    }
}

#[allow(missing_docs)]
pub fn wakeup_task(task: Arc<TaskControlBlock>) {
    let mut task_inner = task.inner_exclusive_access();
    let status_before = task_inner.task_status;
    let pending_before = task_inner.pending_wakeup;
    let on_cpu = task.is_on_cpu();
    let queued = task.is_ready_queued();
    let (pid, global_tid) = (
        task.process.upgrade().map(|process| process.getpid()),
        task_inner.global_tid,
    );
    log::debug!(
        "[IOZONE_HANG wakeup_enter] cpu={} pid={:?} global_tid={} status={:?} pending={} on_cpu={} queued={}",
        current_cpu(),
        pid,
        global_tid,
        status_before,
        pending_before,
        on_cpu,
        queued
    );
    if task_inner.task_status == TaskStatus::Zombie {
        return;
    }
    if task_inner.group_stopped {
        // A wakeup that races with a group stop must be remembered, but the
        // task cannot be published as runnable until SIGCONT clears the stop.
        task_inner.group_stop_resume = true;
        task_inner.pending_wakeup = true;
        return;
    }
    if task.is_on_cpu() {
        task_inner.pending_wakeup = true;
        if task_inner.task_status != TaskStatus::Running {
            if task_inner.task_status == TaskStatus::Blocked {
                task_inner.requeue_after_switch = true;
                task_inner.requeue_front_after_switch = false;
            }
            task_inner.task_status = TaskStatus::Ready;
        }
        log::debug!(
            "[IOZONE_HANG wakeup_on_cpu] cpu={} pid={:?} global_tid={} status_before={:?} status_after={:?} pending=true",
            current_cpu(),
            pid,
            global_tid,
            status_before,
            task_inner.task_status
        );
        drop(task_inner);
        return;
    }
    if task_inner.task_status == TaskStatus::Running {
        task_inner.pending_wakeup = true;
        log::debug!(
            "[IOZONE_HANG wakeup_running] cpu={} pid={:?} global_tid={} pending=true",
            current_cpu(),
            pid,
            global_tid
        );
        drop(task_inner);
        return;
    }
    if task_inner.task_status == TaskStatus::Ready {
        drop(task_inner);
        if !task.is_ready_queued() && !task.is_on_cpu() {
            enqueue_woken_task(task, false);
        }
        return;
    }
    task.boost_mlfq_level();
    task_inner.task_status = TaskStatus::Ready;
    drop(task_inner);
    log::debug!(
        "[IOZONE_HANG wakeup_enqueue] cpu={} pid={:?} global_tid={} status_before={:?}",
        current_cpu(),
        pid,
        global_tid,
        status_before
    );
    enqueue_woken_task(task, false);
}

#[allow(missing_docs)]
pub fn wakeup_task_front(task: Arc<TaskControlBlock>) -> bool {
    let mut task_inner = task.inner_exclusive_access();
    if task_inner.task_status == TaskStatus::Zombie {
        return false;
    }
    if task_inner.group_stopped {
        task_inner.group_stop_resume = true;
        task_inner.pending_wakeup = true;
        return true;
    }
    if task.is_on_cpu() {
        task_inner.pending_wakeup = true;
        if task_inner.task_status != TaskStatus::Running {
            if task_inner.task_status == TaskStatus::Blocked {
                task_inner.requeue_after_switch = true;
                task_inner.requeue_front_after_switch = true;
            }
            task_inner.task_status = TaskStatus::Ready;
        }
        return true;
    }
    if task_inner.task_status == TaskStatus::Running {
        task_inner.pending_wakeup = true;
        return true;
    }
    if task_inner.task_status == TaskStatus::Ready {
        drop(task_inner);
        if !task.is_ready_queued() && !task.is_on_cpu() {
            enqueue_woken_task(Arc::clone(&task), true);
        }
        return task.is_ready_queued() || task.is_on_cpu();
    }
    task.boost_mlfq_level();
    task_inner.task_status = TaskStatus::Ready;
    drop(task_inner);
    enqueue_woken_task(Arc::clone(&task), true);
    task.is_ready_queued() || task.is_on_cpu()
}

#[allow(missing_docs)]
pub fn remove_task(task: Arc<TaskControlBlock>) {
    loop {
        let Some(cpu) = task.ready_queued_cpu() else {
            return;
        };
        let cpu = valid_cpu(cpu);
        let _owner_intent = LocalQueueOwnerIntent::publish_if_local(cpu);
        let _remote_intent = RemoteQueueMutationIntent::publish_if_remote(cpu);
        let removed = {
            let Some(mut manager) = TASK_MANAGER[cpu].try_lock() else {
                core::hint::spin_loop();
                continue;
            };
            if !task.is_ready_queued_at(cpu) {
                continue;
            }
            let removed = manager.remove(&task);
            if removed.is_some() {
                let cleared = task.try_clear_ready_queued(cpu);
                debug_assert!(cleared, "ready queue owner changed while locked");
            } else {
                // Defensive recovery for a marker left by an older/incomplete
                // enqueue. New enqueues publish the marker only under this lock.
                let _ = task.try_clear_ready_queued(cpu);
                READY_TASKS[cpu].store(manager.len(), Ordering::Release);
            }
            removed
        };
        if removed.is_some() {
            decrement_ready_tasks(cpu);
        }
        drop(removed);
        return;
    }
}

enum ClaimTaskResult {
    Claimed(Arc<TaskControlBlock>),
    Empty,
    Contended,
}

fn claim_task_from_cpu(queued_cpu: usize, run_cpu: usize) -> ClaimTaskResult {
    let local_fetch = queued_cpu == run_cpu;
    let (task, stale_task) = {
        // A remote steal is opportunistic: never spin on a victim queue that
        // its owner is actively scheduling.  Check both before and after the
        // try_lock so an active owner that announces itself concurrently cannot
        // be starved by a thief repeatedly reacquiring the queue. An intent left
        // behind by a scheduler whose heartbeat has stopped is advisory: honoring
        // it forever would make that CPU's ready queue impossible to rescue.
        let mut manager = if local_fetch {
            // Keep the owner-intent flag published across a failed attempt.
            // The idle loop retries immediately, while every remote thief must
            // back off. This guarantees owner progress without spinning with
            // interrupts disabled behind a remote CPU that the host may have
            // temporarily descheduled.
            LOCAL_FETCH_PENDING[queued_cpu].store(true, Ordering::SeqCst);
            if remote_queue_mutation_pending(queued_cpu) {
                LOCAL_FETCH_PENDING[queued_cpu].store(false, Ordering::SeqCst);
                return ClaimTaskResult::Contended;
            }
            let Some(manager) = TASK_MANAGER[queued_cpu].try_lock() else {
                LOCAL_FETCH_CONTENTIONS.fetch_add(1, Ordering::Relaxed);
                return ClaimTaskResult::Contended;
            };
            manager
        } else {
            let now_ns = polyhal::timer::current_time().as_nanos() as usize;
            let owner_stalled = crate::task::processor::scheduler_cpu_stalled(queued_cpu, now_ns);
            if LOCAL_FETCH_PENDING[queued_cpu].load(Ordering::SeqCst) && !owner_stalled {
                return ClaimTaskResult::Contended;
            }
            let Some(manager) = TASK_MANAGER[queued_cpu].try_lock() else {
                return ClaimTaskResult::Contended;
            };
            let now_ns = polyhal::timer::current_time().as_nanos() as usize;
            let owner_stalled = crate::task::processor::scheduler_cpu_stalled(queued_cpu, now_ns);
            if LOCAL_FETCH_PENDING[queued_cpu].load(Ordering::SeqCst) && !owner_stalled {
                drop(manager);
                return ClaimTaskResult::Contended;
            }
            manager
        };
        crate::task::processor::record_scheduler_phase(40, None);
        // Failed steal attempts do not advance the MLFQ epoch; otherwise idle
        // CPUs spinning on a briefly unclaimable task would accelerate aging.
        let mut candidates = manager.len();
        if local_fetch && candidates > 1 {
            crate::task::processor::record_scheduler_phase(41, None);
            manager.age_queued_tasks();
        }
        crate::task::processor::record_scheduler_phase(42, None);
        candidates = manager.len();
        crate::task::processor::record_scheduler_phase(43, None);
        if local_fetch && candidates == 0 && READY_TASKS[queued_cpu].load(Ordering::Acquire) != 0 {
            LOCAL_EMPTY_MISMATCHES.fetch_add(1, Ordering::Relaxed);
        }
        let mut claimed = None;
        let mut stale_task = None;

        for _ in 0..candidates {
            crate::task::processor::record_scheduler_phase(44, None);
            let Some(task) = manager.pop_next(run_cpu, candidates) else {
                break;
            };
            crate::task::processor::record_scheduler_phase(45, None);
            let claim_succeeded = task.try_claim_queued(queued_cpu, run_cpu);
            crate::task::processor::record_scheduler_phase(46, None);
            if claim_succeeded {
                decrement_ready_tasks(queued_cpu);
                manager.complete_selection();
                claimed = Some(task);
                break;
            }

            if task.is_ready_queued_at(queued_cpu) {
                // A freshly preempted task may still carry its old on_cpu
                // claim until the context switch reaches the idle stack.
                manager.requeue_after_failed_claim(task);
            } else {
                // remove_task() cleared the ownership marker while waiting for
                // this run-queue lock, so the physical queue entry is stale.
                decrement_ready_tasks(queued_cpu);
                stale_task = Some(task);
                break;
            }
        }
        crate::task::processor::record_scheduler_phase(47, None);
        (claimed, stale_task)
    };
    crate::task::processor::record_scheduler_phase(48, None);
    if local_fetch {
        // Clear intent only after the queue guard has been released. A failed
        // local try_lock returns above and deliberately leaves intent set for
        // the next owner retry.
        LOCAL_FETCH_PENDING[queued_cpu].store(false, Ordering::SeqCst);
    }
    // A stale queue Arc may be the last task reference and release a kernel
    // stack, VM frames, or heap storage.  Never run that drop chain while the
    // scheduler queue is locked.
    drop(stale_task);
    crate::task::processor::record_scheduler_phase(49, None);
    if let Some(task) = task {
        #[cfg(target_arch = "loongarch64")]
        {
            let pid = task
                .process
                .upgrade()
                .map(|process| process.getpid())
                .unwrap_or(usize::MAX);
            if la64_rq_debug_enabled(pid) {
                let task_inner = task.inner_exclusive_access();
                warn!(
                    "[la64 rq] fetch: pid={} tid={} cpu={} ready_queued={} on_cpu={}",
                    pid,
                    task_inner.global_tid,
                    queued_cpu,
                    task.is_ready_queued(),
                    task.is_on_cpu(),
                );
            }
        }
        ClaimTaskResult::Claimed(task)
    } else {
        ClaimTaskResult::Empty
    }
}

pub fn fetch_task(cpu: usize) -> Option<Arc<TaskControlBlock>> {
    let cpu = valid_cpu(cpu);
    // Only a queue's owning CPU may dequeue from it. A remote thief can be
    // descheduled while holding the victim's no-IRQ queue lock, preventing the
    // owner from running even when its Ready count is non-zero. New and woken
    // tasks are still distributed by select_enqueue_cpu(), so SMP placement is
    // preserved without cross-CPU dequeue ownership.
    match claim_task_from_cpu(cpu, cpu) {
        ClaimTaskResult::Claimed(task) => Some(task),
        ClaimTaskResult::Contended | ClaimTaskResult::Empty => None,
    }
}

pub fn ready_queue_lengths() -> [usize; MAX_CPU_NUM] {
    core::array::from_fn(|cpu| READY_TASKS[cpu].load(Ordering::Acquire))
}

/// Lock-free run-queue evidence for interrupt-side scheduler stall reports.
///
/// This deliberately observes only atomics and lock ownership metadata. Stall
/// diagnostics must not acquire a run-queue lock that the target CPU may hold
/// with interrupts disabled.
#[derive(Debug, Clone, Copy)]
#[cfg(target_arch = "riscv64")]
pub(crate) struct RunQueueStallProgress {
    pub ready_tasks: usize,
    pub locked: bool,
    pub owner_hart: usize,
    pub owner_line: usize,
    pub local_fetch_pending: bool,
    pub remote_mutation_pending_mask: usize,
    pub pop_candidates: usize,
    pub pop_level: usize,
    pub pop_len: usize,
    pub pop_capacity: usize,
    pub pop_first_ptr: usize,
    pub pop_first_len: usize,
    pub pop_second_ptr: usize,
    pub pop_second_len: usize,
}

#[cfg(target_arch = "riscv64")]
pub(crate) fn run_queue_stall_progress(cpu: usize) -> Option<RunQueueStallProgress> {
    if cpu >= MAX_CPU_NUM {
        return None;
    }
    let remote_mutation_pending_mask = REMOTE_QUEUE_MUTATION_PENDING[cpu].iter().enumerate().fold(
        0usize,
        |mask, (source_cpu, pending)| {
            if pending.load(Ordering::SeqCst) != 0 {
                mask | (1usize << source_cpu)
            } else {
                mask
            }
        },
    );
    Some(RunQueueStallProgress {
        ready_tasks: READY_TASKS[cpu].load(Ordering::Acquire),
        locked: TASK_MANAGER[cpu].is_locked(),
        owner_hart: TASK_MANAGER[cpu].owner_hart(),
        owner_line: TASK_MANAGER[cpu].owner_line(),
        local_fetch_pending: LOCAL_FETCH_PENDING[cpu].load(Ordering::SeqCst),
        remote_mutation_pending_mask,
        pop_candidates: RUN_QUEUE_POP_CANDIDATES[cpu].load(Ordering::Acquire),
        pop_level: RUN_QUEUE_POP_LEVEL[cpu].load(Ordering::Acquire),
        pop_len: RUN_QUEUE_POP_LEN[cpu].load(Ordering::Acquire),
        pop_capacity: RUN_QUEUE_POP_CAPACITY[cpu].load(Ordering::Acquire),
        pop_first_ptr: RUN_QUEUE_POP_FIRST_PTR[cpu].load(Ordering::Acquire),
        pop_first_len: RUN_QUEUE_POP_FIRST_LEN[cpu].load(Ordering::Acquire),
        pop_second_ptr: RUN_QUEUE_POP_SECOND_PTR[cpu].load(Ordering::Acquire),
        pop_second_len: RUN_QUEUE_POP_SECOND_LEN[cpu].load(Ordering::Acquire),
    })
}

pub fn load_balance_stats() -> LoadBalanceStats {
    let mut physical_ready_tasks = [None; MAX_CPU_NUM];
    let mut run_queue_locked = [false; MAX_CPU_NUM];
    let mut run_queue_owner_harts = [usize::MAX; MAX_CPU_NUM];
    let mut run_queue_owner_lines = [0; MAX_CPU_NUM];
    for cpu in 0..MAX_CPU_NUM {
        if let Some(manager) = TASK_MANAGER[cpu].try_lock() {
            physical_ready_tasks[cpu] = Some(manager.len());
        } else {
            run_queue_locked[cpu] = TASK_MANAGER[cpu].is_locked();
            run_queue_owner_harts[cpu] = TASK_MANAGER[cpu].owner_hart();
            run_queue_owner_lines[cpu] = TASK_MANAGER[cpu].owner_line();
        }
    }
    let now_ns = polyhal::timer::current_time().as_nanos() as usize;
    let scheduler_heartbeats_ns =
        core::array::from_fn(|cpu| crate::task::processor::scheduler_heartbeat_ns(cpu));
    let timer_interrupt_heartbeats_ns = crate::interrupts::timer_interrupt_heartbeats_ns();
    let timer_programming = crate::interrupts::timer_programming_stats();
    let stalled_mask = (0..MAX_CPU_NUM).fold(0usize, |mask, cpu| {
        if cpu_is_online(cpu) && crate::task::processor::scheduler_cpu_stalled(cpu, now_ns) {
            mask | (1usize << cpu)
        } else {
            mask
        }
    });
    LoadBalanceStats {
        remote_enqueues: REMOTE_ENQUEUES.load(Ordering::Relaxed),
        remote_idle_kicks: REMOTE_IDLE_KICKS.load(Ordering::Relaxed),
        remote_idle_kick_failures: REMOTE_IDLE_KICK_FAILURES.load(Ordering::Relaxed),
        steal_attempts: STEAL_ATTEMPTS.load(Ordering::Relaxed),
        steal_successes: STEAL_SUCCESSES.load(Ordering::Relaxed),
        ready_tasks: ready_queue_lengths(),
        online_mask: online_cpu_mask(),
        idle_mask: idle_cpu_mask(),
        stalled_mask,
        scheduler_heartbeats_ns,
        timer_interrupt_heartbeats_ns,
        timer_programming,
        physical_ready_tasks,
        local_fetch_pending: core::array::from_fn(|cpu| {
            LOCAL_FETCH_PENDING[cpu].load(Ordering::SeqCst)
        }),
        remote_queue_mutation_pending_mask: core::array::from_fn(|target_cpu| {
            REMOTE_QUEUE_MUTATION_PENDING[target_cpu]
                .iter()
                .enumerate()
                .fold(0usize, |mask, (source_cpu, pending)| {
                    if pending.load(Ordering::SeqCst) != 0 {
                        mask | (1usize << source_cpu)
                    } else {
                        mask
                    }
                })
        }),
        run_queue_locked,
        run_queue_owner_harts,
        run_queue_owner_lines,
        local_fetch_contentions: LOCAL_FETCH_CONTENTIONS.load(Ordering::Relaxed),
        local_empty_mismatches: LOCAL_EMPTY_MISMATCHES.load(Ordering::Relaxed),
        run_queue_pop_candidates: core::array::from_fn(|cpu| {
            RUN_QUEUE_POP_CANDIDATES[cpu].load(Ordering::Acquire)
        }),
        run_queue_pop_level: core::array::from_fn(|cpu| {
            RUN_QUEUE_POP_LEVEL[cpu].load(Ordering::Acquire)
        }),
        run_queue_pop_len: core::array::from_fn(|cpu| {
            RUN_QUEUE_POP_LEN[cpu].load(Ordering::Acquire)
        }),
        run_queue_pop_capacity: core::array::from_fn(|cpu| {
            RUN_QUEUE_POP_CAPACITY[cpu].load(Ordering::Acquire)
        }),
        run_queue_pop_first_ptr: core::array::from_fn(|cpu| {
            RUN_QUEUE_POP_FIRST_PTR[cpu].load(Ordering::Acquire)
        }),
        run_queue_pop_first_len: core::array::from_fn(|cpu| {
            RUN_QUEUE_POP_FIRST_LEN[cpu].load(Ordering::Acquire)
        }),
        run_queue_pop_second_ptr: core::array::from_fn(|cpu| {
            RUN_QUEUE_POP_SECOND_PTR[cpu].load(Ordering::Acquire)
        }),
        run_queue_pop_second_len: core::array::from_fn(|cpu| {
            RUN_QUEUE_POP_SECOND_LEN[cpu].load(Ordering::Acquire)
        }),
    }
}

/// Collect task states using only try-locks so diagnostics cannot join an
/// existing scheduler/process lock cycle while the system is unhealthy. Keep
/// this path allocation-free as it runs directly on the scheduler idle stack.
///
/// Fill storage supplied by the caller instead of constructing another large
/// `TaskStateStats` in this function's stack frame.  Scheduler diagnostics
/// already reserve their output object in the idle-loop frame; duplicating the
/// roughly 1 KiB value here needlessly deepens the kernel stack at exactly the
/// point where a failing CPU must remain observable.
#[inline(never)]
pub fn fill_task_state_stats(stats: &mut TaskStateStats) {
    crate::task::processor::record_scheduler_phase(80, None);
    *stats = TaskStateStats::empty();
    let lwext4_progress = crate::fs::lwext4::lwext4_c_progress();

    // Inspect PCB lock ownership without acquiring any PCB lock.  The old
    // implementation held PID2PCB and then became the PCB owner while walking
    // all of its tasks.  A diagnostic CPU descheduled in that region could
    // make wait4/fork/exit exhaust the spinlock retry detector.
    let mut last_pid = None;
    loop {
        let next_process = {
            let Some(processes) = PID2PCB.try_lock() else {
                crate::task::processor::record_scheduler_phase(81, None);
                stats.process_table_busy = true;
                return;
            };
            crate::task::processor::record_scheduler_phase(82, None);
            let entry = match last_pid {
                Some(pid) => processes.range((Excluded(pid), Unbounded)).next(),
                None => processes.first_key_value(),
            };
            entry.map(|(&pid, process)| (pid, Arc::clone(process)))
        };
        let Some((pid, process)) = next_process else {
            break;
        };
        last_pid = Some(pid);
        crate::task::processor::record_scheduler_phase(83, None);
        if process.inner_is_locked() {
            crate::task::processor::record_scheduler_phase(84, None);
            stats.process_locks_busy += 1;
            if stats.first_busy_process_pid == 0 {
                let (owner_cpu, owner_line) = process.inner_owner_site();
                stats.first_busy_process_pid = pid;
                stats.first_busy_process_owner_cpu = owner_cpu;
                stats.first_busy_process_owner_line = owner_line;
            }
        }
    }

    // TID2TASK already is the global task index.  Walk one Weak at a time so
    // the registry guard is released before upgrading or inspecting the TCB.
    // This keeps the diagnostic path allocation-free and removes the need to
    // enter ProcessControlBlockInner just to find its tasks.
    let mut last_tid = None;
    loop {
        let next_task = {
            let Some(tasks) = TID2TASK.try_lock() else {
                stats.task_locks_busy += 1;
                break;
            };
            let entry = match last_tid {
                Some(tid) => tasks.range((Excluded(tid), Unbounded)).next(),
                None => tasks.first_key_value(),
            };
            entry.map(|(&tid, task)| (tid, task.clone()))
        };
        let Some((tid, task)) = next_task else {
            break;
        };
        last_tid = Some(tid);
        let Some(task) = task.upgrade() else {
            continue;
        };
        crate::task::processor::record_scheduler_phase(86, None);
        stats.total += 1;
        let (task_status, global_tid) = {
            let Some(task_inner) = task.try_inner_exclusive_access() else {
                crate::task::processor::record_scheduler_phase(87, None);
                stats.task_locks_busy += 1;
                continue;
            };
            crate::task::processor::record_scheduler_phase(88, None);
            (task_inner.task_status, task_inner.global_tid)
        };
        let pid = task.process_id();
        let queued_cpu = task.ready_queued_cpu();
        let on_cpu_index = task.on_cpu_index();
        let queued = queued_cpu.is_some();
        let on_cpu = on_cpu_index.is_some();
        let owner = Arc::as_ptr(&task) as usize;
        let owner_sample = || {
            Some((
                owner,
                pid,
                global_tid,
                task_status,
                task.active_syscall(),
                task.kernel_critical_section_depth(),
                queued_cpu,
                on_cpu_index,
            ))
        };
        if owner == lwext4_progress.journal_owner {
            stats.lwext4_journal_owner_task = owner_sample();
        }
        if owner == lwext4_progress.bcache_owner {
            stats.lwext4_bcache_owner_task = owner_sample();
        }
        if pid > 3 {
            let sample_index = stats.workload_sample_count;
            stats.workload_sample_count += 1;
            if sample_index < stats.workload_samples.len() {
                stats.workload_samples[sample_index] = Some((
                    pid,
                    task_status,
                    task.active_syscall(),
                    queued_cpu,
                    on_cpu_index,
                ));
                stats.workload_context_samples[sample_index] =
                    Some((pid, task.user_context_snapshot()));
            }
        }
        if task_status != TaskStatus::Zombie {
            if let Some(syscall_id) = task.active_syscall() {
                let sample_index = stats.active_syscalls;
                stats.active_syscalls += 1;
                if sample_index < MAX_CPU_NUM {
                    stats.active_samples[sample_index] =
                        Some((pid, syscall_id, task.active_syscall_stage()));
                }
                if stats.first_active_syscall.is_none() {
                    stats.first_active_syscall = Some(syscall_id);
                    stats.first_active_pid = pid;
                }
            }
        }
        match task_status {
            TaskStatus::Ready => {
                stats.ready += 1;
                if !queued && !on_cpu {
                    stats.ready_unowned += 1;
                }
            }
            TaskStatus::Running => {
                stats.running += 1;
                if !on_cpu {
                    stats.running_not_on_cpu += 1;
                }
            }
            TaskStatus::Blocked => {
                stats.blocked += 1;
                if queued {
                    stats.blocked_queued += 1;
                }
            }
            TaskStatus::Zombie => stats.zombie += 1,
            TaskStatus::Sleep => stats.sleep += 1,
        }
        crate::task::processor::record_scheduler_phase(89, None);
    }
    crate::task::processor::record_scheduler_phase(90, None);
}

pub fn task_state_stats() -> TaskStateStats {
    let mut stats = TaskStateStats::default();
    fill_task_state_stats(&mut stats);
    stats
}

/// Emit one best-effort identity/runtime/VMA record for every workload task.
///
/// Registry, task, PCB, and VM locks are acquired only with try-locks so the
/// observer cannot join the dependency that caused the stall. Mapping paths
/// are retained by the VMA at creation time and therefore need no file lock.
pub(crate) fn log_workload_task_diagnostics(tag: &str, observer_cpu: usize, sequence: usize) {
    const TASK_DIAGNOSTIC_LIMIT: usize = 64;

    let mut last_tid = None;
    let mut emitted = 0usize;
    loop {
        let next_task = {
            let Some(tasks) = TID2TASK.try_lock() else {
                error!(
                    "[TASK_RUNTIME_STALL] snapshot_tag={} observer_cpu={} sequence={} registry_lock_busy=true emitted={}",
                    tag, observer_cpu, sequence, emitted,
                );
                return;
            };
            let entry = match last_tid {
                Some(tid) => tasks.range((Excluded(tid), Unbounded)).next(),
                None => tasks.first_key_value(),
            };
            entry.map(|(&tid, task)| (tid, task.clone()))
        };
        let Some((tid, task)) = next_task else {
            break;
        };
        last_tid = Some(tid);
        let Some(task) = task.upgrade() else {
            continue;
        };
        let pid = task.process_id();
        if pid <= 3 {
            continue;
        }
        if emitted >= TASK_DIAGNOSTIC_LIMIT {
            error!(
                "[TASK_RUNTIME_STALL_SUMMARY] snapshot_tag={} observer_cpu={} sequence={} emitted={} truncated=true next_tid={}",
                tag, observer_cpu, sequence, emitted, tid,
            );
            return;
        }
        emitted += 1;

        let (task_lock_busy, status, comm) = match task.try_inner_exclusive_access() {
            Some(inner) => (false, Some(inner.task_status), inner.comm),
            None => (true, None, [0; 16]),
        };
        let comm_end = comm
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(comm.len());
        let comm = if task_lock_busy {
            "<task-lock-busy>"
        } else {
            core::str::from_utf8(&comm[..comm_end]).unwrap_or("<non-utf8>")
        };

        let process = task.process.upgrade();
        let (process_lock_busy, ppid, pgid, executable_path) = process
            .as_ref()
            .map(|process| match process.inner_try_access() {
                Some(inner) => (
                    false,
                    inner
                        .parent
                        .as_ref()
                        .and_then(Weak::upgrade)
                        .map(|parent| parent.getpid()),
                    Some(inner.pgid.0),
                    Some(inner.executable_path.clone()),
                ),
                None => (true, None, None, None),
            })
            .unwrap_or((false, None, None, None));

        let context = task.user_context_snapshot();
        let mut vm_lock_busy = false;
        let mapping = process.as_ref().and_then(|process| {
            let Some(vm_set) = process.try_vm_exclusive_access() else {
                vm_lock_busy = true;
                return None;
            };
            let va = VirtAddr::from(context.pc);
            let vpn = va.floor();
            let pte_ppn = vm_set.translate(vpn).map(|pte| pte.ppn().0);
            vm_set
                .areas
                .iter()
                .find(|area| context.pc >= area.start_va().0 && context.pc < area.end_va().0)
                .map(|area| {
                    let mapping_offset = area.file_offset;
                    let pc_file_offset = area
                        .file_offset
                        .saturating_add(context.pc.saturating_sub(area.start_va().0));
                    (
                        area.areatype(),
                        area.perm().bits(),
                        area.start_va().0,
                        area.end_va().0,
                        mapping_offset,
                        pc_file_offset,
                        area.mapping_path.clone(),
                        pte_ppn,
                        area.data_frames.get(&vpn).map(|frame| frame.ppn.0),
                    )
                })
        });

        let (
            mapping_type,
            mapping_perm,
            mapping_start,
            mapping_end,
            mapping_file_offset,
            pc_file_offset,
            mapping_path,
            pte_ppn,
            resident_ppn,
        ) = mapping
            .map(|mapping| {
                (
                    Some(mapping.0),
                    Some(mapping.1),
                    Some(mapping.2),
                    Some(mapping.3),
                    Some(mapping.4),
                    Some(mapping.5),
                    mapping.6,
                    mapping.7,
                    mapping.8,
                )
            })
            .unwrap_or((None, None, None, None, None, None, None, None, None));

        error!(
            "[TASK_RUNTIME_STALL] snapshot_tag={} observer_cpu={} sequence={} pid={} tid={} comm={} status={:?} task_lock_busy={} ppid={:?} pgid={:?} executable={:?} process_missing={} process_lock_busy={} active_syscall={:?} syscall_stage={} queued_cpu={:?} on_cpu={:?} runtime={:?} user_context={:?} vm_lock_busy={} mapping_found={} mapping_type={:?} mapping_perm={:?} mapping_range={:#x}..{:#x} mapping_path={:?} mapping_file_offset={:?} pc_file_offset={:?} pte_ppn={:?} resident_ppn={:?}",
            tag,
            observer_cpu,
            sequence,
            pid,
            tid,
            comm,
            status,
            task_lock_busy,
            ppid,
            pgid,
            executable_path,
            process.is_none(),
            process_lock_busy,
            task.active_syscall(),
            task.active_syscall_stage(),
            task.ready_queued_cpu(),
            task.on_cpu_index(),
            task.runtime_diagnostic(),
            context,
            vm_lock_busy,
            mapping_type.is_some(),
            mapping_type,
            mapping_perm,
            mapping_start.unwrap_or(0),
            mapping_end.unwrap_or(0),
            mapping_path,
            mapping_file_offset,
            pc_file_offset,
            pte_ppn,
            resident_ppn,
        );
    }
    error!(
        "[TASK_RUNTIME_STALL_SUMMARY] snapshot_tag={} observer_cpu={} sequence={} emitted={} truncated=false",
        tag, observer_cpu, sequence, emitted,
    );
}

/// Return lock ownership for one PID without blocking on the process registry
/// or on either lock represented by `ProcessInnerGuard`.
pub(crate) fn process_lock_stall_snapshot(
    pid: usize,
) -> Option<super::process::ProcessLockStallSnapshot> {
    let process = PID2PCB
        .try_lock_for_diagnostics()?
        .get(&pid)
        .map(Arc::clone)?;
    Some(process.lock_stall_snapshot())
}

pub fn mark_cpu_online(cpu: usize) {
    if cpu < MAX_CPU_NUM {
        let now_ns = polyhal::timer::current_time().as_nanos() as usize;
        let _ = ONLINE_CPU_SINCE_NS[cpu].compare_exchange(
            0,
            now_ns.max(1),
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        ONLINE_CPUS[cpu].store(true, Ordering::Release);
    }
}

/// Return the online mask, CPU count and cumulative available CPU time.
/// Tracking each CPU from the instant it enters the scheduler avoids charging
/// secondary-hart startup time as kernel work.
pub(crate) fn online_cpu_capacity_ns(now_ns: usize) -> (usize, usize, usize) {
    let mut mask = 0usize;
    let mut count = 0usize;
    let mut capacity_ns = 0usize;
    for cpu in 0..MAX_CPU_NUM {
        if !ONLINE_CPUS[cpu].load(Ordering::Acquire) {
            continue;
        }
        mask |= 1usize << cpu;
        count += 1;
        let since_ns = ONLINE_CPU_SINCE_NS[cpu].load(Ordering::Acquire);
        if since_ns != 0 {
            capacity_ns = capacity_ns.saturating_add(now_ns.saturating_sub(since_ns));
        }
    }
    (mask, count, capacity_ns)
}

/// Publish that a scheduler found no local work and is preparing to enter WFI.
pub(crate) fn mark_cpu_idle(cpu: usize) {
    if cpu < MAX_CPU_NUM {
        IDLE_CPUS[cpu].store(true, Ordering::Release);
    }
}

/// Withdraw the idle marker before processing scheduler work.
pub(crate) fn mark_cpu_active(cpu: usize) {
    if cpu < MAX_CPU_NUM {
        IDLE_CPUS[cpu].store(false, Ordering::Release);
    }
}

/// Return whether work was published after this CPU's last empty queue scan.
pub(crate) fn cpu_has_ready_tasks(cpu: usize) -> bool {
    cpu < MAX_CPU_NUM && READY_TASKS[cpu].load(Ordering::Acquire) != 0
}
#[allow(missing_docs)]
pub fn pid2process(pid: usize) -> Option<Arc<ProcessControlBlock>> {
    let map = PID2PCB.lock();
    map.get(&pid).map(Arc::clone)
}

pub fn processes_in_pgrp(pgid: usize) -> Vec<Arc<ProcessControlBlock>> {
    let processes = all_processes();
    processes
        .into_iter()
        .filter(|process| process.getpgid() == pgid)
        .collect()
}

pub fn all_processes() -> Vec<Arc<ProcessControlBlock>> {
    // Reserve outside PID2PCB: the global allocator can grow through the
    // frame allocator, so allocating while the PID table is locked creates a
    // PID -> heap -> frame lock chain.
    let initial_len = PID2PCB.lock().len();
    let mut processes = Vec::with_capacity(initial_len);
    loop {
        let map = PID2PCB.lock();
        let required = map.len();
        if required > processes.capacity() {
            drop(map);
            processes.reserve(required);
            continue;
        }
        for process in map.values() {
            // Capacity was checked while membership is stable under `map`, so
            // these pushes cannot enter the allocator.
            processes.push(Arc::clone(process));
        }
        drop(map);
        return processes;
    }
}

/// Take a non-blocking snapshot of the process table.
///
/// The global PID table only protects membership.  Callers must inspect PCB
/// state after this guard has been released so PID insertion/removal never
/// waits behind process/task locks or diagnostic walks.
pub(crate) fn try_all_processes() -> Option<Vec<Arc<ProcessControlBlock>>> {
    let initial_len = PID2PCB.try_lock()?.len();
    let mut processes = Vec::with_capacity(initial_len);
    loop {
        let map = PID2PCB.try_lock()?;
        let required = map.len();
        if required > processes.capacity() {
            drop(map);
            processes.reserve(required);
            continue;
        }
        for process in map.values() {
            processes.push(Arc::clone(process));
        }
        drop(map);
        return Some(processes);
    }
}

/// Return process-owned memory/file retention stats without blocking on PCB locks.
pub(crate) fn process_memory_retention_stats() -> ProcessMemoryRetentionStats {
    let Some(processes) = try_all_processes() else {
        return ProcessMemoryRetentionStats::lock_busy();
    };
    let mut stats = ProcessMemoryRetentionStats::empty(processes.len());
    for process in &processes {
        let pid = process.getpid();
        let strong_count = Arc::strong_count(process);
        if strong_count > stats.max_process_strong_count {
            stats.max_process_strong_count = strong_count;
            stats.max_process_strong_count_pid = pid;
        }
        let Some(inner) = process.try_inner_exclusive_access() else {
            stats.locked_processes += 1;
            continue;
        };
        if inner.is_zombie {
            stats.zombie_processes += 1;
        }
        let process_is_zombie = inner.is_zombie;
        stats.child_refs += inner.children.len();
        stats.fd_slots += inner.fd_table.len();
        let open_files = inner.fd_table.iter().filter(|fd| fd.is_some()).count();
        stats.open_files += open_files;
        if open_files > stats.max_open_files {
            stats.max_open_files = open_files;
            stats.max_open_files_pid = pid;
        }
        if inner.fd_table.len() > stats.max_fd_slots {
            stats.max_fd_slots = inner.fd_table.len();
            stats.max_fd_slots_pid = pid;
        }
        drop(inner);

        let Some(vm_set) = process.try_vm_exclusive_access() else {
            continue;
        };
        let mut process_frames = 0usize;
        for area in vm_set.areas.iter() {
            let frames = area.data_frames.len();
            stats.user_areas += 1;
            stats.user_data_frames += frames;
            process_frames += frames;
            match area.areatype() {
                UserMapAreaType::Elf => stats.elf_frames += frames,
                UserMapAreaType::Heap => stats.heap_frames += frames,
                UserMapAreaType::Stack | UserMapAreaType::TrapContext => {
                    stats.stack_frames += frames;
                }
                UserMapAreaType::Mmap => stats.mmap_frames += frames,
                UserMapAreaType::Shm => stats.shm_frames += frames,
                UserMapAreaType::RtSigreturnTrampoline => stats.other_frames += frames,
            }
        }
        if process_frames > stats.max_data_frames {
            stats.max_data_frames = process_frames;
            stats.max_data_frames_pid = pid;
            stats.max_data_frames_zombie = process_is_zombie;
        }
    }
    stats
}

pub fn insert_into_pid2process(pid: usize, process: Arc<ProcessControlBlock>) {
    // A replaced PCB may own tasks, VM state and files.  Never run its drop
    // chain while the global process-table lock is held.
    let replaced = PID2PCB.lock().insert(pid, process);
    drop(replaced);
}
#[allow(missing_docs)]
pub fn remove_from_pid2process(pid: usize) {
    // Dropping the table's Arc can transitively release process resources and
    // acquire subsystem locks, so move it out of the critical section first.
    let removed = PID2PCB.lock().remove(&pid);
    if removed.is_none() {
        panic!("cannot find pid {} in pid2task!", pid);
    }
    drop(removed);
}
#[allow(unused)]
pub fn queuelength() -> usize {
    READY_TASKS
        .iter()
        .map(|count| count.load(Ordering::Acquire))
        .sum()
}

/// Get the number of processes currently in the system
pub fn num_processes() -> usize {
    PID2PCB.lock().len()
}

#[allow(missing_docs)]
pub fn tid2task(tid: usize) -> Option<Arc<TaskControlBlock>> {
    let map = TID2TASK.lock();
    map.get(&tid).and_then(|weak| weak.upgrade())
}

#[allow(missing_docs)]
pub fn insert_into_tid2task(tid: usize, task: Arc<TaskControlBlock>) {
    TID2TASK.lock().insert(tid, Arc::downgrade(&task));
}

#[allow(missing_docs)]
pub fn remove_from_tid2task(tid: usize) {
    let mut map = TID2TASK.lock();
    map.remove(&tid);
}

#[allow(missing_docs)]
pub fn remove_from_tid2task_if_present(tid: usize) -> bool {
    TID2TASK.lock().remove(&tid).is_some()
}

#[allow(missing_docs)]
pub fn tid2task_stats() -> Tid2TaskStats {
    let Some(map) = TID2TASK.try_lock() else {
        return Tid2TaskStats {
            entries: 0,
            live: 0,
            dead: 0,
            lock_busy: true,
        };
    };
    let mut live = 0usize;
    let mut dead = 0usize;
    for task in map.values() {
        if task.upgrade().is_some() {
            live += 1;
        } else {
            dead += 1;
        }
    }
    Tid2TaskStats {
        entries: map.len(),
        live,
        dead,
        lock_busy: false,
    }
}

fn current_cpu() -> usize {
    #[cfg(target_arch = "riscv64")]
    {
        crate::sbi::get_tp()
    }
    #[cfg(target_arch = "loongarch64")]
    {
        crate::sbi_la::get_tp()
    }
    #[cfg(not(any(target_arch = "riscv64", target_arch = "loongarch64")))]
    {
        0
    }
}

fn valid_cpu(cpu: usize) -> usize {
    if cpu < MAX_CPU_NUM { cpu } else { 0 }
}

fn cpu_load(cpu: usize) -> usize {
    READY_TASKS[cpu].load(Ordering::Acquire)
        + usize::from(crate::task::processor::cpu_has_current_task(cpu))
}

fn select_enqueue_cpu(preferred_cpu: usize, affinity_mask: usize) -> usize {
    let preferred_cpu = valid_cpu(preferred_cpu);
    let now_ns = polyhal::timer::current_time().as_nanos() as usize;
    let allowed = affinity_mask & online_cpu_mask();
    let mut selected =
        if cpu_is_online(preferred_cpu) && affinity_mask & (1usize << preferred_cpu) != 0 {
            preferred_cpu
        } else {
            (0..MAX_CPU_NUM)
                .find(|cpu| allowed & (1usize << cpu) != 0)
                .unwrap_or(preferred_cpu)
        };
    let mut selected_load = cpu_load(selected);
    for offset in 0..MAX_CPU_NUM {
        let candidate = (preferred_cpu + offset) % MAX_CPU_NUM;
        if !cpu_is_online(candidate) || affinity_mask & (1usize << candidate) == 0 {
            continue;
        }
        // An idle CPU may legitimately keep the scheduler heartbeat unchanged
        // while sleeping in WFI. It is still an eligible target: enqueueing
        // uses the remote-mutation protocol and kick_remote_idle_cpu() sends the
        // reschedule IPI that brings it out of WFI. Only avoid a stale CPU that
        // has not published the idle state.
        if candidate != preferred_cpu
            && !IDLE_CPUS[candidate].load(Ordering::Acquire)
            && crate::task::processor::scheduler_cpu_stalled(candidate, now_ns)
        {
            continue;
        }
        let load = cpu_load(candidate);
        if load < selected_load {
            selected = candidate;
            selected_load = load;
        }
    }
    selected
}

fn select_wakeup_cpu(preferred_cpu: usize, affinity_mask: usize) -> usize {
    let preferred_cpu = valid_cpu(preferred_cpu);
    if !cpu_is_online(preferred_cpu) || affinity_mask & (1usize << preferred_cpu) == 0 {
        return select_enqueue_cpu(current_cpu(), affinity_mask);
    }
    let now_ns = polyhal::timer::current_time().as_nanos() as usize;
    if !IDLE_CPUS[preferred_cpu].load(Ordering::Acquire)
        && crate::task::processor::scheduler_cpu_stalled(preferred_cpu, now_ns)
    {
        return select_enqueue_cpu(current_cpu(), affinity_mask);
    }

    let selected = select_enqueue_cpu(preferred_cpu, affinity_mask);
    if selected == preferred_cpu
        || cpu_load(selected).saturating_add(WAKE_AFFINITY_LOAD_MARGIN) >= cpu_load(preferred_cpu)
    {
        preferred_cpu
    } else {
        selected
    }
}

fn cpu_is_online(cpu: usize) -> bool {
    ONLINE_CPUS[cpu].load(Ordering::Acquire)
}

/// Snapshot the CPUs that have entered their scheduler loop.
pub(crate) fn online_cpu_mask() -> usize {
    let mut mask = 0usize;
    for cpu in 0..MAX_CPU_NUM {
        if cpu_is_online(cpu) {
            mask |= 1usize << cpu;
        }
    }
    mask
}

pub(crate) fn idle_cpu_mask() -> usize {
    let mut mask = 0usize;
    for cpu in 0..MAX_CPU_NUM {
        if IDLE_CPUS[cpu].load(Ordering::Acquire) {
            mask |= 1usize << cpu;
        }
    }
    mask
}

fn decrement_ready_tasks(cpu: usize) {
    let previous = READY_TASKS[cpu]
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            Some(count.saturating_sub(1))
        })
        .unwrap();
    debug_assert!(previous > 0, "ready-task count underflow on CPU {cpu}");
}
