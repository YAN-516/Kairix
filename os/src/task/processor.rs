// use super::__switch;
use super::{ProcessControlBlock, TaskControlBlock};
use super::{TaskStatus, fetch_task};
use crate::config::MAX_CPU_NUM;
use crate::mm::VMSpace;
use crate::set_init_completed;
use crate::sync::SpinNoIrqLock;
use crate::task::check_timers;
// use crate::trap::{TrapContext, trap_handler, trap_return};
use super::task_entry;
#[cfg(target_arch = "riscv64")]
use crate::sbi::*;
use crate::wait_for_init;
use alloc::sync::Arc;
use polyhal::kcontext::{KContext, context_switch};
use polyhal_trap::trapframe::TrapFrame;

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
    pub fn take_current(&mut self) -> Option<Arc<TaskControlBlock>> {
        self.current.take()
    }
    pub fn current(&self) -> Option<Arc<TaskControlBlock>> {
        self.current.as_ref().map(Arc::clone)
    }
}

pub static mut PROCESSORS: [Option<SpinNoIrqLock<Processor>>; MAX_CPU_NUM] =
    [const { None }; MAX_CPU_NUM];
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
    unsafe {
        for cpu in 0..MAX_CPU_NUM {
            if let Some(processor) = PROCESSORS[cpu].as_ref() {
                if let Some(processor) = processor.try_lock() {
                    if processor.current.is_some() {
                        current_tasks += 1;
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
    }
}
#[allow(missing_docs)]
pub fn run_tasks() {
    let id: usize = get_tp();
    //println!("cpu {} run tasks", id);
    if id == 0 {
        set_init_completed();
        // loop{}
    }
    loop {
        crate::task::reap_deferred_exited_tasks();
        check_timers();
        unsafe {
            if let Some(task) = fetch_task(id) {
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
                    continue;
                }
                //println!("cpu {} get processor", id);
                let mut processor = PROCESSORS[id].as_mut().unwrap().lock();
                //println!("cpu {} get processor success", id);
                let mut task_inner = task.inner_exclusive_access();
                if task_inner.task_status == TaskStatus::Zombie {
                    drop(task_inner);
                    processor.current = None;
                    continue;
                }
                if task_inner.task_status != TaskStatus::Ready {
                    drop(task_inner);
                    processor.current = None;
                    continue;
                }
                if !task.try_mark_on_cpu(id) {
                    drop(task_inner);
                    processor.current = None;
                    drop(processor);
                    crate::task::add_task_to_cpu(task, id);
                    continue;
                }
                let idle_task_cx_ptr = processor.get_idle_task_cx_ptr();
                // access coming task TCB exclusively
                let next_task_cx_ptr = &task_inner.task_cx as *const KContext;
                task_inner.task_status = TaskStatus::Running;
                //println!("pid:{}", task.process.upgrade().unwrap().getpid());
                drop(task_inner);
                // release coming task TCB manually
                processor.current = Some(task);
                // release processor manually
                drop(processor);
                // Use the cloned task instead of calling current_task() to avoid extra lock acquisition

                let process = match task_clone.process.upgrade() {
                    Some(p) => p,
                    None => {
                        // PCB has been freed (e.g. process killed by signal and reaped by waitpid),
                        // but this orphan task is still in the ready queue. Drop it and continue.
                        let mut processor = PROCESSORS[id].as_mut().unwrap().lock();
                        processor.current = None;
                        task_clone.clear_on_cpu();
                        continue;
                    }
                };

                process.inner_exclusive_access().vm_set.activate();

                context_switch(idle_task_cx_ptr, next_task_cx_ptr);
                task_clone.clear_on_cpu();
                let pending_wakeup = {
                    let mut task_inner = task_clone.inner_exclusive_access();
                    let pending = task_inner.pending_wakeup;
                    if pending {
                        task_inner.pending_wakeup = false;
                        if task_inner.task_status != TaskStatus::Zombie {
                            task_inner.task_status = TaskStatus::Ready;
                        }
                    }
                    pending
                };
                if pending_wakeup {
                    crate::task::add_task_to_cpu(task_clone, id);
                }
            } else {
                // warn!("cpu {}: no tasks available in run_tasks", id);
            }
        }
    }
}
#[allow(missing_docs)]
pub fn take_current_task() -> Option<Arc<TaskControlBlock>> {
    let id: usize = get_tp();
    unsafe { PROCESSORS[id].as_mut().unwrap().lock().take_current() }
}
#[allow(missing_docs)]
pub fn current_task() -> Option<Arc<TaskControlBlock>> {
    let id: usize = get_tp();
    unsafe { PROCESSORS[id].as_mut().unwrap().lock().current() }
}
#[allow(missing_docs)]
pub fn set_current_task(task: Arc<TaskControlBlock>) {
    let id: usize = get_tp();
    unsafe {
        PROCESSORS[id].as_mut().unwrap().lock().current = Some(task);
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
    unsafe {
        let mut processor = PROCESSORS[id].as_mut().unwrap().lock();
        let idle_task_cx_ptr = processor.get_idle_task_cx_ptr();
        drop(processor);
        context_switch(switched_task_cx_ptr, idle_task_cx_ptr);
    }
}
