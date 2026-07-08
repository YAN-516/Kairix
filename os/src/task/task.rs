use super::id::TaskUserRes;
use super::{KernelStack, ProcessControlBlock, task_entry};
// use crate::config::KERNEL_STACK_SIZE;
use crate::mm::VMSpace;
// use crate::{mm::PhysPageNum, mm::address::*, sync::UPSafeCell};
use crate::sync::SpinNoIrqLock;
use crate::task::processor::PROCESSORS;

use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::cell::RefMut;
use core::error;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicUsize, Ordering};

use polyhal::consts::*;
use polyhal::kcontext::*;
pub use polyhal::utils::addr::*;
use polyhal_trap::trap::*;
use polyhal_trap::trapframe::*;

use log::{error, info};
//use riscv::addr::VirtAddr;
#[allow(missing_docs)]
use alloc::string::String;

static TASK_CREATE_COUNT: AtomicUsize = AtomicUsize::new(0);
static TASK_DROP_COUNT: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Copy)]
pub struct TaskLifecycleStats {
    pub created: usize,
    pub dropped: usize,
    pub live_delta: usize,
}

pub fn task_lifecycle_stats() -> TaskLifecycleStats {
    let created = TASK_CREATE_COUNT.load(Ordering::Relaxed);
    let dropped = TASK_DROP_COUNT.load(Ordering::Relaxed);
    TaskLifecycleStats {
        created,
        dropped,
        live_delta: created.saturating_sub(dropped),
    }
}

pub struct TaskControlBlock {
    // immutable
    pub process: Weak<ProcessControlBlock>,
    pub kstack: KernelStack,
    // mutable
    inner: SpinNoIrqLock<TaskControlBlockInner>,
    sched_policy: AtomicU32,
    sched_priority: AtomicI32,
    mlfq_level: AtomicUsize,
    mlfq_slice_remaining: AtomicUsize,
    mlfq_wait_ticks: AtomicUsize,
    on_cpu: AtomicUsize,
    ready_queued: AtomicUsize,
}

const NO_CPU: usize = usize::MAX;
/// Number of MLFQ levels. Level 0 is the highest priority level.
pub const MLFQ_LEVELS: usize = 4;
/// Highest MLFQ priority level.
pub const MLFQ_TOP_LEVEL: usize = 0;
/// Initial MLFQ level for normal tasks.
pub const MLFQ_DEFAULT_LEVEL: usize = 1;
/// Lowest MLFQ priority level.
pub const MLFQ_BOTTOM_LEVEL: usize = MLFQ_LEVELS - 1;
/// Number of scheduler selections a queued task may wait before aging up.
pub const MLFQ_AGING_THRESHOLD: usize = 64;
/// Per-level time slices measured in timer ticks.
pub const MLFQ_TIME_SLICES: [usize; MLFQ_LEVELS] = [2, 4, 8, 16];

fn clamp_mlfq_level(level: usize) -> usize {
    level.min(MLFQ_BOTTOM_LEVEL)
}

