use crate::error::{SysError, SysResult, SyscallResult};
use crate::mm::{translated_ref, write_user_value};
use crate::security::landlock::landlock_can_signal;
use crate::syscall::time;
use crate::syscall::time::TimeVal;
use crate::task::signal::*;
use crate::task::*;
use crate::trap::_set_sum_bit;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use log::{error, info};
use polyhal::timer::current_time;
use polyhal_trap::trapframe::TrapFrameArgs;

/// Restores a thread's signal mask on every syscall exit path.  ppoll,
/// pselect6, and epoll_pwait install their temporary masks before inspecting
/// readiness and keep this guard alive through waker registration and sleep.
pub struct TemporarySignalMask {
    task: Arc<TaskControlBlock>,
    old_mask: SignalSet,
    restore_on_drop: bool,
}

impl Drop for TemporarySignalMask {
    fn drop(&mut self) {
        if !self.restore_on_drop {
            return;
        }
        let mut inner = self.task.inner_exclusive_access();
        inner.blocked_signals = self.old_mask;
        inner.need_signal_handle =
            (inner.pending_signals.bits() & !inner.blocked_signals.bits()) != 0;
    }
}

impl TemporarySignalMask {
    /// A signal interrupted the wait. Keep the temporary mask installed while
    /// its signal frame is built, then restore the old mask from rt_sigreturn.
    pub fn defer_restore_until_sigreturn(&mut self) {
        if !self.restore_on_drop {
            return;
        }
        self.task
            .inner_exclusive_access()
            .signal_wait_old_masks
            .push(self.old_mask);
        self.restore_on_drop = false;
    }
}

/// Install a syscall-scoped signal mask and return its restoration guard.
pub fn install_temporary_signal_mask(mask: SignalSet) -> SysResult<TemporarySignalMask> {
    let task = current_task().ok_or(SysError::ESRCH)?;
    let mut inner = task.inner_exclusive_access();
    let old_mask = inner.blocked_signals;
    inner.blocked_signals = mask.without_unblockable();
    inner.need_signal_handle = (inner.pending_signals.bits() & !inner.blocked_signals.bits()) != 0;
    drop(inner);
    Ok(TemporarySignalMask {
        task,
        old_mask,
        restore_on_drop: true,
    })
}

/// Whether a pending, unblocked signal must interrupt a poll-like wait.
/// Poll-family waits are interrupted by caught signals regardless of
/// SA_RESTART; explicitly or implicitly ignored signals do not end the wait.
pub fn pending_signal_interrupts_wait() -> bool {
    let Some(task) = current_task() else {
        return false;
    };
    let (task_pending, blocked) = {
        let inner = task.inner_exclusive_access();
        (inner.pending_signals.bits(), inner.blocked_signals.bits())
    };
    let Some(process) = task.process.upgrade() else {
        return false;
    };
    let inner = process.inner_exclusive_access();
    let pending = (task_pending | inner.pending_signals.bits()) & !blocked;
    if pending == 0 {
        return false;
    }
    let handlers = inner.signals_handler.lock();
    for number in 1..=64 {
        if pending & (1u64 << (number - 1)) == 0 {
            continue;
        }
        let Some(signal) = Signal::from_i32(number) else {
            continue;
        };
        match handlers.get(signal).sa_handler {
            SigHandler::Ignore => {}
            SigHandler::Default if signal.default_action() == SignalAction::Ignore => {}
            SigHandler::Default | SigHandler::Custom(_) => return true,
        }
    }
    false
}

/// Default and ignored dispositions create no user signal frame, hence no
/// rt_sigreturn at which an interrupted wait could restore its old mask.
pub(super) fn restore_wait_mask_without_signal_frame(task: &Arc<TaskControlBlock>) {
    let mut inner = task.inner_exclusive_access();
    if let Some(old_mask) = inner.signal_wait_old_masks.pop() {
        inner.blocked_signals = old_mask.without_unblockable();
        inner.need_signal_handle =
            (inner.pending_signals.bits() & !inner.blocked_signals.bits()) != 0;
    }
}

fn generated_siginfo(signal: Signal) -> SigInfo {
    let sender_pid = current_task()
        .and_then(|task| task.process.upgrade())
        .map(|process| process.getpid() as i32)
        .unwrap_or(0);
    SigInfo {
        si_signo: signal.as_i32(),
        si_errno: 0,
        si_code: 0,
        si_pid: sender_pid,
        si_uid: 0,
        si_value: 0,
    }
}

pub(crate) fn enqueue_pending_signal(
    pending: &mut SignalSet,
    queue: &mut VecDeque<SigInfo>,
    signal: Signal,
    info: Option<SigInfo>,
) {
    let realtime = signal.as_i32() >= 32;
    if realtime || !pending.contains(signal) {
        queue.push_back(info.unwrap_or_else(|| generated_siginfo(signal)));
    }
    pending.add(signal);
}

pub(crate) fn consume_pending_signal(
    pending: &mut SignalSet,
    queue: &mut VecDeque<SigInfo>,
    signal: Signal,
) -> Option<SigInfo> {
    let position = queue
        .iter()
        .position(|entry| entry.si_signo == signal.as_i32());
    let info = position.and_then(|position| queue.remove(position));
    if signal.as_i32() < 32 || !queue.iter().any(|entry| entry.si_signo == signal.as_i32()) {
        pending.remove(signal);
    }
    info
}

