use super::__switch;
use super::{ProcessControlBlock, TaskContext, TaskControlBlock};
use super::{TaskStatus, fetch_task};
use crate::config::{KERNEL_STACK_SIZE, MAX_CPU_NUM};
use crate::sync::UPSafeCell;
use crate::task::id;
use crate::task::manager::queuelength;
use crate::trap::{TrapContext, trap_handler, trap_return};
use alloc::sync::Arc;
use core::arch::asm;
use lazy_static::*;
use log::{error, warn};

pub struct Processor {
    current: Option<Arc<TaskControlBlock>>,
    idle_task_cx: TaskContext,
}
impl Processor {
    pub fn new() -> Self {
        Self {
            current: None,
            idle_task_cx: TaskContext::zero_init(),
        }
    }
    fn get_idle_task_cx_ptr(&mut self) -> *mut TaskContext {
        &mut self.idle_task_cx as *mut _
    }
    pub fn take_current(&mut self) -> Option<Arc<TaskControlBlock>> {
        self.current.take()
    }
    pub fn current(&self) -> Option<Arc<TaskControlBlock>> {
        self.current.as_ref().map(Arc::clone)
    }
}

pub static mut PROCESSORS: [Option<UPSafeCell<Processor>>; MAX_CPU_NUM] =
    [const { None }; MAX_CPU_NUM];
pub fn init_processors() {
    unsafe {
        for i in 0..MAX_CPU_NUM {
            PROCESSORS[i] = Some(UPSafeCell::new(Processor::new()));
        }
    }
}
#[allow(missing_docs)]
pub fn run_tasks() {
    let id: usize = crate::sbi::get_tp();

    loop {
        unsafe {
            if let Some(task) = fetch_task() {
                if id == 1 {
                    warn!(
                        "cpu {}: run_tasks loop, queue length: {}",
                        id,
                        queuelength()
                    );
                }

                //println!("cpu {} get one task", id);
                let mut processor = PROCESSORS[id].as_mut().unwrap().exclusive_access();
                let idle_task_cx_ptr = processor.get_idle_task_cx_ptr();
                // access coming task TCB exclusively
                let mut task_inner = task.inner_exclusive_access();
                let next_task_cx_ptr = &task_inner.task_cx as *const TaskContext;
                task_inner.task_status = TaskStatus::Running;

                drop(task_inner);
                // release coming task TCB manually
                processor.current = Some(task);
                // release processor manually
                drop(processor);
                // //切换页表
                let task_satp = current_user_token();

                // println!("task satp: {:#x}", task_satp);
                riscv::register::satp::write(task_satp);
                asm!("sfence.vma");
                //println!("satp:  {:#x}", task_satp);
                //warn!("switching to task");
                __switch(idle_task_cx_ptr, next_task_cx_ptr);
            } else {
                //warn!("cpu {}: no tasks available in run_tasks", id);
            }
        }
    }
}
#[allow(missing_docs)]
pub fn take_current_task() -> Option<Arc<TaskControlBlock>> {
    let id: usize = crate::sbi::get_tp();
    unsafe {
        PROCESSORS[id]
            .as_mut()
            .unwrap()
            .exclusive_access()
            .take_current()
    }
}
#[allow(missing_docs)]
pub fn current_task() -> Option<Arc<TaskControlBlock>> {
    let id: usize = crate::sbi::get_tp();
    unsafe {
        PROCESSORS[id]
            .as_mut()
            .unwrap()
            .exclusive_access()
            .current()
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
pub fn current_trap_cx() -> &'static mut TrapContext {
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
pub fn schedule(switched_task_cx_ptr: *mut TaskContext) {
    let id: usize = crate::sbi::get_tp();
    unsafe {
        let mut processor = PROCESSORS[id].as_mut().unwrap().exclusive_access();
        let idle_task_cx_ptr = processor.get_idle_task_cx_ptr();
        drop(processor);
        __switch(switched_task_cx_ptr, idle_task_cx_ptr);
    }
}
