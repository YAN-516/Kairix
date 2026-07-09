use super::task::{MLFQ_BOTTOM_LEVEL, MLFQ_LEVELS};
use super::{ProcessControlBlock, TaskControlBlock, TaskStatus};
use crate::config::MAX_CPU_NUM;
use crate::mm::UserMapAreaType;
use crate::sync::SpinNoIrqLock;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use lazy_static::*;
use log::warn;

lazy_static! {
    pub static ref TASK_MANAGER: [SpinNoIrqLock<TaskManager>; MAX_CPU_NUM] =
        core::array::from_fn(|_| SpinNoIrqLock::new(TaskManager::new()));
    pub static ref PID2PCB: SpinNoIrqLock<BTreeMap<usize, Arc<ProcessControlBlock>>> =
        SpinNoIrqLock::new(BTreeMap::new());
    /// 全局 TID -> TaskControlBlock 映射（弱引用，由 process.tasks 保持强引用）
    pub static ref TID2TASK: SpinNoIrqLock<BTreeMap<usize, Weak<TaskControlBlock>>> =
        SpinNoIrqLock::new(BTreeMap::new());
    /// 维护设置了 alarm/itimer 的进程，避免 timer 中断遍历所有进程
    pub static ref TIMER_PROCS: SpinNoIrqLock<BTreeMap<usize, Weak<ProcessControlBlock>>> =
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
    sched_epoch: usize,
    aging_cursor_level: usize,
}

const MLFQ_AGING_SCAN_BUDGET: usize = 32;

/// Multi-level feedback queues with round-robin order inside each level.
impl TaskManager {
    pub fn new() -> Self {
        Self {
            ready_queues: core::array::from_fn(|_| VecDeque::new()),
            sched_epoch: 0,
            aging_cursor_level: 1,
        }
    }
    fn queue_index(task: &TaskControlBlock) -> usize {
        task.mlfq_level().min(MLFQ_BOTTOM_LEVEL)
    }
    fn add(&mut self, task: Arc<TaskControlBlock>) {
        task.note_mlfq_enqueued(self.sched_epoch);
        let level = Self::queue_index(&task);
        self.ready_queues[level].push_back(task);
    }
    fn add_front(&mut self, task: Arc<TaskControlBlock>) {
        task.note_mlfq_enqueued(self.sched_epoch);
        let level = Self::queue_index(&task);
        self.ready_queues[level].push_front(task);
    }
    fn fetch(&mut self) -> Option<Arc<TaskControlBlock>> {
        self.sched_epoch = self.sched_epoch.wrapping_add(1);
        self.age_queued_tasks();
        for level in 0..MLFQ_LEVELS {
            if let Some(task) = self.ready_queues[level].pop_front() {
                return Some(task);
            }
        }
        None
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
                let task = self.ready_queues[level].pop_front().unwrap();
                let new_level = level - 1;
                task.set_mlfq_level(new_level);
                task.note_mlfq_enqueued(self.sched_epoch);
                self.ready_queues[new_level].push_back(task);
                promoted += 1;
            }
        }
    }
    fn remove(&mut self, task: &Arc<TaskControlBlock>) -> bool {
        for queue in self.ready_queues.iter_mut() {
            if let Some((id, _)) = queue
                .iter()
                .enumerate()
                .find(|(_, t)| Arc::as_ptr(t) == Arc::as_ptr(task))
            {
                queue.remove(id);
                return true;
            }
        }
        false
    }
    pub fn len(&self) -> usize {
        self.ready_queues.iter().map(VecDeque::len).sum()
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
    add_task_to_cpu(task, current_cpu());
}

pub fn add_task_front(task: Arc<TaskControlBlock>) {
    add_task_to_cpu_front(task, current_cpu());
}

pub fn add_task_to_cpu(task: Arc<TaskControlBlock>, cpu: usize) {
    {
        let task_inner = task.inner_exclusive_access();
        if task_inner.task_status != TaskStatus::Ready {
            return;
        }
    }
    let cpu = valid_cpu(cpu);
    if !task.try_mark_ready_queued(cpu) {
        return;
    }

    {
        let task_inner = task.inner_exclusive_access();
        if task_inner.task_status != TaskStatus::Ready {
            task.clear_ready_queued();
            return;
        }
    }

    {
        let mut manager = TASK_MANAGER[cpu].lock();
        manager.add(task);
    }
}

