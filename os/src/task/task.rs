use super::id::TaskUserRes;
use super::{KernelStack, ProcessControlBlock, task_entry};
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
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};

use polyhal::consts::*;
use polyhal::kcontext::*;
pub use polyhal::utils::addr::*;
use polyhal_trap::trap::*;
use polyhal_trap::trapframe::*;

use log::{error, info};
//use riscv::addr::VirtAddr;
#[allow(missing_docs)]
use alloc::string::String;
pub struct TaskControlBlock {
    // immutable
    pub process: Weak<ProcessControlBlock>,
    pub kstack: KernelStack,
    // mutable
    inner: SpinNoIrqLock<TaskControlBlockInner>,
    sched_policy: AtomicU32,
    sched_priority: AtomicI32,
    on_cpu: AtomicBool,
    ready_queued: AtomicBool,
}

impl TaskControlBlock {
    #[allow(missing_docs)]
    #[track_caller]
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
    #[allow(missing_docs)]
    pub fn sched_priority(&self) -> i32 {
        self.sched_priority.load(Ordering::Relaxed)
    }
    #[allow(missing_docs)]
    pub fn set_sched_priority(&self, priority: i32) {
        self.sched_priority
            .store(priority.clamp(0, 99), Ordering::Relaxed);
    }
    #[allow(missing_docs)]
    pub fn sched_policy(&self) -> u32 {
        self.sched_policy.load(Ordering::Relaxed)
    }
    #[allow(missing_docs)]
    pub fn set_sched_policy(&self, policy: u32) {
        self.sched_policy.store(policy, Ordering::Relaxed);
    }
    #[allow(missing_docs)]
    pub fn set_sched(&self, policy: u32, priority: i32) {
        self.set_sched_policy(policy);
        self.set_sched_priority(priority);
    }
    #[allow(missing_docs)]
    pub fn try_mark_on_cpu(&self) -> bool {
        !self.on_cpu.swap(true, Ordering::AcqRel)
    }
    #[allow(missing_docs)]
    pub fn clear_on_cpu(&self) {
        self.on_cpu.store(false, Ordering::Release);
    }
    #[allow(missing_docs)]
    pub fn try_mark_ready_queued(&self) -> bool {
        !self.ready_queued.swap(true, Ordering::AcqRel)
    }
    #[allow(missing_docs)]
    pub fn clear_ready_queued(&self) {
        self.ready_queued.store(false, Ordering::Release);
    }
}

pub struct TaskControlBlockInner {
    pub res: Option<TaskUserRes>,
    pub global_tid: usize,
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
        global_tid: usize,
    ) -> Self {
        let res = TaskUserRes::new(
            Arc::clone(&process),
            ustack_base,
            alloc_user_res,
            global_tid,
        );
        // let trap_cx_ppn = res.trap_cx_ppn();
        // let kstack = kstack_alloc();
        // let kstack_top = kstack.get_top();
        let kstack_top = kstack.get_top();
        let mut kcontext = KContext::blank();
        kcontext[KContextArgs::KSP] = kstack_top;
        kcontext[KContextArgs::KPC] = task_entry as usize;

        Self {
            process: Arc::downgrade(&process),
            kstack,
            sched_policy: AtomicU32::new(0),
            sched_priority: AtomicI32::new(0),
            on_cpu: AtomicBool::new(false),
            ready_queued: AtomicBool::new(false),
            inner: SpinNoIrqLock::new(TaskControlBlockInner {
                res: Some(res),
                global_tid,
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
#[derive(Copy, Clone, PartialEq, Debug)]
///
pub enum TaskStatus {
    ///
    Ready,
    ///
    Running,
    Blocked,
    Zombie,
    Sleep,
}
