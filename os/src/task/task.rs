use super::id::TaskUserRes;
use super::{KernelStack, ProcessControlBlock, kstack_alloc, task_entry};
use crate::config::KERNEL_STACK_SIZE;
use crate::mm::VMSpace;
// use crate::trap::TrapContext;
// use crate::{mm::PhysPageNum, mm::address::*, sync::UPSafeCell};
use crate::sync::UPSafeCell;

use alloc::sync::{Arc, Weak};
use core::cell::RefMut;
use core::error;

use polyhal::kcontext::*;
use polyhal_trap::trapframe::*;
use polyhal_trap::trap::*;
pub use polyhal::utils::addr::*;

use log::{error, info, warn};
//use riscv::addr::VirtAddr;
#[allow(missing_docs)]
use alloc::string::String;
pub struct TaskControlBlock {
    // immutable
    pub process: Weak<ProcessControlBlock>,
    pub kstack: KernelStack,
    // mutable
    inner: UPSafeCell<TaskControlBlockInner>,
}

impl TaskControlBlock {
    #[allow(missing_docs)]
    pub fn inner_exclusive_access(&self) -> spin::MutexGuard<'_, TaskControlBlockInner> {
        self.inner.exclusive_access()
    }
    #[allow(missing_docs)]
    pub fn get_user_token(&self) -> usize {
        let process = self.process.upgrade().unwrap();
        let inner = process.inner_exclusive_access();
        inner.vm_set.token()
    }
}

pub struct TaskControlBlockInner {
    pub res: Option<TaskUserRes>,
    pub trap_cx: TrapFrame,
    pub task_cx: KContext,
    ///
    pub task_status: TaskStatus,
    pub exit_code: Option<i32>,
    ///线程退出时需要清零的用户态虚拟地址
    pub clear_child_tid: usize, 
}

impl TaskControlBlockInner {
    ///
    pub fn get_trap_cx(&self) -> &'static mut TrapFrame {
        // self.trap_cx_ppn.get_mut()
        let paddr = &self.trap_cx as *const TrapFrame as usize as *mut TrapFrame;

        unsafe { paddr.as_mut().unwrap()} 

    }

    #[allow(unused)]
    fn get_status(&self) -> TaskStatus {
        self.task_status
    }
}

impl TaskControlBlock {
    #[allow(missing_docs)]
    pub fn new(
        process: Arc<ProcessControlBlock>,
        ustack_base: usize,
        alloc_user_res: bool,
        kstack: KernelStack,
    ) -> Self {
        let res = TaskUserRes::new(Arc::clone(&process), ustack_base, alloc_user_res);
        // let trap_cx_ppn = res.trap_cx_ppn();
        // let kstack = kstack_alloc();
        // let kstack_top = kstack.get_top();
        let kstack_top = kstack.get_top();
        let kstack_bottom = kstack_top - KERNEL_STACK_SIZE;

        let mut kcontext = KContext::blank();
        kcontext[KContextArgs::KSP] = kstack_top;
        kcontext[KContextArgs::KPC] = task_entry as usize;

        if let Some(_pte) = process
            .inner_exclusive_access()
            .vm_set
            .translate(VirtAddr::from(kstack_bottom).floor())
        {
            warn!("success");
        } else {
            warn!("failed");
        }

        Self {
            process: Arc::downgrade(&process),
            kstack,
            inner: unsafe {
                UPSafeCell::new(TaskControlBlockInner {
                    res: Some(res),
                    trap_cx: TrapFrame::new(),
                    task_cx: kcontext,
                    task_status: TaskStatus::Ready,
                    exit_code: None,
                    clear_child_tid: 0,
                })
            },
        }
    }
}
#[allow(missing_docs)]
#[derive(Copy, Clone, PartialEq)]
///
pub enum TaskStatus {
    ///
    Ready,
    ///
    Running,
    Blocked,
    Zombie,
}
