use super::task::{MLFQ_BOTTOM_LEVEL, MLFQ_LEVELS};
use super::{ProcessControlBlock, TaskControlBlock, TaskStatus};
use crate::config::MAX_CPU_NUM;
use crate::sync::SpinNoIrqLock;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use lazy_static::*;

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

pub struct TaskManager {
    ready_queues: [VecDeque<Arc<TaskControlBlock>>; MLFQ_LEVELS],
}

/// Multi-level feedback queues with round-robin order inside each level.
impl TaskManager {
    pub fn new() -> Self {
        Self {
            ready_queues: core::array::from_fn(|_| VecDeque::new()),
        }
    }
    fn queue_index(task: &TaskControlBlock) -> usize {
        task.mlfq_level().min(MLFQ_BOTTOM_LEVEL)
    }
    fn add(&mut self, task: Arc<TaskControlBlock>) {
        task.note_mlfq_enqueued();
        let level = Self::queue_index(&task);
        self.ready_queues[level].push_back(task);
    }
    fn add_front(&mut self, task: Arc<TaskControlBlock>) {
        task.note_mlfq_enqueued();
        let level = Self::queue_index(&task);
        self.ready_queues[level].push_front(task);
    }
    fn fetch(&mut self) -> Option<Arc<TaskControlBlock>> {
        self.age_queued_tasks();
        for level in 0..MLFQ_LEVELS {
            if let Some(task) = self.ready_queues[level].pop_front() {
                task.reset_mlfq_wait_ticks();
                return Some(task);
            }
        }
        None
    }
    fn age_queued_tasks(&mut self) {
        for level in 1..MLFQ_LEVELS {
            let len = self.ready_queues[level].len();
            for _ in 0..len {
                let Some(task) = self.ready_queues[level].pop_front() else {
                    break;
                };
                if task.age_mlfq_wait_tick() {
                    let new_level = level - 1;
                    task.set_mlfq_level(new_level);
                    task.reset_mlfq_wait_ticks();
                    self.ready_queues[new_level].push_back(task);
                } else {
                    self.ready_queues[level].push_back(task);
                }
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
    if task_inner.task_status == TaskStatus::Zombie {
        return;
    }
    if task.is_on_cpu() {
        task_inner.pending_wakeup = true;
        if task_inner.task_status != TaskStatus::Running {
            task_inner.task_status = TaskStatus::Ready;
        }
        drop(task_inner);
        return;
    }
    if task_inner.task_status == TaskStatus::Running {
        task_inner.pending_wakeup = true;
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