pub fn add_task_to_cpu_front(task: Arc<TaskControlBlock>, cpu: usize) {
    {
        let task_inner = task.inner_exclusive_access();
        if task_inner.task_status != TaskStatus::Ready {
            return;
        }
    }
    let cpu = valid_cpu(cpu);
    if !task.try_mark_ready_queued(cpu) {
        return;
    }

    {
        let task_inner = task.inner_exclusive_access();
        if task_inner.task_status != TaskStatus::Ready {
            task.clear_ready_queued();
            return;
        }
    }

    {
        let mut manager = TASK_MANAGER[cpu].lock();
        manager.add_front(task);
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
    warn!(
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
    if task.is_on_cpu() {
        task_inner.pending_wakeup = true;
        if task_inner.task_status != TaskStatus::Running {
            task_inner.task_status = TaskStatus::Ready;
        }
        warn!(
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
        warn!(
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
            add_task(task);
        }
        return;
    }
    task.boost_mlfq_level();
    task_inner.task_status = TaskStatus::Ready;
    drop(task_inner);
    warn!(
        "[IOZONE_HANG wakeup_enqueue] cpu={} pid={:?} global_tid={} status_before={:?}",
        current_cpu(),
        pid,
        global_tid,
        status_before
    );
    add_task(task);
}

#[allow(missing_docs)]
pub fn remove_task(task: Arc<TaskControlBlock>) {
    task.clear_ready_queued();
    for manager in TASK_MANAGER.iter() {
        if manager.lock().remove(&task) {
            break;
        }
    }
}

fn fetch_task_from_cpu(cpu: usize) -> Option<Arc<TaskControlBlock>> {
    let task = {
        let mut manager = TASK_MANAGER[cpu].lock();
        manager.fetch()
    };
    if let Some(task) = task {
        task.clear_ready_queued();
        Some(task)
    } else {
        None
    }
}

pub fn fetch_task(cpu: usize) -> Option<Arc<TaskControlBlock>> {
    let cpu = valid_cpu(cpu);
    fetch_task_from_cpu(cpu)
}

pub fn ready_queue_lengths() -> [usize; MAX_CPU_NUM] {
    core::array::from_fn(|cpu| TASK_MANAGER[cpu].lock().len())
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
    let map = PID2PCB.lock();
    map.values().map(Arc::clone).collect()
}

/// Return process-owned memory/file retention stats without allocating.
pub(crate) fn process_memory_retention_stats() -> ProcessMemoryRetentionStats {
    let Some(map) = PID2PCB.try_lock() else {
        return ProcessMemoryRetentionStats::lock_busy();
    };
    let mut stats = ProcessMemoryRetentionStats::empty(map.len());
    for (pid, process) in map.iter() {
        let strong_count = Arc::strong_count(process);
        if strong_count > stats.max_process_strong_count {
            stats.max_process_strong_count = strong_count;
            stats.max_process_strong_count_pid = *pid;
        }
        let Some(inner) = process.try_inner_exclusive_access() else {
            stats.locked_processes += 1;
            continue;
        };
        if inner.is_zombie {
            stats.zombie_processes += 1;
        }
        stats.child_refs += inner.children.len();
        stats.fd_slots += inner.fd_table.len();
        let open_files = inner.fd_table.iter().filter(|fd| fd.is_some()).count();
        stats.open_files += open_files;
        if open_files > stats.max_open_files {
            stats.max_open_files = open_files;
            stats.max_open_files_pid = *pid;
        }
        if inner.fd_table.len() > stats.max_fd_slots {
            stats.max_fd_slots = inner.fd_table.len();
            stats.max_fd_slots_pid = *pid;
        }

        let mut process_frames = 0usize;
        for area in inner.vm_set.areas.iter() {
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
            stats.max_data_frames_pid = *pid;
            stats.max_data_frames_zombie = inner.is_zombie;
        }
    }
    stats
}

pub fn insert_into_pid2process(pid: usize, process: Arc<ProcessControlBlock>) {
    PID2PCB.lock().insert(pid, process);
}
#[allow(missing_docs)]
pub fn remove_from_pid2process(pid: usize) {
    let mut map = PID2PCB.lock();
    if map.remove(&pid).is_none() {
        panic!("cannot find pid {} in pid2task!", pid);
    }
}
#[allow(unused)]
pub fn queuelength() -> usize {
    TASK_MANAGER.iter().map(|queue| queue.lock().len()).sum()
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
