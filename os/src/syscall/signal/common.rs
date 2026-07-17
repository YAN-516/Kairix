use crate::error::{SysError, SyscallResult};
use crate::mm::{translated_ref, translated_refmut};
use crate::security::landlock::landlock_can_signal;
use crate::syscall::time;
use crate::syscall::time::TimeVal;
use crate::task::signal::*;
use crate::task::*;
use crate::trap::_set_sum_bit;
use alloc::sync::Arc;
use log::{error, info};
use polyhal::timer::current_time;

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
    fn new(signo: i32) -> Self {
        Self {
            si_signo: signo,
            si_errno: 0,
            si_code: 0,
            _pad: [0; 116],
        }
    }
}

/// ========== 2. sys_kill ==========
/// 向进程发送信号
pub fn sys_kill(pid: isize, sig: usize) -> SyscallResult {
    _set_sum_bit();
    error!("sys_kill: pid={}, sig={}", pid, sig);

    // 检查信号编号
    if sig >= 64 {
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

/// tgkill: send a signal to a specific thread in a thread group.
/// Since Kairix handles signals at process granularity, we verify that
/// the given tid exists inside the target process and then deliver.
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
    if sig >= 64 {
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

    // 尝试向目标线程专门投递中断标记并唤醒
    let is_blocked = {
        let mut t_inner = target_task.inner_exclusive_access();
        t_inner.interrupted_by_signal = true;
        t_inner.task_status == crate::task::TaskStatus::Blocked
    };
    if is_blocked {
        crate::task::wakeup_task(target_task.clone());
    }

    // 对于自定义 handler 的线程定向信号，投递到目标线程的 pending；
    // 对于 Default / Ignore / SIGKILL / SIGSTOP，走进程级 deliver_signal。
    let action = {
        let p_inner = process.inner_exclusive_access();
        p_inner.signals_handler.get(signal)
    };
    match action.sa_handler {
        SigHandler::Custom(_) => {
            let mut t_inner = target_task.inner_exclusive_access();
            t_inner.pending_signals.add(signal);
            t_inner.need_signal_handle = true;
            info!(
                "sys_tkill: Custom handler -> added sig {} to target_task tid={} pending={:#x}",
                signal.as_i32(),
                t_inner.res.as_ref().map(|r| r.tid).unwrap_or(999),
                t_inner.pending_signals.bits()
            );
        }
        _ => {
            error!(
                "sys_tkill: non-Custom handler ({:?}) -> deliver_signal process-wide",
                action.sa_handler
            );
            deliver_signal(&process, signal);
        }
    }
    Ok(0)
}

/// tgkill(2) - 向指定进程中的指定线程发送信号
pub fn sys_tgkill(tgid: isize, tid: isize, sig: usize) -> SyscallResult {
    _set_sum_bit();
    error!("sys_tgkill: tgid={}, tid={}, sig={}", tgid, tid, sig);

    if tid <= 0 || tgid <= 0 {
        return Err(SysError::EINVAL);
    }
    if sig >= 64 {
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

    // 尝试向目标线程专门投递中断标记并唤醒
    let is_blocked = {
        let mut t_inner = target_task.inner_exclusive_access();
        t_inner.interrupted_by_signal = true;
        t_inner.task_status == crate::task::TaskStatus::Blocked
    };
    if is_blocked {
        crate::task::wakeup_task(target_task.clone());
    }

    // 对于自定义 handler 的线程定向信号，投递到目标线程的 pending；
    // 对于 Default / Ignore / SIGKILL / SIGSTOP，走进程级 deliver_signal。
    let action = {
        let p_inner = target_proc.inner_exclusive_access();
        p_inner.signals_handler.get(signal)
    };
    match action.sa_handler {
        SigHandler::Custom(_) => {
            let mut t_inner = target_task.inner_exclusive_access();
            t_inner.pending_signals.add(signal);
            t_inner.need_signal_handle = true;
        }
        _ => {
            deliver_signal(&target_proc, signal);
        }
    }
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

        for i in 1..64 {
            if (pending >> (i - 1)) & 1 != 0 {
                if let Some(sig) = Signal::from_i32(i) {
                    let action = p_inner.signals_handler.get(sig);
                    match action.sa_handler {
                        SigHandler::Ignore => {}
                        SigHandler::Default => {
                            return true;
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
        if t_inner.task_status == crate::task::TaskStatus::Running {
            running_count += 1;
            continue;
        }
        drop(t_inner);

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
        log::debug!(
            "[signal] requested exit for process with {} task(s) already running",
            running_count
        );
    }
}

pub(super) fn finish_signaled_process(
    proc: &Arc<ProcessControlBlock>,
    signal: Signal,
    core_dump: bool,
) {
    let exit_code = 128 + signal.as_i32();
    let (pid, tasks, parent, exit_signal) = {
        let mut inner = proc.inner_exclusive_access();
        if inner.is_zombie {
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
        (proc.getpid(), tasks, parent, inner.exit_signal)
    };

    if pid != 1 {
        if proc.reparent_children_to(&crate::task::INITPROC) {
            wakeup_first_blocked_task(&crate::task::INITPROC);
        }
    }

    request_tasks_exit(&tasks, exit_code);
    if tasks.is_empty() {
        proc.close_all_files_on_exit();
        proc.release_user_space_on_exit();
        if let Some(parent) = parent {
            if let Some(signal) = crate::task::signal::Signal::from_i32(exit_signal) {
                deliver_signal(&parent, signal);
            }
            wakeup_first_blocked_task(&parent);
        }
    }
}

/// 投递信号到进程
pub fn deliver_signal(proc: &Arc<ProcessControlBlock>, signal: Signal) -> isize {
    let mut inner = proc.inner_exclusive_access();
    // 特殊处理：SIGKILL 和 SIGSTOP 不能被阻塞
    match signal {
        Signal::SigKill => {
            drop(inner);
            finish_signaled_process(proc, signal, false);
            return 0;
        }
        Signal::SigStop => {
            inner.state = crate::task::process::ProcessStatus::Terminal;
            inner.is_stopped = true;
            inner.term_status = crate::task::TermStatus::Stopped(signal.as_i32());
            let parent = inner.parent.as_ref().and_then(|w| w.upgrade());
            drop(inner);
            wakeup_first_blocked_task(proc);
            if let Some(parent) = parent {
                wakeup_first_blocked_task(&parent);
            }
            if let Some(current_task) = crate::task::current_task() {
                if let Some(current_proc) = current_task.process.upgrade() {
                    if Arc::ptr_eq(proc, &current_proc) {
                        crate::task::block_current_and_run_next();
                    }
                }
            }
            return 0;
        }
        Signal::SigCont => {
            let was_stopped = inner.is_stopped;
            if was_stopped {
                inner.is_stopped = false;
                inner.was_continued = true;
                inner.state = crate::task::process::ProcessStatus::Ready;
            }
            let parent = inner.parent.as_ref().and_then(|w| w.upgrade());
            let tasks: alloc::vec::Vec<_> = inner
                .tasks
                .iter()
                .filter_map(|t| t.as_ref().map(Arc::clone))
                .collect();
            drop(inner);
            if was_stopped {
                for task in tasks {
                    crate::task::wakeup_task(task);
                }
                if let Some(parent) = parent {
                    wakeup_first_blocked_task(&parent);
                }
            }
            return 0;
        }
        _ => {}
    }

    // 检查是否被阻塞
    if inner.blocked_signals.contains(signal) {
        inner.pending_signals.add(signal);
        inner.need_signal_handle = true;
        drop(inner);
        wakeup_signal_receivers(proc, signal);
        return 0;
    }

    // 获取处理动作
    let action = inner.signals_handler.get(signal);

    match action.sa_handler {
        SigHandler::Ignore => {
            // 忽略
            drop(inner);
            0
        }
        SigHandler::Default => {
            // 默认处理
            let action = signal.default_action();
            match action {
                SignalAction::Terminate | SignalAction::Core => {
                    let core_dump = matches!(action, SignalAction::Core);
                    drop(inner);
                    finish_signaled_process(proc, signal, core_dump);
                }
                SignalAction::Stop => {
                    inner.handle_default_action(signal);
                    inner.is_stopped = true;
                    inner.term_status = crate::task::TermStatus::Stopped(signal.as_i32());
                    let parent = inner.parent.as_ref().and_then(|w| w.upgrade());
                    drop(inner);
                    wakeup_first_blocked_task(proc);
                    if let Some(parent) = parent {
                        wakeup_first_blocked_task(&parent);
                    }
                    if let Some(current_task) = crate::task::current_task() {
                        if let Some(current_proc) = current_task.process.upgrade() {
                            if Arc::ptr_eq(proc, &current_proc) {
                                crate::task::block_current_and_run_next();
                            }
                        }
                    }
                }
                _ => {
                    inner.handle_default_action(signal);
                    drop(inner);
                    wakeup_signal_receivers(proc, signal);
                }
            }
            0
        }
        SigHandler::Custom(_) => {
            // 用户自定义，标记为需要处理
            inner.pending_signals.add(signal);
            inner.need_signal_handle = true;
            drop(inner);
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
    let process = current_process();
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

    if let Some(mask) = updated_mask {
        let mut p_inner = process.inner_exclusive_access();
        p_inner.blocked_signals = mask;
        p_inner.need_signal_handle = (p_inner.pending_signals.bits() & !mask.bits()) != 0;
    }

    if let Some(mask) = old_mask {
        *translated_refmut(token, oldset as *mut u64)? = mask;
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
        let mut t_inner = task.inner_exclusive_access();
        let mut p_inner = process.inner_exclusive_access();
        let matched =
            (t_inner.pending_signals.bits() | p_inner.pending_signals.bits()) & wait_set.bits();
        if matched != 0 {
            let idx = matched.trailing_zeros() as usize;
            if let Some(sig) = Signal::from_i32((idx + 1) as i32) {
                // 优先从线程级 pending 中移除
                if t_inner.pending_signals.contains(sig) {
                    t_inner.pending_signals.remove(sig);
                    t_inner.need_signal_handle =
                        (t_inner.pending_signals.bits() & !t_inner.blocked_signals.bits()) != 0;
                } else {
                    p_inner.pending_signals.remove(sig);
                    p_inner.need_signal_handle =
                        (p_inner.pending_signals.bits() & !t_inner.blocked_signals.bits()) != 0;
                }
                drop(t_inner);
                drop(p_inner);

                if info != 0 {
                    *translated_refmut(token, info as *mut LinuxSigInfo)? =
                        LinuxSigInfo::new(sig.as_i32());
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
        t_inner.blocked_signals = new_mask;
        t_inner.sigsuspend_old_mask = Some(old_mask);
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
        *translated_refmut(token, old_value as *mut Itimerval)? = Itimerval {
            it_interval: time::TimeVal { sec: 0, usec: 0 },
            it_value: time::TimeVal { sec: 0, usec: 0 },
        };
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

    *translated_refmut(token, curr_value)? = Itimerval {
        it_interval: TimeVal {
            sec: (interval_us / 1_000_000) as i64,
            usec: (interval_us % 1_000_000) as i64,
        },
        it_value: TimeVal {
            sec: (remaining_us / 1_000_000) as i64,
            usec: (remaining_us % 1_000_000) as i64,
        },
    };

    Ok(0)
}
/// ========== 8. sys_sigaltstack ==========
/// 设置/获取备用信号栈（当前为桩实现）
pub fn sys_sigaltstack(_ss: usize, _old_ss: usize) -> SyscallResult {
    Ok(0)
}

/// ========== 9. sys_pidfd_send_signal ==========
/// 通过 pidfd 向进程发送信号
#[repr(C)]
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

    // 如果提供了 siginfo，读取并保存到目标进程
    if info != 0 {
        let token = current_user_token();
        let user_siginfo = translated_ref(token, info as *const UserSigInfo)?;
        let mut target_inner = target.inner_exclusive_access();
        target_inner.last_siginfo = Some(crate::task::signal::SigInfo {
            si_signo: user_siginfo.si_signo,
            si_errno: user_siginfo.si_errno,
            si_code: user_siginfo.si_code,
            si_pid: process.getpid() as i32,
            si_uid: 0, // 当前内核单用户，root
            si_value: user_siginfo.si_value,
        });
        drop(target_inner);
    }

    deliver_signal(&target, signal);
    Ok(0)
}