fn mlfq_slice_for_level(level: usize) -> usize {
    MLFQ_TIME_SLICES[clamp_mlfq_level(level)]
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
    #[track_caller]
    pub fn try_inner_exclusive_access(
        &self,
    ) -> Option<crate::sync::SpinMutexGuard<'_, TaskControlBlockInner, crate::sync::SpinNoIrq>>
    {
        self.inner.try_lock()
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
        if priority > 0 {
            self.set_mlfq_level(MLFQ_TOP_LEVEL);
        } else {
            self.set_mlfq_level(MLFQ_DEFAULT_LEVEL);
        }
    }
    #[allow(missing_docs)]
    pub fn mlfq_level(&self) -> usize {
        clamp_mlfq_level(self.mlfq_level.load(Ordering::Relaxed))
    }
    #[allow(missing_docs)]
    pub fn set_mlfq_level(&self, level: usize) {
        let level = clamp_mlfq_level(level);
        self.mlfq_level.store(level, Ordering::Relaxed);
        self.reset_mlfq_slice();
    }
    #[allow(missing_docs)]
    pub fn boost_mlfq_level(&self) {
        let level = self.mlfq_level();
        if level > MLFQ_TOP_LEVEL {
            self.set_mlfq_level(level - 1);
        } else {
            self.reset_mlfq_slice();
        }
    }
    #[allow(missing_docs)]
    pub fn demote_mlfq_level(&self) {
        let level = self.mlfq_level();
        if level < MLFQ_BOTTOM_LEVEL {
            self.set_mlfq_level(level + 1);
        } else {
            self.reset_mlfq_slice();
        }
    }
    #[allow(missing_docs)]
    pub fn reset_mlfq_slice(&self) {
        let slice = mlfq_slice_for_level(self.mlfq_level());
        self.mlfq_slice_remaining.store(slice, Ordering::Relaxed);
    }
    #[allow(missing_docs)]
    pub fn consume_mlfq_tick(&self) -> bool {
        let remaining = self.mlfq_slice_remaining.load(Ordering::Relaxed);
        if remaining <= 1 {
            self.mlfq_slice_remaining.store(0, Ordering::Relaxed);
            true
        } else {
            self.mlfq_slice_remaining
                .store(remaining - 1, Ordering::Relaxed);
            false
        }
    }
    #[allow(missing_docs)]
    pub fn note_mlfq_enqueued(&self) {
        self.mlfq_wait_ticks.store(0, Ordering::Relaxed);
        if self.mlfq_slice_remaining.load(Ordering::Relaxed) == 0 {
            self.reset_mlfq_slice();
        }
    }
    #[allow(missing_docs)]
    pub fn reset_mlfq_wait_ticks(&self) {
        self.mlfq_wait_ticks.store(0, Ordering::Relaxed);
    }
    #[allow(missing_docs)]
    pub fn age_mlfq_wait_tick(&self) -> bool {
        self.mlfq_wait_ticks.fetch_add(1, Ordering::Relaxed) + 1 >= MLFQ_AGING_THRESHOLD
    }
    #[allow(missing_docs)]
    pub fn try_mark_on_cpu(&self, cpu: usize) -> bool {
        self.on_cpu
            .compare_exchange(NO_CPU, cpu, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
    #[allow(missing_docs)]
    pub fn clear_on_cpu(&self) {
        self.on_cpu.store(NO_CPU, Ordering::Release);
    }
    #[allow(missing_docs)]
    pub fn is_on_cpu(&self) -> bool {
        self.on_cpu.load(Ordering::Acquire) != NO_CPU
    }
    #[allow(missing_docs)]
    pub fn try_mark_ready_queued(&self, cpu: usize) -> bool {
        self.ready_queued
            .compare_exchange(NO_CPU, cpu, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
    #[allow(missing_docs)]
    pub fn clear_ready_queued(&self) {
        self.ready_queued.store(NO_CPU, Ordering::Release);
    }
    #[allow(missing_docs)]
    pub fn is_ready_queued(&self) -> bool {
        self.ready_queued.load(Ordering::Acquire) != NO_CPU
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
    /// Linux `CLONE_THREAD` tasks are auto-reaped by the kernel on exit.
    pub auto_reap_on_exit: bool,
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
        TASK_CREATE_COUNT.fetch_add(1, Ordering::Relaxed);
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
            mlfq_level: AtomicUsize::new(MLFQ_DEFAULT_LEVEL),
            mlfq_slice_remaining: AtomicUsize::new(MLFQ_TIME_SLICES[MLFQ_DEFAULT_LEVEL]),
            mlfq_wait_ticks: AtomicUsize::new(0),
            on_cpu: AtomicUsize::new(NO_CPU),
            ready_queued: AtomicUsize::new(NO_CPU),
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
                auto_reap_on_exit: false,
            }),
        }
    }

    /// Release resources whose lifetime must not depend on TCB Drop.
    ///
    /// Exited tasks switch away from their kernel stack instead of unwinding it.
    /// If a stale Arc on that abandoned stack keeps the TCB alive, Drop will not
    /// run, so the idle-side reaper calls this explicitly after the task is no
    /// longer executing on its own stack.
    pub(crate) fn release_exited_resources(&self) {
        let res = {
            let mut inner = self.inner_exclusive_access();
            inner.sig_context_stack.clear();
            inner.saved_sigtrapframe = None;
            inner.sigsuspend_old_mask = None;
            inner.res.take()
        };
        drop(res);
        self.kstack.release();
    }
}

impl Drop for TaskControlBlock {
    fn drop(&mut self) {
        TASK_DROP_COUNT.fetch_add(1, Ordering::Relaxed);
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
