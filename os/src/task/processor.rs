// use super::__switch;
use super::{ProcessControlBlock, TaskControlBlock};
use super::{TaskStatus, fetch_task};
use crate::config::MAX_CPU_NUM;
use crate::mm::{VMSpace, KERNEL_VMSET};
use crate::sync::SpinNoIrqLock;
use crate::task::id;
use crate::task::manager::queuelength;
// use crate::trap::{TrapContext, trap_handler, trap_return};
#[cfg(target_arch = "riscv64")]
use crate::sbi::*;
use alloc::sync::Arc;
use polyhal::consts::KERNEL_STACK_SIZE;
use polyhal::pagetable::TLB;
use polyhal::print;
use polyhal::utils::addr::{PhysPageNum, VirtPageNum};
use core::arch::asm;
use lazy_static::*;
use log::{error, warn};
use polyhal::kcontext::{KContext, context_switch};
use polyhal_trap::trapframe::TrapFrame;
use polyhal_trap::trapframe::TrapFrameArgs;
use polyhal::VirtAddr;
use crate::check_timers;
use polyhal::println;
use super::task_entry;

#[cfg(target_arch = "loongarch64")]
use crate::sbi_la::*;

pub struct Processor {
    current: Option<Arc<TaskControlBlock>>,
    idle_task_cx: KContext,
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
#[allow(missing_docs)]
pub fn run_tasks() {
    let id: usize = get_tp();
    loop {
        unsafe {
            if let Some(task) = fetch_task() {
                let mut processor = PROCESSORS[id].as_mut().unwrap().lock();
                let _idle_task_cx_ptr = processor.get_idle_task_cx_ptr();
                // access coming task TCB exclusively
                let mut task_inner = task.inner_exclusive_access();
                let _next_task_cx_ptr = &task_inner.task_cx as *const KContext;
                task_inner.task_status = TaskStatus::Running;
                //println!("pid:{}", task.process.upgrade().unwrap().getpid());
                drop(task_inner);
                // release coming task TCB manually
                processor.current = Some(task);
                // release processor manually
                drop(processor);

                //println!("cpu {} run task", id);
                // //切换页表
                // let task_satp = current_user_token();
                // println!("task satp: {:#x}", task_satp);
                // riscv::register::satp::write(task_satp);
                // asm!("sfence.vma");
                let current_task = current_task().unwrap();
                let process = match current_task.process.upgrade() {
                    Some(p) => p,
                    None => {
                        // PCB has been freed (e.g. process killed by signal and reaped by waitpid),
                        // but this orphan task is still in the ready queue. Drop it and continue.
                        let mut processor = PROCESSORS[id].as_mut().unwrap().lock();
                        processor.current = None;
                        continue;
                    }
                };
                process.inner_exclusive_access().vm_set.activate();
                // KERNEL_VMSET.lock().activate();
                // let trap_cx = &current_task.inner_exclusive_access().trap_cx;
                // warn!("trap_cx {:#x?}", trap_cx );
                // warn!("idle kcontext {:#x?}", *next_task_cx_ptr );
                // warn!("task entry {:#x}", task_entry as usize);
                // let pgdl: usize;
                // core::arch::asm!("csrrd {}, 0x1B", out(reg) pgdl);
                // error!("PGDL = 0x{:016x}", pgdl);
                // warn!("trap_cx sp {:#x}", trap_cx[TrapFrameArgs::SP] );

                // warn!("kcontext sp {:#x}", (*next_task_cx_ptr).sp());
                // warn!("kcontext ra {:#x}", (*next_task_cx_ptr).ra());
                // warn!("kcontext {:?}", *next_task_cx_ptr);
                // let _sp = (*next_task_cx_ptr).sp()

                // for pte in PhysPageNum(task_satp).get_pte_array(){
                //     println!("{:#x}", pte.0);
                // }

    //             let test_va = 0x3ffffdf000usize;

    // // 尝试写入一个魔数
    // let ptr = test_va as *mut u64;
    // core::ptr::write_volatile(ptr, 0xdeadbeefcafebabe);
    
    // // 尝试读回
    // let val = core::ptr::read_volatile(ptr);
    // error!("Write test: wrote 0xdeadbeefcafebabe, read 0x{:016x}", val);
                // println!("pgtb change success");
                //println!("satp:  {:#x}", task_satp);
                //warn!("switching to task");
                // __switch(idle_task_cx_ptr, next_task_cx_ptr);
                // error!("asdj");
                context_switch(_idle_task_cx_ptr, _next_task_cx_ptr);
            } else {
                check_timers();
                warn!("cpu {}: no tasks available in run_tasks", id);
            }
        }
    }
}
#[allow(missing_docs)]
pub fn take_current_task() -> Option<Arc<TaskControlBlock>> {
    let id: usize = get_tp();
    unsafe {
        PROCESSORS[id]
            .as_mut()
            .unwrap()
            .lock()
            .take_current()
    }
}
#[allow(missing_docs)]
pub fn current_task() -> Option<Arc<TaskControlBlock>> {
    let id: usize = get_tp();
    unsafe {
        PROCESSORS[id]
            .as_mut()
            .unwrap()
            .lock()
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
    let id: usize = get_tp();
    check_timers();
    unsafe {
        let mut processor = PROCESSORS[id].as_mut().unwrap().lock();
        let idle_task_cx_ptr = processor.get_idle_task_cx_ptr();
        drop(processor);
        context_switch(switched_task_cx_ptr, idle_task_cx_ptr);
    }
}