pub(super) fn discard_pending_signal(
    pending: &mut SignalSet,
    queue: &mut VecDeque<SigInfo>,
    signal: Signal,
) {
    pending.remove(signal);
    queue.retain(|entry| entry.si_signo != signal.as_i32());
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub(super) struct LinuxStack {
    pub sp: usize,
    pub flags: i32,
    // 显式清零 ABI 对齐填充，避免向用户态复制未初始化的内核栈字节。
    pub _pad: u32,
    pub size: usize,
}

pub(super) struct SignalStackPlan {
    pub frame_sp: usize,
    pub saved_alt_stack: LinuxStack,
    autodisarm: bool,
}

fn configured_alt_stack(
    stack: LinuxStack,
    allow_onstack: bool,
) -> Result<SignalAltStack, SysError> {
    let flags = stack.flags as u32;
    if flags == SS_DISABLE {
        return Ok(SignalAltStack::disabled());
    }
    let allowed = if allow_onstack {
        flags == 0 || flags == SS_ONSTACK || flags == SS_AUTODISARM
    } else {
        flags == 0 || flags == SS_AUTODISARM
    };
    if !allowed {
        return Err(SysError::EINVAL);
    }
    if stack.size < MINSIGSTKSZ {
        return Err(SysError::ENOMEM);
    }
    if stack.sp.checked_add(stack.size).is_none() {
        return Err(SysError::EINVAL);
    }
    Ok(SignalAltStack::new(
        stack.sp,
        stack.size,
        flags & SS_AUTODISARM,
    ))
}

pub(super) fn prepare_signal_stack(
    task: &Arc<TaskControlBlock>,
    current_sp: usize,
    action_flags: u32,
    frame_size: usize,
) -> Option<SignalStackPlan> {
    let alt_stack = task.inner_exclusive_access().signal_alt_stack;
    let saved_alt_stack = LinuxStack {
        sp: alt_stack.sp,
        flags: alt_stack.user_flags(current_sp) as i32,
        _pad: 0,
        size: alt_stack.size,
    };
    let switch_to_alt =
        action_flags & SA_ONSTACK != 0 && alt_stack.is_enabled() && !alt_stack.contains(current_sp);
    let stack_top = if switch_to_alt {
        alt_stack.top()?
    } else {
        current_sp
    };
    let frame_sp = stack_top.checked_sub(frame_size)? & !0xf;
    if switch_to_alt && frame_sp < alt_stack.sp {
        return None;
    }
    Some(SignalStackPlan {
        frame_sp,
        saved_alt_stack,
        autodisarm: switch_to_alt && alt_stack.flags & SS_AUTODISARM != 0,
    })
}

pub(super) fn write_alt_stack_to_ucontext(
    frame: &mut [u8],
    ucontext_base: usize,
    stack: LinuxStack,
) {
    frame[ucontext_base + 16..ucontext_base + 24].copy_from_slice(&stack.sp.to_ne_bytes());
    frame[ucontext_base + 24..ucontext_base + 28].copy_from_slice(&stack.flags.to_ne_bytes());
    frame[ucontext_base + 32..ucontext_base + 40].copy_from_slice(&stack.size.to_ne_bytes());
}

pub(super) fn commit_signal_stack(task: &Arc<TaskControlBlock>, plan: &SignalStackPlan) {
    if plan.autodisarm {
        task.inner_exclusive_access().signal_alt_stack = SignalAltStack::disabled();
    }
}

/// Apply Linux's forced-SIGSEGV rule when a userspace signal frame cannot be
/// constructed. The signal selected for delivery has already left the normal
/// disposition path, so leaving it pending would retry the same inaccessible
/// stack forever.
pub(super) fn handle_signal_frame_failure(
    process: &Arc<ProcessControlBlock>,
    task: &Arc<TaskControlBlock>,
    signal: Signal,
    task_level: bool,
    error: SysError,
) {
    let tid = task.inner_exclusive_access().global_tid;
    error!(
        "[SIGNAL_FRAME_FAULT] pid={} tid={} signal={} err={:?}; forcing SIGSEGV",
        process.getpid(),
        tid,
        signal.as_i32(),
        error,
    );

    // Linux dequeues the selected signal before attempting setup_rt_frame().
    // Mirror that ordering so a lower-numbered failed signal cannot starve the
    // forced SIGSEGV that reports the frame construction fault.
    if task_level {
        let mut inner = task.inner_exclusive_access();
        let inner = &mut *inner;
        consume_pending_signal(
            &mut inner.pending_signals,
            &mut inner.pending_signal_queue,
            signal,
        );
        inner.need_signal_handle =
            (inner.pending_signals.bits() & !inner.blocked_signals.bits()) != 0;
    } else {
        let blocked = task.inner_exclusive_access().blocked_signals.bits();
        let mut inner = process.inner_exclusive_access();
        let inner = &mut *inner;
        consume_pending_signal(
            &mut inner.pending_signals,
            &mut inner.pending_signal_queue,
            signal,
        );
        inner.need_signal_handle = (inner.pending_signals.bits() & !blocked) != 0;
    }

    task.inner_exclusive_access()
        .blocked_signals
        .remove(Signal::SigSegv);
    let catch_forced_sigsegv = {
        let mut inner = process.inner_exclusive_access();
        inner.blocked_signals.remove(Signal::SigSegv);
        let mut handlers = inner.signals_handler.lock();
        let action = handlers.get(Signal::SigSegv);
        if signal == Signal::SigSegv || !action.is_custom() {
            // If SIGSEGV itself cannot build a frame, Linux resets a caught or
            // ignored disposition to SIG_DFL to prevent recursive delivery.
            let _ = handlers.reset(Signal::SigSegv);
            false
        } else {
            true
        }
    };

    if catch_forced_sigsegv {
        deliver_thread_signal(process, task, Signal::SigSegv, None, false);
    } else {
        finish_signaled_process(process, Signal::SigSegv, true);
    }
}

pub(super) fn restore_signal_alt_stack(
    task: &Arc<TaskControlBlock>,
    stack: LinuxStack,
) -> Result<(), SysError> {
    let restored = configured_alt_stack(stack, true)?;
    task.inner_exclusive_access().signal_alt_stack = restored;
    Ok(())
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct LinuxTimeSpec {
    tv_sec: i64,
    tv_nsec: i64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
///
pub struct Itimerval {
    it_interval: time::TimeVal,
    it_value: time::TimeVal,
}

// 仅写入 glibc/musl 常用字段，剩余保持 0。
#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxSigInfo {
    si_signo: i32,
    si_errno: i32,
    si_code: i32,
    _pad: [u8; 116],
}

impl LinuxSigInfo {
    fn from_siginfo(info: SigInfo) -> Self {
        let mut result = Self {
            si_signo: info.si_signo,
            si_errno: info.si_errno,
            si_code: info.si_code,
            _pad: [0; 116],
        };
        // Linux's generic kill/rt payload starts at byte 16 of siginfo_t.
        result._pad[4..8].copy_from_slice(&info.si_pid.to_ne_bytes());
        result._pad[8..12].copy_from_slice(&info.si_uid.to_ne_bytes());
        result._pad[12..16].copy_from_slice(&info.si_value.to_ne_bytes());
        result
    }
}

/// ========== 2. sys_kill ==========
/// 向进程发送信号
pub fn sys_kill(pid: isize, sig: usize) -> SyscallResult {
    _set_sum_bit();
    error!("sys_kill: pid={}, sig={}", pid, sig);

    // 检查信号编号
    if sig > 64 {
        return Err(SysError::EINVAL);
    }

    let targets = if pid > 0 {
        match pid2process(pid as usize) {
            Some(target) => alloc::vec![target],
            None => return Err(SysError::ESRCH),
        }
    } else if pid == 0 {
        processes_in_pgrp(current_process().getpgid())
    } else if pid == -1 {
        let current_pid = current_process().getpid();
        all_processes()
            .into_iter()
            .filter(|process| process.getpid() != 1 && process.getpid() != current_pid)
            .collect()
    } else {
        processes_in_pgrp((-pid) as usize)
    };
    if targets.is_empty() {
        return Err(SysError::ESRCH);
    }
    let current = current_process();
    if targets
        .iter()
        .any(|target| !landlock_can_signal(&current, target))
    {
        return Err(SysError::EPERM);
    }

    // 空信号，只检查进程是否存在
    if sig == 0 {
        return Ok(0);
    }

    // 转换信号
    let signal = match Signal::from_i32(sig as i32) {
        Some(s) => s,
        None => return Err(SysError::EINVAL),
    };

    // 投递信号
    for target in targets {
        deliver_signal(&target, signal);
    }
    Ok(0)
}

/// Publish a thread-directed signal before issuing the scheduler wakeup.
///
/// `wakeup_task()` must run even while the target is still `Running`: in the
/// futex check-to-block window it records `pending_wakeup`, which prevents the
/// target from going to sleep after the signal has already been delivered.
fn deliver_thread_signal(
    process: &Arc<ProcessControlBlock>,
    target_task: &Arc<TaskControlBlock>,
    signal: Signal,
    siginfo: Option<SigInfo>,
    log_tkill_delivery: bool,
) {
    let action = {
        let p_inner = process.inner_exclusive_access();
        p_inner.signals_handler.lock().get(signal)
    };
    // SIGKILL/SIGSTOP are process-wide even when generated through tgkill.
    if matches!(signal, Signal::SigKill | Signal::SigStop) {
        deliver_signal(process, signal);
        return;
    }
    if signal == Signal::SigCont {
        // The continue side effect is process-wide and immediate, but a
        // caught/blocked tgkill(SIGCONT) remains directed at the requested
        // thread rather than migrating into the process-pending queue.
        continue_process(process);
    }
    if matches!(action.sa_handler, SigHandler::Ignore) {
        return;
    }

    // Thread-directed signals never migrate to the process-pending set.  In
    // particular, a blocked signal with a default terminating disposition must
    // remain pending for this exact thread instead of killing the group early.
    let deliverable = {
        let mut t_inner = target_task.inner_exclusive_access();
        let t_inner = &mut *t_inner;
        enqueue_pending_signal(
            &mut t_inner.pending_signals,
            &mut t_inner.pending_signal_queue,
            signal,
            siginfo,
        );
        let deliverable = !t_inner.blocked_signals.contains(signal);
        t_inner.need_signal_handle =
            (t_inner.pending_signals.bits() & !t_inner.blocked_signals.bits()) != 0;
        if deliverable {
            t_inner.interrupted_by_signal = true;
        }
        if log_tkill_delivery {
            info!(
                "sys_tkill: Custom handler -> added sig {} to target_task tid={} pending={:#x}",
                signal.as_i32(),
                t_inner.res.as_ref().map(|r| r.tid).unwrap_or(999),
                t_inner.pending_signals.bits()
            );
        }
        deliverable
    };
    crate::syscall::misc::wake_signalfd_waiters(process, signal);
    if deliverable {
        crate::task::wakeup_task(Arc::clone(target_task));
    }
}

/// tkill: send a signal to a specific thread.
pub fn sys_tkill(tid: isize, sig: usize) -> SyscallResult {
    _set_sum_bit();
    error!("[DEBUG sys_tkill] tid={}, sig={}", tid, sig);
    {
        let process = current_process();
        let inner = process.inner_exclusive_access();
        info!("sys_tkill: process.inner addr = {:p}", &*inner as *const _);
    }

    if tid <= 0 {
        return Err(SysError::EINVAL);
    }
    if sig > 64 {
        return Err(SysError::EINVAL);
    }

    let process = current_process();
    let target_task = match crate::task::tid2task(tid as usize) {
        Some(t) => t,
        None => return Err(SysError::ESRCH),
    };
    // Verify the tid belongs to this process
    let target_pid = target_task.process.upgrade().unwrap().getpid();
    if target_pid != process.getpid() {
        return Err(SysError::ESRCH);
    }
    // 线程已退出（zombie），不能接收信号
    if target_task.inner_exclusive_access().exit_code.is_some() {
        return Err(SysError::ESRCH);
    }

    if sig == 0 {
        return Ok(0);
    }

    let signal = match Signal::from_i32(sig as i32) {
        Some(s) => s,
        None => return Err(SysError::EINVAL),
    };

    deliver_thread_signal(&process, &target_task, signal, None, true);
    Ok(0)
}

/// tgkill(2) - 向指定进程中的指定线程发送信号
pub fn sys_tgkill(tgid: isize, tid: isize, sig: usize) -> SyscallResult {
    _set_sum_bit();
    error!("sys_tgkill: tgid={}, tid={}, sig={}", tgid, tid, sig);

    // SIGABRT is normally raised from a libc fatal path.  In particular, a
    // vfork child may still carry its parent's cached pthread pid/tid before
    // exec.  Record the actual syscall caller as well as the requested target
    // so we can distinguish a process aborting itself from a child accidentally
    // aborting its vfork parent.
    if sig == Signal::SigAbrt.as_i32() as usize {
        let caller_process = current_process();
        let caller_pid = caller_process.getpid();
        let caller_executable = caller_process
            .inner_exclusive_access()
            .executable_path
            .clone();
        let (caller_tid, caller_pc, caller_ra, caller_sp, active_syscall) =
            if let Some(task) = current_task() {
                let snapshot = task.user_context_snapshot();
                let caller_tid = task.inner_exclusive_access().global_tid;
                (
                    caller_tid,
                    snapshot.pc,
                    snapshot.ra,
                    snapshot.sp,
                    task.active_syscall(),
                )
            } else {
                (usize::MAX, 0, 0, 0, None)
            };
        error!(
            "[TGKILL_ABORT_CALLER] cpu={} caller_pid={} caller_tid={} target_tgid={} target_tid={} pc={:#x} ra={:#x} sp={:#x} active_syscall={:?} executable={}",
            polyhal::arch::hart_id(),
            caller_pid,
            caller_tid,
            tgid,
            tid,
            caller_pc,
            caller_ra,
            caller_sp,
            active_syscall,
            caller_executable,
        );
    }

    if tid <= 0 || tgid <= 0 {
        return Err(SysError::EINVAL);
    }
    if sig > 64 {
        return Err(SysError::EINVAL);
    }

    let target_proc = match pid2process(tgid as usize) {
        Some(p) => p,
        None => return Err(SysError::ESRCH),
    };

    let target_task = match crate::task::tid2task(tid as usize) {
        Some(t) => t,
        None => return Err(SysError::ESRCH),
    };
    // Verify the tid belongs to the target process
    let target_pid = target_task.process.upgrade().unwrap().getpid();
    if target_pid != target_proc.getpid() {
        return Err(SysError::ESRCH);
    }
    if !landlock_can_signal(&current_process(), &target_proc) {
        return Err(SysError::EPERM);
    }
    // 线程已退出（zombie），不能接收信号
    if target_task.inner_exclusive_access().exit_code.is_some() {
        return Err(SysError::ESRCH);
    }

    if sig == 0 {
        return Ok(0);
    }

    let signal = match Signal::from_i32(sig as i32) {
        Some(s) => s,
        None => return Err(SysError::EINVAL),
    };

    deliver_thread_signal(&target_proc, &target_task, signal, None, false);
    Ok(0)
}

fn signal_must_interrupt_blocking_syscall(signal: Signal) -> bool {
    matches!(
        signal,
        Signal::SigHup | Signal::SigInt | Signal::SigQuit | Signal::SigTerm
    )
}

/// 检查当前任务的阻塞系统调用是否应该返回用户态处理信号。
///
/// 对带 SA_RESTART 的普通自定义信号继续等待，避免 SIGCHLD 这类监督信号
/// 把 pipe/read/write 等阻塞 syscall 误打断；但终止/交互类信号仍要立刻返回
/// 用户态执行 handler，用于 hackbench 等程序清理 worker。
pub fn should_interrupt_syscall() -> bool {
    let task = match current_task() {
        Some(t) => t,
        None => return false,
    };
    let (task_pending, blocked) = {
        let t_inner = task.inner_exclusive_access();
        (
            t_inner.pending_signals.bits(),
            t_inner.blocked_signals.bits(),
        )
    };

    if let Some(process) = task.process.upgrade() {
        let p_inner = process.inner_exclusive_access();
        let pending = (task_pending | p_inner.pending_signals.bits()) & !blocked;

        if pending == 0 {
            return false;
        }

        for i in 1..=64 {
            if (pending >> (i - 1)) & 1 != 0 {
                if let Some(sig) = Signal::from_i32(i) {
                    let action = p_inner.signals_handler.lock().get(sig);
                    match action.sa_handler {
                        SigHandler::Ignore => {}
                        SigHandler::Default => {
                            // A default disposition is not necessarily an
                            // interrupting disposition.  In particular,
                            // SIGCHLD/SIGURG/SIGWINCH are ignored by default;
                            // treating them like caught signals can make a
                            // pipe read lose data when a writer exits just as
                            // it makes the pipe readable (popen is a common
                            // trigger for this race).
                            if sig.default_action() != SignalAction::Ignore {
                                return true;
                            }
                        }
                        SigHandler::Custom(_) => {
                            if action.sa_flags & SA_RESTART == 0
                                || signal_must_interrupt_blocking_syscall(sig)
                            {
                                return true;
                            }
                        }
                    }
                }
            }
        }
    }
    false
}

/// 唤醒目标进程中第一个处于 Blocked 状态的任务，并标记为被信号中断
#[allow(dead_code)]
fn wakeup_first_blocked_task(proc: &Arc<ProcessControlBlock>) {
    let tasks = {
        let inner = proc.inner_exclusive_access();
        inner
            .tasks
            .iter()
            .filter_map(|task| task.as_ref().map(Arc::clone))
            .collect::<alloc::vec::Vec<_>>()
    };

    for task in tasks {
        let mut t_inner = task.inner_exclusive_access();
        if t_inner.task_status == crate::task::TaskStatus::Blocked {
            t_inner.interrupted_by_signal = true;
            drop(t_inner);
            crate::task::wakeup_task(task);
            break;
        }
    }
}

fn wakeup_signal_receivers(proc: &Arc<ProcessControlBlock>, signal: Signal) {
    let tasks = {
        let inner = proc.inner_exclusive_access();
        inner
            .tasks
            .iter()
            .filter_map(|task| task.as_ref().map(Arc::clone))
            .collect::<alloc::vec::Vec<_>>()
    };

    for task in tasks {
        let mut t_inner = task.inner_exclusive_access();
        if t_inner.blocked_signals.contains(signal) {
            continue;
        }
        t_inner.interrupted_by_signal = true;
        drop(t_inner);
        crate::task::wakeup_task(task);
    }
}

fn request_tasks_exit(tasks: &[Arc<TaskControlBlock>], exit_code: i32) {
    let mut running_count = 0usize;
    for task in tasks {
        let mut t_inner = task.inner_exclusive_access();
        t_inner
            .zombie_flag
            .store(true, core::sync::atomic::Ordering::SeqCst);
        t_inner.interrupted_by_signal = true;
        if t_inner.exit_code.is_none() {
            t_inner.exit_code = Some(exit_code);
        }
        let is_running = t_inner.task_status == crate::task::TaskStatus::Running;
        let running_cpu = if is_running {
            running_count += 1;
            task.on_cpu_index()
        } else {
            None
        };
        drop(t_inner);

        if is_running {
            // A fatal process-directed signal is also a cross-CPU scheduling
            // event. Merely setting zombie_flag leaves a sibling executing
            // user code indefinitely when its timer interrupt is delayed or
            // masked. Force it through the kernel safe point after dropping
            // its TCB lock so the remote CPU can inspect the task immediately.
            if let Some(cpu) = running_cpu.or_else(|| task.on_cpu_index()) {
                let _ = polyhal::multicore::send_reschedule_ipi(cpu);
            }
            continue;
        }

        // A blocked task may have Arc<TaskControlBlock>/Arc<ProcessControlBlock>
        // locals on its own kernel stack. If another CPU marks it Zombie and
        // drops the external refs, that stack never unwinds and the task keeps
        // itself alive. Wake it so it exits on its own stack via
        // exit_current_and_run_next().
        crate::task::remove_task(Arc::clone(task));
        crate::task::remove_task_from_timer_queue(task);
        crate::syscall::futex::remove_task_from_futex_table(task);
        crate::task::wakeup_task(Arc::clone(task));
    }
    if running_count != 0 {
        error!(
            "[SIGNAL_FATAL] tasks_marked_exit count={} running={} exit_code={}",
            tasks.len(),
            running_count,
            exit_code,
        );
        log::debug!(
            "[signal] requested exit for process with {} task(s) already running",
            running_count
        );
    }
}

/// Enter a Linux thread-group stop without destroying each thread's underlying
/// blocking state. Runnable tasks are removed from runqueues and running tasks
/// are forced through a scheduler safe point; tasks already sleeping remain
/// asleep across SIGCONT unless a real wakeup arrives while the group is
/// stopped.
pub(super) fn stop_process(proc: &Arc<ProcessControlBlock>, signal: Signal) {
    let (tasks, parent) = {
        let mut inner = proc.inner_exclusive_access();
        if inner.is_zombie {
            return;
        }
        inner.state = crate::task::process::ProcessStatus::Terminal;
        inner.is_stopped = true;
        inner.term_status = crate::task::TermStatus::Stopped(signal.as_i32());
        {
            let inner = &mut *inner;
            discard_pending_signal(
                &mut inner.pending_signals,
                &mut inner.pending_signal_queue,
                Signal::SigCont,
            );
            inner.need_signal_handle = !inner.pending_signals.is_empty();
        }
        (
            inner
                .tasks
                .iter()
                .filter_map(|task| task.as_ref().map(Arc::clone))
                .collect::<alloc::vec::Vec<_>>(),
            inner.parent.as_ref().and_then(|parent| parent.upgrade()),
        )
    };

    for task in &tasks {
        let on_cpu = {
            let mut task_inner = task.inner_exclusive_access();
            if task_inner.task_status == crate::task::TaskStatus::Zombie {
                continue;
            }
            if !task_inner.group_stopped {
                task_inner.group_stop_resume = matches!(
                    task_inner.task_status,
                    crate::task::TaskStatus::Ready | crate::task::TaskStatus::Running
                );
                task_inner.group_stopped = true;
            }
            {
                let task_inner = &mut *task_inner;
                discard_pending_signal(
                    &mut task_inner.pending_signals,
                    &mut task_inner.pending_signal_queue,
                    Signal::SigCont,
                );
                task_inner.need_signal_handle =
                    (task_inner.pending_signals.bits() & !task_inner.blocked_signals.bits()) != 0;
            }
            if task_inner.group_stop_resume {
                task_inner.task_status = crate::task::TaskStatus::Blocked;
                task_inner.requeue_after_switch = false;
                task_inner.requeue_front_after_switch = false;
            }
            task.on_cpu_index()
        };

        crate::task::remove_task(Arc::clone(task));
        if let Some(cpu) = on_cpu.or_else(|| task.on_cpu_index()) {
            let _ = polyhal::multicore::send_reschedule_ipi(cpu);
        }
    }

    if let Some(parent) = parent {
        parent.publish_child_event();
        wakeup_first_blocked_task(&parent);
    }

    if let Some(current) = crate::task::current_task() {
        if current
            .process
            .upgrade()
            .is_some_and(|current_proc| Arc::ptr_eq(proc, &current_proc))
        {
            crate::task::suspend_current_and_run_next();
        }
    }
}

fn continue_process(proc: &Arc<ProcessControlBlock>) {
    let (was_stopped, tasks, parent) = {
        let mut inner = proc.inner_exclusive_access();
        let was_stopped = inner.is_stopped;
        if was_stopped {
            inner.is_stopped = false;
            inner.was_continued = true;
            inner.state = crate::task::process::ProcessStatus::Ready;
        }
        for stop_signal in [
            Signal::SigStop,
            Signal::SigTstp,
            Signal::SigTtin,
            Signal::SigTtou,
        ] {
            let inner = &mut *inner;
            discard_pending_signal(
                &mut inner.pending_signals,
                &mut inner.pending_signal_queue,
                stop_signal,
            );
        }
        inner.need_signal_handle = !inner.pending_signals.is_empty();
        (
            was_stopped,
            inner
                .tasks
                .iter()
                .filter_map(|task| task.as_ref().map(Arc::clone))
                .collect::<alloc::vec::Vec<_>>(),
            inner.parent.as_ref().and_then(|parent| parent.upgrade()),
        )
    };
    for task in tasks {
        let should_wake = {
            let mut task_inner = task.inner_exclusive_access();
            let should_wake = task_inner.group_stopped
                && task_inner.group_stop_resume
                && task_inner.task_status != crate::task::TaskStatus::Zombie;
            task_inner.group_stopped = false;
            task_inner.group_stop_resume = false;
            for stop_signal in [
                Signal::SigStop,
                Signal::SigTstp,
                Signal::SigTtin,
                Signal::SigTtou,
            ] {
                let task_inner = &mut *task_inner;
                discard_pending_signal(
                    &mut task_inner.pending_signals,
                    &mut task_inner.pending_signal_queue,
                    stop_signal,
                );
            }
            task_inner.need_signal_handle =
                (task_inner.pending_signals.bits() & !task_inner.blocked_signals.bits()) != 0;
            if should_wake {
                task_inner.pending_wakeup = false;
            }
            should_wake
        };
        if should_wake {
            crate::task::wakeup_task(task);
        }
    }
    if was_stopped {
        if let Some(parent) = parent {
            parent.publish_child_event();
            wakeup_first_blocked_task(&parent);
        }
    }
}

pub(super) fn finish_signaled_process(
    proc: &Arc<ProcessControlBlock>,
    signal: Signal,
    core_dump: bool,
) {
    let exit_code = 128 + signal.as_i32();
    error!(
        "[SIGNAL_FATAL] enter pid={} signal={} exit_code={} core_dump={}",
        proc.getpid(),
        signal.as_i32(),
        exit_code,
        core_dump,
    );
    let (pid, tasks, parent, exit_signal) = {
        let mut inner = proc.inner_exclusive_access();
        if inner.is_zombie {
            error!(
                "[SIGNAL_FATAL] already_zombie pid={} signal={} stored_exit={}",
                proc.getpid(),
                signal.as_i32(),
                inner.exit_code,
            );
            return;
        }
        inner.is_zombie = true;
        inner
            .zombie_flag
            .store(true, core::sync::atomic::Ordering::SeqCst);
        inner.exit_code = exit_code;
        inner.term_status = crate::task::TermStatus::Signaled(signal.as_i32(), core_dump);
        let tasks = inner
            .tasks
            .iter()
            .filter_map(|task| task.as_ref().map(Arc::clone))
            .collect::<alloc::vec::Vec<_>>();
        let parent = inner.parent.as_ref().and_then(|w| w.upgrade());
        error!(
            "[SIGNAL_FATAL] marked_zombie pid={} signal={} tasks={} term_status={:?}",
            proc.getpid(),
            signal.as_i32(),
            tasks.len(),
            inner.term_status,
        );
        (proc.getpid(), tasks, parent, inner.exit_signal)
    };

    if pid != 1 {
        if proc.reparent_children_to(&crate::task::INITPROC) {
            crate::task::INITPROC.publish_child_event();
            wakeup_first_blocked_task(&crate::task::INITPROC);
        }
    }

    request_tasks_exit(&tasks, exit_code);
    if tasks.is_empty() {
        proc.close_all_files_on_exit();
        proc.release_user_space_on_exit();
        if let Some(parent) = parent {
            parent.publish_child_event();
            if let Some(signal) = crate::task::signal::Signal::from_i32(exit_signal) {
                deliver_signal(&parent, signal);
            }
            wakeup_first_blocked_task(&parent);
        }
    }
}

/// 投递信号到进程
pub fn deliver_signal(proc: &Arc<ProcessControlBlock>, signal: Signal) -> isize {
    deliver_signal_with_info(proc, signal, None)
}

/// Deliver a child's configured exit signal after deferred exit cleanup has
/// switched off the child's kernel stack. There is no current task on that
/// scheduler continuation, so preserve the child PID explicitly in siginfo.
pub(crate) fn deliver_child_exit_signal(
    proc: &Arc<ProcessControlBlock>,
    signal: Signal,
    child_pid: usize,
) -> isize {
    deliver_signal_with_info(
        proc,
        signal,
        Some(SigInfo {
            si_signo: signal.as_i32(),
            si_errno: 0,
            si_code: 0,
            si_pid: child_pid as i32,
            si_uid: 0,
            si_value: 0,
        }),
    )
}

fn deliver_signal_with_info(
    proc: &Arc<ProcessControlBlock>,
    signal: Signal,
    siginfo: Option<SigInfo>,
) -> isize {
    // 特殊处理：SIGKILL 和 SIGSTOP 不能被阻塞
    match signal {
        Signal::SigKill => {
            finish_signaled_process(proc, signal, false);
            return 0;
        }
        Signal::SigStop => {
            stop_process(proc, signal);
            return 0;
        }
        Signal::SigCont => {
            // Continuing is unconditional and immediate, even when SIGCONT is
            // blocked. A caught SIGCONT is still queued below for its handler.
            continue_process(proc);
        }
        _ => {}
    }

    let mut inner = proc.inner_exclusive_access();

    // 获取处理动作
    let action = inner.signals_handler.lock().get(signal);

    match action.sa_handler {
        SigHandler::Ignore => {
            // 忽略
            drop(inner);
            0
        }
        SigHandler::Default => {
            // Signal generation and signal action are separate Linux phases.
            // Queue first so per-thread masks choose an eligible recipient;
            // the eventual recipient performs the default action on return to
            // user mode. SIGKILL/SIGSTOP/SIGCONT were handled above.
            {
                let inner_ref = &mut *inner;
                enqueue_pending_signal(
                    &mut inner_ref.pending_signals,
                    &mut inner_ref.pending_signal_queue,
                    signal,
                    siginfo,
                );
            }
            inner.need_signal_handle = true;
            drop(inner);
            crate::syscall::misc::wake_signalfd_waiters(proc, signal);
            wakeup_signal_receivers(proc, signal);
            0
        }
        SigHandler::Custom(_) => {
            // 用户自定义，标记为需要处理
            {
                let inner_ref = &mut *inner;
                enqueue_pending_signal(
                    &mut inner_ref.pending_signals,
                    &mut inner_ref.pending_signal_queue,
                    signal,
                    siginfo,
                );
            }
            inner.need_signal_handle = true;
            drop(inner);
            crate::syscall::misc::wake_signalfd_waiters(proc, signal);
            wakeup_signal_receivers(proc, signal);
            0
        }
    }
}

/// ========== 3. sys_sigprocmask ==========
/// 检查或更改阻塞信号掩码
pub fn sys_sigprocmask(how: usize, set: usize, oldset: usize, _sigsetsize: usize) -> SyscallResult {
    _set_sum_bit();
    info!(
        "sys_sigprocmask: how={}, set={:#x}, oldset={:#x}",
        how, set, oldset
    );
    let token = current_user_token();

    // 先读用户输入，避免持锁访问用户地址触发缺页死锁。
    let new_set = if set != 0 {
        let bits = *translated_ref(token, set as *const u64)?;
        info!(
            "sys_sigprocmask: read set addr={:p}, bits={:#x}",
            set as *const u64, bits
        );
        Some(SignalSet::from_bits(bits))
    } else {
        None
    };

    let task = current_task().unwrap();
    let mut old_mask = None;
    let mut updated_mask = None;
    {
        let mut t_inner = task.inner_exclusive_access();

        // 返回旧的阻塞掩码
        if oldset != 0 {
            old_mask = Some(t_inner.blocked_signals.bits());
        }

        // 设置新的阻塞掩码
        if let Some(new_set) = new_set {
            match how {
                0 => {
                    // SIG_BLOCK
                    let bits = t_inner.blocked_signals.bits() | new_set.bits();
                    t_inner.blocked_signals = SignalSet::from_bits(bits);
                }
                1 => {
                    // SIG_UNBLOCK
                    let bits = t_inner.blocked_signals.bits() & !new_set.bits();
                    t_inner.blocked_signals = SignalSet::from_bits(bits);
                }
                2 => {
                    // SIG_SETMASK
                    t_inner.blocked_signals = new_set;
                }
                _ => return Err(SysError::EINVAL),
            }
            t_inner.blocked_signals = t_inner.blocked_signals.without_unblockable();
            updated_mask = Some(t_inner.blocked_signals);

            // 解除阻塞后，检查是否有待处理的信号（线程级 + 进程级）
            if how == 1 || how == 2 {
                let ready = t_inner.pending_signals.bits() & !t_inner.blocked_signals.bits();
                if ready != 0 {
                    t_inner.need_signal_handle = true;
                }
            }
        }
    }

    // Signal masks are strictly per-thread. Process-directed pending signals
    // are filtered against the mask of whichever thread is considered for
    // delivery, never against a last-writer-wins PCB copy.

    if let Some(mask) = old_mask {
        write_user_value(token, oldset as *mut u64, &mask)?;
    }

    info!(
        "sys_sigprocmask: done, old_mask={:?}, updated_mask={:?}",
        old_mask, updated_mask
    );
    Ok(0)
}

/// ========== 4. sys_rt_sigtimedwait (137) ==========
/// 从给定信号集中取一个待处理信号，可选超时。
/// 返回值：成功返回信号编号；失败返回负 errno。
pub fn sys_rt_sigtimedwait(
    set: usize,
    info: usize,
    timeout: usize,
    _sigsetsize: usize,
) -> SyscallResult {
    _set_sum_bit();
    if set == 0 {
        return Err(SysError::EINVAL);
    }

    let token = current_user_token();
    let wait_set = SignalSet::from_bits(*translated_ref(token, set as *const u64)?);

    let deadline_us = if timeout != 0 {
        let ts = *translated_ref(token, timeout as *const LinuxTimeSpec)?;
        if ts.tv_sec < 0 || ts.tv_nsec < 0 || ts.tv_nsec >= 1_000_000_000 {
            return Err(SysError::EINVAL);
        }
        let delta_us = (ts.tv_sec as i128)
            .saturating_mul(1_000_000)
            .saturating_add((ts.tv_nsec as i128) / 1_000);
        Some((current_time().as_micros() as i128).saturating_add(delta_us))
    } else {
        None
    };

    loop {
        let process = current_process();
        let task = current_task().unwrap();
        let mut p_inner = process.inner_exclusive_access();
        // Global order is process -> task. Taking these in the reverse order
        // can deadlock against signal generation and thread exit.
        let mut t_inner = task.inner_exclusive_access();
        let matched =
            (t_inner.pending_signals.bits() | p_inner.pending_signals.bits()) & wait_set.bits();
        if matched != 0 {
            let idx = matched.trailing_zeros() as usize;
            if let Some(sig) = Signal::from_i32((idx + 1) as i32) {
                // 优先从线程级 pending 中移除
                let from_task = t_inner.pending_signals.contains(sig);
                let consumed = if from_task {
                    {
                        let inner = &mut *t_inner;
                        let consumed = consume_pending_signal(
                            &mut inner.pending_signals,
                            &mut inner.pending_signal_queue,
                            sig,
                        );
                        consumed
                    }
                } else {
                    {
                        let inner = &mut *p_inner;
                        consume_pending_signal(
                            &mut inner.pending_signals,
                            &mut inner.pending_signal_queue,
                            sig,
                        )
                    }
                };
                if from_task {
                    t_inner.need_signal_handle =
                        (t_inner.pending_signals.bits() & !t_inner.blocked_signals.bits()) != 0;
                } else {
                    p_inner.need_signal_handle =
                        (p_inner.pending_signals.bits() & !t_inner.blocked_signals.bits()) != 0;
                }
                drop(t_inner);
                drop(p_inner);

                if info != 0 {
                    let siginfo = consumed.unwrap_or_else(|| generated_siginfo(sig));
                    write_user_value(
                        token,
                        info as *mut LinuxSigInfo,
                        &LinuxSigInfo::from_siginfo(siginfo),
                    )?;
                }
                return Ok(sig.as_i32() as usize);
            }
        }
        drop(t_inner);
        drop(p_inner);

        if let Some(deadline) = deadline_us {
            if (current_time().as_micros() as i128) >= deadline {
                return Err(SysError::EAGAIN);
            }
        }
        block_current_and_run_next();
        // 被强制终止信号或被非 SA_RESTART 信号中断后应直接返回 -EINTR
        if current_process().inner_exclusive_access().is_zombie || should_interrupt_syscall() {
            return Err(SysError::EINTR);
        }
    }
}
/// ========== 5.5 sys_pause (34) ==========
/// 挂起调用进程，直到捕获到一个信号。
/// 返回时总是返回 -EINTR（如果进程没有被信号终止或停止）。
pub fn sys_pause() -> SyscallResult {
    let task = current_task().unwrap();
    let process = task.process.upgrade().unwrap();
    loop {
        {
            let t_inner = task.inner_exclusive_access();
            let task_pending = t_inner.pending_signals.bits() & !t_inner.blocked_signals.bits();
            if task_pending != 0 {
                return Err(SysError::EINTR);
            }
        }
        let blocked = task.inner_exclusive_access().blocked_signals.bits();
        let proc_pending = process.inner_exclusive_access().pending_signals.bits() & !blocked;
        if proc_pending != 0 {
            return Err(SysError::EINTR);
        }
        block_current_and_run_next();
        // 被强制终止信号或被非 SA_RESTART 信号中断后应直接返回 -EINTR
        if current_process().inner_exclusive_access().is_zombie || should_interrupt_syscall() {
            return Err(SysError::EINTR);
        }
    }
}

/// ========== 5.6 sys_rt_sigsuspend (133) ==========
/// 原子地替换当前线程的信号阻塞掩码，然后挂起进程直到收到未被阻塞的信号。
/// sigreturn 后会恢复原来的掩码。
pub fn sys_rt_sigsuspend(mask_ptr: usize, sigsetsize: usize) -> SyscallResult {
    if sigsetsize != core::mem::size_of::<u64>() {
        return Err(SysError::EINVAL);
    }

    let new_mask = if mask_ptr != 0 {
        let token = current_user_token();
        let bits = *translated_ref(token, mask_ptr as *const u64)?;
        SignalSet::from_bits(bits)
    } else {
        SignalSet::empty()
    };

    let task = current_task().unwrap();
    let process = task.process.upgrade().unwrap();

    // 保存旧掩码并设置新掩码
    {
        let mut t_inner = task.inner_exclusive_access();
        let old_mask = t_inner.blocked_signals;
        t_inner.blocked_signals = new_mask.without_unblockable();
        t_inner.signal_wait_old_masks.push(old_mask);
    }

    loop {
        {
            let t_inner = task.inner_exclusive_access();
            let task_pending = t_inner.pending_signals.bits() & !t_inner.blocked_signals.bits();
            if task_pending != 0 {
                return Err(SysError::EINTR);
            }
        }
        let blocked = task.inner_exclusive_access().blocked_signals.bits();
        let proc_pending = process.inner_exclusive_access().pending_signals.bits() & !blocked;
        if proc_pending != 0 {
            return Err(SysError::EINTR);
        }
        block_current_and_run_next();
        // 被强制终止信号或被非 SA_RESTART 信号中断后应直接返回 -EINTR
        if current_process().inner_exclusive_access().is_zombie || should_interrupt_syscall() {
            return Err(SysError::EINTR);
        }
    }
}
/// ========== 7. setitimer / getitimer ==========

/// 设置间隔定时器（目前仅支持 ITIMER_REAL）
pub fn sys_setitimer(which: usize, new_value: usize, old_value: usize) -> SyscallResult {
    //const EINVAL: isize = -22;
    const ITIMER_REAL: usize = 0;

    _set_sum_bit();
    error!(
        "sys_setitimer: pid = {}, which={}, new_value={:#x}, old_value={:#x}",
        current_process().getpid(),
        which,
        new_value,
        old_value
    );

    if which != ITIMER_REAL {
        return Err(SysError::EINVAL);
    }

    let process = current_process();
    let token = current_user_token();

    if old_value != 0 {
        write_user_value(token, old_value as *mut Itimerval, &Itimerval {
            it_interval: time::TimeVal { sec: 0, usec: 0 },
            it_value: time::TimeVal { sec: 0, usec: 0 },
        })?;
    }

    let new_timer = if new_value != 0 {
        Some(*translated_ref(token, new_value as *const Itimerval)?)
    } else {
        None
    };

    let (new_deadline, new_interval) = if let Some(new) = new_timer {
        let value_usec = new
            .it_value
            .sec
            .max(0)
            .saturating_mul(1_000_000)
            .saturating_add(new.it_value.usec.max(0));
        let interval_usec = new
            .it_interval
            .sec
            .max(0)
            .saturating_mul(1_000_000)
            .saturating_add(new.it_interval.usec.max(0));

        let deadline = if value_usec > 0 {
            let freq = polyhal::timer::get_freq() as usize;
            let ticks = (value_usec as usize).saturating_mul(freq) / 1_000_000;
            Some(crate::timer::get_time().saturating_add(ticks))
        } else {
            None
        };
        let interval = if interval_usec > 0 {
            Some(
                (interval_usec as usize).saturating_mul(polyhal::timer::get_freq() as usize)
                    / 1_000_000,
            )
        } else {
            None
        };
        (deadline, interval)
    } else {
        (None, None)
    };

    {
        let mut inner = process.inner_exclusive_access();
        inner.itimer_real_deadline = new_deadline;
        inner.itimer_real_interval = new_interval;
    }

    if new_deadline.is_some() {
        crate::task::manager::TIMER_PROCS
            .lock()
            .insert(process.getpid(), Arc::clone(&process));
    } else {
        crate::task::manager::TIMER_PROCS
            .lock()
            .remove(&process.getpid());
    }

    Ok(0)
}

/// 获取间隔定时器的当前值（目前仅支持 ITIMER_REAL）
pub fn sys_getitimer(which: usize, curr_value: *mut Itimerval) -> SyscallResult {
    const ITIMER_REAL: usize = 0;

    if which != ITIMER_REAL {
        return Err(SysError::EINVAL);
    }

    let process = current_process();
    let token = current_user_token();

    let (remaining_us, interval_us) = {
        let inner = process.inner_exclusive_access();
        let freq = polyhal::timer::get_freq() as usize;
        let remaining_us = if let Some(deadline) = inner.itimer_real_deadline {
            let remaining_ticks = deadline.saturating_sub(crate::timer::get_time());
            (remaining_ticks.saturating_mul(1_000_000) / freq) as u128
        } else {
            0
        };
        let interval_us = inner
            .itimer_real_interval
            .map(|ticks| (ticks.saturating_mul(1_000_000) / freq) as u128)
            .unwrap_or(0);
        (remaining_us, interval_us)
    };

    write_user_value(token, curr_value, &Itimerval {
        it_interval: TimeVal {
            sec: (interval_us / 1_000_000) as i64,
            usec: (interval_us % 1_000_000) as i64,
        },
        it_value: TimeVal {
            sec: (remaining_us / 1_000_000) as i64,
            usec: (remaining_us % 1_000_000) as i64,
        },
    })?;

    Ok(0)
}
/// ========== 8. sys_sigaltstack ==========
/// 设置/获取当前线程的备用信号栈。
pub fn sys_sigaltstack(ss: usize, old_ss: usize) -> SyscallResult {
    _set_sum_bit();
    let task = current_task().ok_or(SysError::ESRCH)?;
    let token = current_user_token();
    let current_sp = current_trap_cx()[TrapFrameArgs::SP];

    // 先完整读取新值，以支持 ss 与 old_ss 指向同一地址的合法用法。
    let requested = if ss != 0 {
        Some(*translated_ref(token, ss as *const LinuxStack)?)
    } else {
        None
    };
    let old_config = task.inner_exclusive_access().signal_alt_stack;
    if old_ss != 0 {
        write_user_value(token, old_ss as *mut LinuxStack, &LinuxStack {
            sp: old_config.sp,
            flags: old_config.user_flags(current_sp) as i32,
            _pad: 0,
            size: old_config.size,
        })?;
    }

    if let Some(requested) = requested {
        // Linux 禁止在正在使用的普通备用栈上改变配置。AUTODISARM 进入
        // handler 后配置已暂时禁用，因此允许该 handler 安装新栈。
        if old_config.contains(current_sp) {
            return Err(SysError::EPERM);
        }
        let new_config = configured_alt_stack(requested, false)?;
        task.inner_exclusive_access().signal_alt_stack = new_config;
    }
    Ok(0)
}

/// ========== 9. sys_pidfd_send_signal ==========
/// 通过 pidfd 向进程发送信号
#[repr(C)]
#[derive(Clone, Copy)]
struct UserSigInfo {
    si_signo: i32,
    si_errno: i32,
    si_code: i32,
    __pad0: [u8; 4],
    _kill_pid: i32,
    _kill_uid: u32,
    si_value: i32,
    __pad1: [u8; 4],
    __rest: [u8; 96],
}

fn queued_user_siginfo(info: usize, sig: i32) -> Result<SigInfo, SysError> {
    if info == 0 {
        return Err(SysError::EFAULT);
    }
    let user = translated_ref(current_user_token(), info as *const UserSigInfo)?;
    if user.si_signo != sig || user.si_code >= 0 {
        return Err(SysError::EPERM);
    }
    Ok(SigInfo {
        si_signo: sig,
        si_errno: user.si_errno,
        si_code: user.si_code,
        si_pid: current_process().getpid() as i32,
        si_uid: 0,
        si_value: user.si_value,
    })
}

/// Queue a process-directed signal together with its siginfo payload.
pub fn sys_rt_sigqueueinfo(pid: isize, sig: i32, info: usize) -> SyscallResult {
    if pid <= 0 || !(1..=64).contains(&sig) {
        return Err(SysError::EINVAL);
    }
    let target = pid2process(pid as usize).ok_or(SysError::ESRCH)?;
    if !landlock_can_signal(&current_process(), &target) {
        return Err(SysError::EPERM);
    }
    let signal = Signal::from_i32(sig).ok_or(SysError::EINVAL)?;
    let siginfo = queued_user_siginfo(info, sig)?;
    deliver_signal_with_info(&target, signal, Some(siginfo));
    Ok(0)
}

/// Queue a thread-directed signal together with its siginfo payload.
pub fn sys_rt_tgsigqueueinfo(tgid: isize, tid: isize, sig: i32, info: usize) -> SyscallResult {
    if tgid <= 0 || tid <= 0 || !(1..=64).contains(&sig) {
        return Err(SysError::EINVAL);
    }
    let target_task = tid2task(tid as usize).ok_or(SysError::ESRCH)?;
    let target = target_task.process.upgrade().ok_or(SysError::ESRCH)?;
    if target.getpid() != tgid as usize {
        return Err(SysError::ESRCH);
    }
    if !landlock_can_signal(&current_process(), &target) {
        return Err(SysError::EPERM);
    }
    let signal = Signal::from_i32(sig).ok_or(SysError::EINVAL)?;
    let siginfo = queued_user_siginfo(info, sig)?;
    deliver_thread_signal(&target, &target_task, signal, Some(siginfo), false);
    Ok(0)
}

/// Send a signal to a process identified by a pidfd
pub fn sys_pidfd_send_signal(pidfd: i32, sig: i32, info: usize, flags: u32) -> SyscallResult {
    _set_sum_bit();
    if pidfd < 0 {
        return Err(SysError::EBADF);
    }
    if sig < 0 || sig > 64 {
        return Err(SysError::EINVAL);
    }
    if flags != 0 {
        return Err(SysError::EINVAL);
    }

    let process = current_process();
    let inner = process.inner_exclusive_access();
    let fd = pidfd as usize;
    if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
        return Err(SysError::EBADF);
    }
    let file = inner.fd_table[fd].as_ref().unwrap().clone();
    let target_pid = match file.pidfd_pid() {
        Some(p) => p,
        None => return Err(SysError::EINVAL),
    };
    drop(inner);

    let target = match pid2process(target_pid) {
        Some(p) => p,
        None => return Err(SysError::ESRCH),
    };
    if !landlock_can_signal(&process, &target) {
        return Err(SysError::EPERM);
    }

    if sig == 0 {
        return Ok(0);
    }

    let signal = match Signal::from_i32(sig) {
        Some(s) => s,
        None => return Err(SysError::EINVAL),
    };

    let siginfo = if info != 0 {
        let token = current_user_token();
        let user_siginfo = translated_ref(token, info as *const UserSigInfo)?;
        if user_siginfo.si_signo != sig {
            return Err(SysError::EINVAL);
        }
        Some(crate::task::signal::SigInfo {
            si_signo: sig,
            si_errno: user_siginfo.si_errno,
            si_code: user_siginfo.si_code,
            si_pid: process.getpid() as i32,
            si_uid: 0, // 当前内核单用户，root
            si_value: user_siginfo.si_value,
        })
    } else {
        None
    };

    deliver_signal_with_info(&target, signal, siginfo);
    Ok(0)
}
