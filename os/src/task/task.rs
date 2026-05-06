use super::id::TaskUserRes;
use super::{KernelStack, ProcessControlBlock, kstack_alloc, task_entry};
// use crate::config::KERNEL_STACK_SIZE;
use crate::mm::VMSpace;
// use crate::trap::TrapContext;
// use crate::{mm::PhysPageNum, mm::address::*, sync::UPSafeCell};
use crate::sync::SpinNoIrqLock;
use crate::task::processor::PROCESSORS;

use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::cell::RefMut;
use core::error;
use core::sync::atomic::{AtomicBool, Ordering};

use polyhal::consts::*;
use polyhal::kcontext::*;
pub use polyhal::utils::addr::*;
use polyhal_trap::trap::*;
use polyhal_trap::trapframe::*;

use log::{error, info, warn};
//use riscv::addr::VirtAddr;
#[allow(missing_docs)]
use alloc::string::String;
pub struct TaskControlBlock {
    // immutable
    pub process: Weak<ProcessControlBlock>,
    pub kstack: KernelStack,
    // mutable
    inner: SpinNoIrqLock<TaskControlBlockInner>,
}

impl TaskControlBlock {
    #[allow(missing_docs)]
    pub fn inner_exclusive_access(
        &self,
    ) -> crate::sync::SpinMutexGuard<'_, TaskControlBlockInner, crate::sync::SpinNoIrq> {
        self.inner.lock()
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
    /// 信号处理时保存的原始 TrapFrame
    pub saved_sigtrapframe: Option<TrapFrame>,
    /// 标记该线程是否被信号中断唤醒（用于阻塞系统调用返回 EINTR）
    pub interrupted_by_signal: bool,
    /// 线程级待处理信号（用于 tkill/tgkill 等线程定向信号）
    pub pending_signals: crate::task::signal::SignalSet,
    /// 线程级信号阻塞掩码
    pub blocked_signals: crate::task::signal::SignalSet,
    /// 是否需要处理信号
    pub need_signal_handle: bool,
    /// 信号处理上下文栈（用于线程自定义 handler 返回）
    pub sig_context_stack: Vec<(TrapFrame, crate::task::signal::SignalSet)>,
    /// sigsuspend 保存的旧信号掩码，sigreturn 后恢复
    pub sigsuspend_old_mask: Option<crate::task::signal::SignalSet>,
    /// 标记该线程是否已被 futex_wake 唤醒（防止丢失唤醒）
    pub futex_woken: bool,
    /// 标记该线程是否有待处理的唤醒（解决 lost wakeup race）
    pub pending_wakeup: bool,
    /// robust_list_head 指针（set_robust_list 设置）
    pub robust_list_head: usize,
    /// robust_list 长度（通常为 24 字节）
    pub robust_list_len: usize,
    /// 标记所属进程是否已被 SIGKILL 等标记为 zombie（避免 block 时竞态）
    pub zombie_flag: AtomicBool,
}

impl TaskControlBlockInner {
    ///
    pub fn get_trap_cx(&self) -> &'static mut TrapFrame {
        // self.trap_cx_ppn.get_mut()
        let paddr = &self.trap_cx as *const TrapFrame as usize as *mut TrapFrame;

        unsafe { paddr.as_mut().unwrap() }
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
            inner: SpinNoIrqLock::new(TaskControlBlockInner {
                res: Some(res),
                trap_cx: TrapFrame::new(),
                task_cx: kcontext,
                task_status: TaskStatus::Ready,
                exit_code: None,
                clear_child_tid: 0,
                saved_sigtrapframe: None,
                interrupted_by_signal: false,
                pending_signals: crate::task::signal::SignalSet::empty(),
                blocked_signals: crate::task::signal::SignalSet::empty(),
                need_signal_handle: false,
                sig_context_stack: Vec::new(),
                sigsuspend_old_mask: None,
                futex_woken: false,
                pending_wakeup: false,
                robust_list_head: 0,
                robust_list_len: 0,
                zombie_flag: AtomicBool::new(false),
            }),
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
