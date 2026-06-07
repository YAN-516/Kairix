use super::{ProcessControlBlock, TaskControlBlock, TaskStatus};
use crate::sync::SpinNoIrqLock;
use crate::sync::mutex::*;
use crate::task::suspend_current_and_run_next;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use lazy_static::*;

const MAX_SCHED_PRIORITY: usize = 99;
const HIGH_PRIORITY_BUDGET: usize = 32;

lazy_static! {
    pub static ref TASK_MANAGER: SpinNoIrqLock<TaskManager> =
        SpinNoIrqLock::new(TaskManager::new());
    pub static ref PID2PCB: SpinNoIrqLock<BTreeMap<usize, Arc<ProcessControlBlock>>> =
        SpinNoIrqLock::new(BTreeMap::new());
    /// 全局 TID -> TaskControlBlock 映射（弱引用，由 process.tasks 保持强引用）
    pub static ref TID2TASK: SpinNoIrqLock<BTreeMap<usize, Weak<TaskControlBlock>>> =
        SpinNoIrqLock::new(BTreeMap::new());
    /// 维护设置了 alarm/itimer 的进程，避免 timer 中断遍历所有进程
    pub static ref TIMER_PROCS: SpinNoIrqLock<BTreeMap<usize, Weak<ProcessControlBlock>>> =
        SpinNoIrqLock::new(BTreeMap::new());
    /// 维护开启了内核 watchdog 的进程，主要用于避免 LTP 卡死用例阻塞测试流程
    pub static ref WATCHDOG_PROCS: SpinNoIrqLock<BTreeMap<usize, Weak<ProcessControlBlock>>> =
        SpinNoIrqLock::new(BTreeMap::new());
}
pub struct TaskManager {
    ready_queues: [VecDeque<Arc<TaskControlBlock>>; MAX_SCHED_PRIORITY + 1],
    high_priority_runs: usize,
}

/// Priority buckets with FIFO order inside each bucket.
impl TaskManager {
    pub fn new() -> Self {
        Self {
            ready_queues: core::array::from_fn(|_| VecDeque::new()),
            high_priority_runs: 0,
        }
    }
    fn queue_index(task: &TaskControlBlock) -> usize {
        task.sched_priority().clamp(0, MAX_SCHED_PRIORITY as i32) as usize
    }
    pub fn add(&mut self, task: Arc<TaskControlBlock>) {
        let priority = Self::queue_index(&task);
        self.ready_queues[priority].push_back(task);
    }
    pub fn add_front(&mut self, task: Arc<TaskControlBlock>) {
        let priority = Self::queue_index(&task);
        self.ready_queues[priority].push_front(task);
    }
    pub fn fetch(&mut self) -> Option<Arc<TaskControlBlock>> {
        if self.high_priority_runs >= HIGH_PRIORITY_BUDGET {
            if let Some(task) = self.ready_queues[0].pop_front() {
                self.high_priority_runs = 0;
                return Some(task);
            }
            self.high_priority_runs = 0;
        }
        for priority in (0..=MAX_SCHED_PRIORITY).rev() {
            if let Some(task) = self.ready_queues[priority].pop_front() {
                if priority > 0 {
                    self.high_priority_runs += 1;
                } else {
                    self.high_priority_runs = 0;
                }
                return Some(task);
            }
        }
        None
    }
    pub fn remove(&mut self, task: Arc<TaskControlBlock>) {
        for queue in self.ready_queues.iter_mut() {
            if let Some((id, _)) = queue
                .iter()
                .enumerate()
                .find(|(_, t)| Arc::as_ptr(t) == Arc::as_ptr(&task))
            {
                queue.remove(id);
                break;
            }
        }
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
    if task.inner_exclusive_access().task_status != TaskStatus::Ready {
        return;
    }
    if !task.try_mark_ready_queued() {
        return;
    }
    TASK_MANAGER.lock().add(task);
}

pub fn add_task_front(task: Arc<TaskControlBlock>) {
    if task.inner_exclusive_access().task_status != TaskStatus::Ready {
        return;
    }
    if !task.try_mark_ready_queued() {
        return;
    }
    let mut manager = TASK_MANAGER.lock();
    manager.add_front(task);
}
#[allow(missing_docs)]
pub fn wakeup_task(task: Arc<TaskControlBlock>) {
    let _process_is_zombie = task
        .process
        .upgrade()
        .map(|process| process.inner_exclusive_access().is_zombie)
        .unwrap_or(true);
    let mut task_inner = task.inner_exclusive_access();
    if task_inner.task_status == TaskStatus::Zombie {
        return;
    }
    // 避免与 suspend_current_and_run_next 竞态导致重复入队
    if task_inner.task_status == TaskStatus::Ready || task_inner.task_status == TaskStatus::Running
    {
        // 任务还在 Running/Ready，但已经有人在它阻塞前发了唤醒。
        // 设置 pending_wakeup 标志，让 block_current_and_run_next 看到后不阻塞。
        task_inner.pending_wakeup = true;
        drop(task_inner);
        return;
    }
    task_inner.task_status = TaskStatus::Ready;
    drop(task_inner);
    add_task(task);
    // suspend_current_and_run_next();
}
#[allow(missing_docs)]
pub fn remove_task(task: Arc<TaskControlBlock>) {
    task.clear_ready_queued();
    TASK_MANAGER.lock().remove(task);
}

pub fn fetch_task() -> Option<Arc<TaskControlBlock>> {
    let task = TASK_MANAGER.lock().fetch()?;
    task.clear_ready_queued();
    Some(task)
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
    TASK_MANAGER.lock().len()
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
