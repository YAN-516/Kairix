use super::{ProcessControlBlock, TaskControlBlock, TaskStatus};
use crate::sync::SpinNoIrqLock;
use crate::sync::mutex::*;
use crate::task::suspend_current_and_run_next;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::sync::Arc;
use lazy_static::*;

lazy_static! {
    pub static ref TASK_MANAGER: SpinNoIrqLock<TaskManager> =
        SpinNoIrqLock::new(TaskManager::new());
    pub static ref PID2PCB: SpinNoIrqLock<BTreeMap<usize, Arc<ProcessControlBlock>>> =
        SpinNoIrqLock::new(BTreeMap::new());
}
pub struct TaskManager {
    ready_queue: VecDeque<Arc<TaskControlBlock>>,
}

/// A simple FIFO scheduler.
impl TaskManager {
    pub fn new() -> Self {
        Self {
            ready_queue: VecDeque::new(),
        }
    }
    pub fn add(&mut self, task: Arc<TaskControlBlock>) {
        self.ready_queue.push_back(task);
    }
    pub fn fetch(&mut self) -> Option<Arc<TaskControlBlock>> {
        self.ready_queue.pop_front()
    }
    pub fn remove(&mut self, task: Arc<TaskControlBlock>) {
        if let Some((id, _)) = self
            .ready_queue
            .iter()
            .enumerate()
            .find(|(_, t)| Arc::as_ptr(t) == Arc::as_ptr(&task))
        {
            self.ready_queue.remove(id);
        }
    }
}

#[allow(missing_docs)]
pub fn add_task(task: Arc<TaskControlBlock>) {
    TASK_MANAGER.lock().add(task);
}

pub fn add_task_front(task: Arc<TaskControlBlock>) {
    let mut manager = TASK_MANAGER.lock();
    manager.ready_queue.push_front(task);
}
#[allow(missing_docs)]
pub fn wakeup_task(task: Arc<TaskControlBlock>) {
    let mut task_inner = task.inner_exclusive_access();
    // 避免与 suspend_current_and_run_next 竞态导致重复入队
    if task_inner.task_status == TaskStatus::Ready || task_inner.task_status == TaskStatus::Running {
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
    TASK_MANAGER.lock().remove(task);
}

pub fn fetch_task() -> Option<Arc<TaskControlBlock>> {
    TASK_MANAGER.lock().fetch()
}
#[allow(missing_docs)]
pub fn pid2process(pid: usize) -> Option<Arc<ProcessControlBlock>> {
    let map = PID2PCB.lock();
    map.get(&pid).map(Arc::clone)
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
    TASK_MANAGER.lock().ready_queue.len()
}

/// Get the number of processes currently in the system
pub fn num_processes() -> usize {
    PID2PCB.lock().len()
}
