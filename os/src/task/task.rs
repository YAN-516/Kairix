use super::id::TaskUserRes;
use super::{KernelStack, ProcessControlBlock, task_entry};
// use crate::config::KERNEL_STACK_SIZE;
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
    process_id: usize,
    pub kstack: KernelStack,
    // mutable
    inner: SpinNoIrqLock<TaskControlBlockInner>,
    sched_policy: AtomicU32,
    sched_priority: AtomicI32,
    /// Linux SCHED_RESET_ON_FORK state is separate from the base policy.
    /// Keeping it here avoids treating the flag as an executable policy and
    /// lets clone/fork clear realtime scheduling only in the new task.
    sched_reset_on_fork: AtomicBool,
    pi_boost_priority: AtomicI32,
    mlfq_level: AtomicUsize,
    mlfq_slice_remaining: AtomicUsize,
    mlfq_enqueue_epoch: AtomicUsize,
    on_cpu: AtomicUsize,
    last_cpu: AtomicUsize,
    affinity_mask: AtomicUsize,
    ready_queued: AtomicUsize,
    active_syscall: AtomicUsize,
    active_syscall_stage: AtomicUsize,
    active_syscall_ticks: AtomicUsize,
    /// Set whenever the thread has been scheduled out after registering rseq.
    /// The next return to userspace must update the ABI area and abort an
    /// interrupted restartable sequence before clearing this flag.
    rseq_resume_pending: AtomicBool,
    /// Set when this thread must leave the old image so a sibling can execve.
    exec_exit_requested: AtomicBool,
    /// Nesting of kernel locks whose guards live on this task's continuation.
    /// A pending exec/exit may be observed while waiting, but the continuation
    /// must resume and release every such lock before the task can terminate.
    kernel_critical_depth: AtomicUsize,
    last_user_pc: AtomicUsize,
    last_user_ra: AtomicUsize,
    last_user_sp: AtomicUsize,
    last_user_tls: AtomicUsize,
    last_user_fcsr: AtomicUsize,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct UserContextSnapshot {
    pub pc: usize,
    pub ra: usize,
    pub sp: usize,
    pub tls: usize,
    pub fcsr: usize,
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
    /// Replace this thread's comm name, truncating to Linux's 15 visible
    /// bytes and always retaining a trailing NUL.
    pub fn set_comm(&self, name: &[u8]) {
        let mut inner = self.inner_exclusive_access();
        inner.comm.fill(0);
        let end = name
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(name.len());
        let len = core::cmp::min(end, inner.comm.len() - 1);
        inner.comm[..len].copy_from_slice(&name[..len]);
    }

    pub fn comm(&self) -> [u8; 16] {
        self.inner_exclusive_access().comm
    }

    /// Process ID captured when the task is created.
    ///
    /// Scheduler diagnostics use this immutable value instead of upgrading
    /// the process `Weak`, which keeps phase recording lock-free even while
    /// the owning process is being destroyed.
    pub(crate) fn process_id(&self) -> usize {
        self.process_id
    }

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
        process.user_token()
    }
    #[allow(missing_docs)]
    pub fn sched_priority(&self) -> i32 {
        self.sched_priority.load(Ordering::Relaxed)
    }
    pub fn effective_sched_priority(&self) -> i32 {
        self.sched_priority()
            .max(self.pi_boost_priority.load(Ordering::Acquire))
    }
    pub fn set_pi_boost_priority(&self, priority: i32) {
        self.pi_boost_priority
            .store(priority.clamp(0, 99), Ordering::Release);
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
    pub fn sched_reset_on_fork(&self) -> bool {
        self.sched_reset_on_fork.load(Ordering::Acquire)
    }
    pub fn set_sched_reset_on_fork(&self, enabled: bool) {
        self.sched_reset_on_fork.store(enabled, Ordering::Release);
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
    pub fn is_realtime(&self) -> bool {
        self.effective_sched_priority() > 0
    }
    pub fn is_sched_fifo(&self) -> bool {
        self.effective_sched_priority() > self.sched_priority()
            || self.sched_policy() == 1 && self.sched_priority() > 0
    }
    pub fn is_sched_rr(&self) -> bool {
        self.sched_policy() == 2 && self.sched_priority() > 0
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
    pub fn note_mlfq_enqueued(&self, sched_epoch: usize) {
        self.mlfq_enqueue_epoch
            .store(sched_epoch, Ordering::Relaxed);
        if self.mlfq_slice_remaining.load(Ordering::Relaxed) == 0 {
            self.reset_mlfq_slice();
        }
    }
    #[allow(missing_docs)]
    pub fn mlfq_wait_expired(&self, sched_epoch: usize) -> bool {
        let enqueued_epoch = self.mlfq_enqueue_epoch.load(Ordering::Relaxed);
        sched_epoch.wrapping_sub(enqueued_epoch) >= MLFQ_AGING_THRESHOLD
    }
    #[allow(missing_docs)]
    pub fn try_mark_on_cpu(&self, cpu: usize) -> bool {
        let claimed = self
            .on_cpu
            .compare_exchange(NO_CPU, cpu, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        if claimed {
            self.last_cpu.store(cpu, Ordering::Release);
        }
        claimed
    }
    /// Atomically claim a task removed from `queued_cpu` for execution on
    /// `run_cpu`.  `on_cpu` is published before the ready-queue marker is
    /// cleared so a concurrent wakeup never observes the task as unowned.
    pub fn try_claim_queued(&self, queued_cpu: usize, run_cpu: usize) -> bool {
        if self
            .on_cpu
            .compare_exchange(NO_CPU, run_cpu, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }

        if self
            .ready_queued
            .compare_exchange(queued_cpu, NO_CPU, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.last_cpu.store(run_cpu, Ordering::Release);
            true
        } else {
            let _ =
                self.on_cpu
                    .compare_exchange(run_cpu, NO_CPU, Ordering::AcqRel, Ordering::Acquire);
            false
        }
    }
    #[allow(missing_docs)]
    pub fn clear_on_cpu(&self) {
        self.rseq_resume_pending.store(true, Ordering::Release);
        self.on_cpu.store(NO_CPU, Ordering::Release);
    }
    #[allow(missing_docs)]
    pub fn is_on_cpu(&self) -> bool {
        self.on_cpu.load(Ordering::Acquire) != NO_CPU
    }
    #[allow(missing_docs)]
    pub fn is_on_cpu_at(&self, cpu: usize) -> bool {
        self.on_cpu.load(Ordering::Acquire) == cpu
    }
    #[allow(missing_docs)]
    pub fn on_cpu_index(&self) -> Option<usize> {
        let cpu = self.on_cpu.load(Ordering::Acquire);
        (cpu != NO_CPU).then_some(cpu)
    }
    /// Return the CPU on which this task most recently executed.
    pub fn last_cpu_index(&self) -> Option<usize> {
        let cpu = self.last_cpu.load(Ordering::Acquire);
        (cpu != NO_CPU).then_some(cpu)
    }
    pub fn affinity_mask(&self) -> usize {
        self.affinity_mask.load(Ordering::Acquire)
    }
    pub fn set_affinity_mask(&self, mask: usize) {
        self.affinity_mask.store(mask, Ordering::Release);
    }
    pub fn allows_cpu(&self, cpu: usize) -> bool {
        cpu < usize::BITS as usize && self.affinity_mask() & (1usize << cpu) != 0
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
    /// Clear a ready-queue ownership marker only if it still names `cpu`.
    pub fn try_clear_ready_queued(&self, cpu: usize) -> bool {
        self.ready_queued
            .compare_exchange(cpu, NO_CPU, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
    #[allow(missing_docs)]
    pub fn is_ready_queued(&self) -> bool {
        self.ready_queued.load(Ordering::Acquire) != NO_CPU
    }
    #[allow(missing_docs)]
    pub fn is_ready_queued_at(&self, cpu: usize) -> bool {
        self.ready_queued.load(Ordering::Acquire) == cpu
    }
    #[allow(missing_docs)]
    pub fn ready_queued_cpu(&self) -> Option<usize> {
        let cpu = self.ready_queued.load(Ordering::Acquire);
        (cpu != NO_CPU).then_some(cpu)
    }
    #[allow(missing_docs)]
    pub fn set_active_syscall(&self, syscall_id: usize) {
        self.active_syscall_stage.store(0, Ordering::Relaxed);
        self.active_syscall_ticks.store(0, Ordering::Relaxed);
        self.active_syscall.store(syscall_id, Ordering::Release);
        crate::task::processor::publish_current_syscall_nolock(
            self as *const Self,
            Some(syscall_id),
            0,
        );
    }
    /// Publish lock-free progress within the currently active syscall.
    ///
    /// Stage values are syscall-specific and are intended for remote-CPU stall
    /// snapshots when the executing CPU can no longer print its own state.
    pub fn set_active_syscall_stage(&self, stage: usize) {
        self.active_syscall_stage.store(stage, Ordering::Release);
        let syscall_id = self.active_syscall.load(Ordering::Acquire);
        crate::task::processor::publish_current_syscall_nolock(
            self as *const Self,
            (syscall_id != usize::MAX).then_some(syscall_id),
            stage,
        );
    }
    /// Return the most recently published syscall-specific progress stage.
    pub fn active_syscall_stage(&self) -> usize {
        self.active_syscall_stage.load(Ordering::Acquire)
    }
    #[allow(missing_docs)]
    pub fn clear_active_syscall(&self) {
        self.active_syscall.store(usize::MAX, Ordering::Release);
        self.active_syscall_stage.store(0, Ordering::Relaxed);
        self.active_syscall_ticks.store(0, Ordering::Relaxed);
        crate::task::processor::publish_current_syscall_nolock(self as *const Self, None, 0);
    }
    #[allow(missing_docs)]
    pub fn active_syscall(&self) -> Option<usize> {
        let syscall_id = self.active_syscall.load(Ordering::Acquire);
        (syscall_id != usize::MAX).then_some(syscall_id)
    }
    /// Account one timer tick spent executing the currently active syscall.
    ///
    /// The counter belongs to the task rather than a CPU so it remains valid
    /// when SMP load balancing migrates a kernel-side syscall continuation.
    pub fn tick_active_syscall(&self) -> Option<(usize, usize)> {
        let syscall_id = self.active_syscall.load(Ordering::Acquire);
        if syscall_id == usize::MAX {
            return None;
        }
        let ticks = self.active_syscall_ticks.fetch_add(1, Ordering::Relaxed) + 1;
        if self.active_syscall.load(Ordering::Acquire) == syscall_id {
            Some((syscall_id, ticks))
        } else {
            None
        }
    }
    #[allow(missing_docs)]
    pub fn record_user_context(&self, trap_cx: &TrapFrame) {
        self.last_user_pc.store(trap_cx.pc(), Ordering::Relaxed);
        self.last_user_ra
            .store(trap_cx[TrapFrameArgs::RA], Ordering::Relaxed);
        self.last_user_sp
            .store(trap_cx[TrapFrameArgs::SP], Ordering::Relaxed);
        self.last_user_tls
            .store(trap_cx[TrapFrameArgs::TLS], Ordering::Relaxed);
        self.last_user_fcsr.store(trap_cx.fcsr, Ordering::Relaxed);
    }
    #[allow(missing_docs)]
    pub fn user_context_snapshot(&self) -> UserContextSnapshot {
        UserContextSnapshot {
            pc: self.last_user_pc.load(Ordering::Relaxed),
            ra: self.last_user_ra.load(Ordering::Relaxed),
            sp: self.last_user_sp.load(Ordering::Relaxed),
            tls: self.last_user_tls.load(Ordering::Relaxed),
            fcsr: self.last_user_fcsr.load(Ordering::Relaxed),
        }
    }

    /// Request an rseq ABI-area update at the next userspace return.
    pub(crate) fn request_rseq_resume_update(&self) {
        self.rseq_resume_pending.store(true, Ordering::Release);
    }

    /// Whether a scheduling event requires rseq processing before user return.
    pub(crate) fn rseq_resume_update_pending(&self) -> bool {
        self.rseq_resume_pending.load(Ordering::Acquire)
    }

    /// Complete the rseq update requested for this thread.
    pub(crate) fn complete_rseq_resume_update(&self) {
        self.rseq_resume_pending.store(false, Ordering::Release);
    }
}

pub struct TaskControlBlockInner {
    pub res: Option<TaskUserRes>,
    pub global_tid: usize,
    /// Per-thread Linux comm name (TASK_COMM_LEN, including trailing NUL).
    pub comm: [u8; 16],
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
    /// Queued siginfo records. Realtime signals have one entry per
    /// generation; standard signals have at most one while pending.
    pub pending_signal_queue: alloc::collections::VecDeque<crate::task::signal::SigInfo>,
    /// 线程级信号阻塞掩码
    pub blocked_signals: crate::task::signal::SignalSet,
    /// 是否需要处理信号
    pub need_signal_handle: bool,
    /// 信号处理上下文栈（用于线程自定义 handler 返回）
    pub sig_context_stack: Vec<(TrapFrame, crate::task::signal::SignalSet)>,
    /// Old masks for wait syscalls whose temporary mask was interrupted by a
    /// signal.  Each delivered signal frame consumes one entry on sigreturn,
    /// which also handles nested pselect/ppoll/sigsuspend calls correctly.
    pub signal_wait_old_masks: Vec<crate::task::signal::SignalSet>,
    /// 每线程备用信号栈；Linux 的 sigaltstack 状态不属于进程共享属性。
    pub signal_alt_stack: crate::task::signal::SignalAltStack,
    /// 标记该线程是否已被 futex_wake 唤醒（防止丢失唤醒）
    pub futex_woken: bool,
    /// Set when the futex timeout scanner, rather than FUTEX_WAKE, won the
    /// serialized removal from the futex wait queue.
    pub futex_timed_out: bool,
    pub futex_waitv_index: usize,
    /// 标记该线程是否有待处理的唤醒（解决 lost wakeup race）
    pub pending_wakeup: bool,
    /// PID of the CLONE_VFORK child whose exec/exit this task must observe.
    ///
    /// This is a completion condition, not a scheduler wakeup token.  Keeping
    /// it on the parent task lets unrelated I/O and signal wakeups make the
    /// waiter retry without allowing clone() to return early.
    pub vfork_child_pid: Option<usize>,
    /// This task is participating in a POSIX thread-group stop.  Keeping this
    /// separate from `task_status` lets SIGCONT restore runnable threads
    /// without spuriously completing an unrelated futex or I/O sleep.
    pub group_stopped: bool,
    /// A runnable task must become runnable again when the group continues.
    /// A normal wakeup received during the stop also sets this bit so that the
    /// wakeup is not lost while the task is deliberately absent from runqueues.
    pub group_stop_resume: bool,
    /// A blocked task must be queued after it finishes switching off its CPU.
    pub requeue_after_switch: bool,
    /// Preserve front/back queue placement until the idle-side requeue.
    pub requeue_front_after_switch: bool,
    /// robust_list_head 指针（set_robust_list 设置）
    pub robust_list_head: usize,
    /// robust_list 长度（通常为 24 字节）
    pub robust_list_len: usize,
    /// Userspace address registered by rseq(2), or zero when unregistered.
    pub rseq_address: usize,
    /// Size supplied by userspace for the active rseq registration.
    pub rseq_len: u32,
    /// Architecture signature preceding every registered abort handler.
    pub rseq_signature: u32,
    /// Skip rseq processing for the SIGSEGV forced by an rseq user-access
    /// failure; otherwise constructing that signal would recurse immediately.
    pub rseq_signal_fault_bypass: bool,
    /// Defer one resume update so an rseq-fault SIGSEGV handler can enter
    /// userspace before the kernel retries the repaired registration.
    pub rseq_prepare_fault_bypass: bool,
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
            process_id: process.getpid(),
            kstack,
            sched_policy: AtomicU32::new(0),
            sched_priority: AtomicI32::new(0),
            sched_reset_on_fork: AtomicBool::new(false),
            pi_boost_priority: AtomicI32::new(0),
            mlfq_level: AtomicUsize::new(MLFQ_DEFAULT_LEVEL),
            mlfq_slice_remaining: AtomicUsize::new(MLFQ_TIME_SLICES[MLFQ_DEFAULT_LEVEL]),
            mlfq_enqueue_epoch: AtomicUsize::new(0),
            on_cpu: AtomicUsize::new(NO_CPU),
            last_cpu: AtomicUsize::new(NO_CPU),
            affinity_mask: AtomicUsize::new(usize::MAX),
            ready_queued: AtomicUsize::new(NO_CPU),
            active_syscall: AtomicUsize::new(usize::MAX),
            active_syscall_stage: AtomicUsize::new(0),
            active_syscall_ticks: AtomicUsize::new(0),
            rseq_resume_pending: AtomicBool::new(false),
            exec_exit_requested: AtomicBool::new(false),
            kernel_critical_depth: AtomicUsize::new(0),
            last_user_pc: AtomicUsize::new(0),
            last_user_ra: AtomicUsize::new(0),
            last_user_sp: AtomicUsize::new(0),
            last_user_tls: AtomicUsize::new(0),
            last_user_fcsr: AtomicUsize::new(0),
            inner: SpinNoIrqLock::new(TaskControlBlockInner {
                res: Some(res),
                global_tid,
                comm: [0; 16],
                trap_cx: TrapFrame::new(),
                task_cx: kcontext,
                task_status: TaskStatus::Ready,
                exit_code: None,
                clear_child_tid: 0,
                saved_sigtrapframe: None,
                interrupted_by_signal: false,
                pending_signals: crate::task::signal::SignalSet::empty(),
                pending_signal_queue: alloc::collections::VecDeque::new(),
                blocked_signals: crate::task::signal::SignalSet::empty(),
                need_signal_handle: false,
                sig_context_stack: Vec::new(),
                signal_wait_old_masks: Vec::new(),
                signal_alt_stack: crate::task::signal::SignalAltStack::disabled(),
                futex_woken: false,
                futex_timed_out: false,
                futex_waitv_index: usize::MAX,
                pending_wakeup: false,
                vfork_child_pid: None,
                group_stopped: false,
                group_stop_resume: false,
                requeue_after_switch: false,
                requeue_front_after_switch: false,
                robust_list_head: 0,
                robust_list_len: 0,
                rseq_address: 0,
                rseq_len: 0,
                rseq_signature: 0,
                rseq_signal_fault_bypass: false,
                rseq_prepare_fault_bypass: false,
                zombie_flag: AtomicBool::new(false),
                auto_reap_on_exit: false,
            }),
        }
    }

    /// Ask this task to exit at the next kernel safe point for a sibling's execve.
    pub(crate) fn request_exec_exit(&self) {
        self.exec_exit_requested.store(true, Ordering::Release);
    }

    /// Return whether this task is being removed by a sibling's execve.
    pub(crate) fn exec_exit_requested(&self) -> bool {
        self.exec_exit_requested.load(Ordering::Acquire)
    }

    /// Enter a kernel critical section whose guard is stored on this stack.
    pub(crate) fn enter_kernel_critical_section(&self) {
        let previous = self.kernel_critical_depth.fetch_add(1, Ordering::AcqRel);
        assert!(previous != usize::MAX, "kernel critical depth overflow");
    }

    /// Leave one kernel critical-section nesting level.
    pub(crate) fn leave_kernel_critical_section(&self) {
        let previous = self.kernel_critical_depth.fetch_sub(1, Ordering::AcqRel);
        assert!(previous != 0, "kernel critical depth underflow");
    }

    /// Whether terminating now would abandon a live kernel lock guard.
    pub(crate) fn kernel_critical_section_active(&self) -> bool {
        self.kernel_critical_depth.load(Ordering::Acquire) != 0
    }

    /// Current nesting depth used by lock-free deadlock diagnostics.
    pub(crate) fn kernel_critical_section_depth(&self) -> usize {
        self.kernel_critical_depth.load(Ordering::Acquire)
    }

    /// Clear the request after the winning execve caller becomes the sole thread.
    pub(crate) fn clear_exec_exit_request(&self) {
        self.exec_exit_requested.store(false, Ordering::Release);
    }

    /// Release resources whose lifetime must not depend on TCB Drop.
    ///
    /// Exited tasks switch away from their kernel stack instead of unwinding it.
    /// If a stale Arc on that abandoned stack keeps the TCB alive, Drop will not
    /// run, so the idle-side reaper calls this explicitly after the task is no
    /// longer executing on its own stack.
    pub(crate) fn release_exited_resources(&self, process: Option<&Arc<ProcessControlBlock>>) {
        let on_cpu = self.on_cpu_index();
        let ready_queued_cpu = self.ready_queued_cpu();
        assert!(
            on_cpu.is_none() && ready_queued_cpu.is_none(),
            "attempted to release runnable exited task resources: on_cpu={:?} ready_queued_cpu={:?}",
            on_cpu,
            ready_queued_cpu,
        );
        crate::task::processor::record_scheduler_phase(70, None);
        let mut res = {
            let mut inner = self.inner_exclusive_access();
            crate::task::processor::record_scheduler_phase(71, None);
            inner.sig_context_stack.clear();
            inner.saved_sigtrapframe = None;
            inner.signal_wait_old_masks.clear();
            inner.signal_alt_stack = crate::task::signal::SignalAltStack::disabled();
            inner.res.take()
        };
        crate::task::processor::record_scheduler_phase(72, None);
        if let Some(res) = res.as_mut() {
            res.release_with_process(process);
        }
        drop(res);
        crate::task::processor::record_scheduler_phase(73, None);
        self.kstack.release();
        crate::task::processor::record_scheduler_phase(74, None);
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
