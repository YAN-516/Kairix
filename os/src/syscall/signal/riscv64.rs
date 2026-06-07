// src/signal/syscall.rs
use crate::error::{SysError, SyscallResult};
use crate::mm::{translated_byte_buffer, translated_ref, translated_refmut};
use crate::syscall::landlock::landlock_can_signal;
use crate::syscall::time;
use crate::syscall::time::TimeVal;
use crate::task::signal::*;
use crate::task::*;
#[cfg(target_arch = "riscv64")]
use crate::timer::get_time_us;
use crate::trap::_set_sum_bit;
use alloc::sync::Arc;
use log::warn;
use log::{error, info, trace};
use polyhal::println;
use polyhal::timer::current_time;
use polyhal_trap::trapframe::TrapFrameArgs;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct LinuxRtSigAction {
    handler: usize,
    flags: usize,
    mask: usize,
}

fn kernel_to_linux_sigaction(action: SigAction) -> LinuxRtSigAction {
    LinuxRtSigAction {
        handler: action.sa_handler.as_ptr() as usize,
        flags: action.sa_flags as usize,
        mask: action.sa_mask.bits() as usize,
    }
}

fn linux_to_kernel_sigaction(action: LinuxRtSigAction) -> SigAction {
    SigAction {
        sa_handler: unsafe { SigHandler::from_ptr(action.handler as *const core::ffi::c_void) },
        sa_mask: SignalSet::from_bits(action.mask as u64),
        sa_flags: action.flags as u32,
        sa_restorer: 0,
    }
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
    fn new(signo: i32) -> Self {
        Self {
            si_signo: signo,
            si_errno: 0,
            si_code: 0,
            _pad: [0; 116],
        }
    }
}

/// ========== 1. sys_sigaction ==========
/// 设置或查询信号处理函数
pub fn sys_sigaction(
    signum: usize,
    act: usize,
    oldact: usize,
    _sigsetsize: usize,
) -> SyscallResult {
    _set_sum_bit();
    warn!("PRINTLN sys_sigaction: signum={}", signum);
    warn!(
        "sys_sigaction: signum={}, act={:#x}, oldact={:#x}",
        signum, act, oldact
    );
    let process = current_process();
    // 检查信号编号
    let signal = match Signal::from_i32(signum as i32) {
        Some(s) => s,
        None => return Err(SysError::EINVAL),
    };

    if !signal.can_catch() && act != 0 {
        return Err(SysError::EINVAL);
    }

    let token = current_user_token();

    // 先读取用户传入的新 action，避免持锁后访问用户地址导致缺页死锁。
    let new_action = if act != 0 {
        Some(linux_to_kernel_sigaction(*translated_ref(
            token,
            act as *const LinuxRtSigAction,
        )?))
    } else {
        None
    };

    if let Some(ref new_action) = new_action {
        match new_action.sa_handler {
            crate::task::signal::SigHandler::Default => {
                error!("[DEBUG sigaction] new handler = DEFAULT")
            }
            crate::task::signal::SigHandler::Ignore => {
                error!("[DEBUG sigaction] new handler = IGNORE")
            }
            crate::task::signal::SigHandler::Custom(addr) => {
                error!("[DEBUG sigaction] new handler = CUSTOM {:p}", addr)
            }
        }
    }
    let mut old_action = None;
    let mut clear_task_pending = false;
    {
        let mut inner = process.inner_exclusive_access();

        // 返回旧的信号处理动作
        if oldact != 0 {
            old_action = Some(inner.signals_handler.get(signal));
        }

        // 设置新的信号处理动作
        if let Some(new_action) = new_action {
            if inner
                .signals_handler
                .set(signal, &new_action as *const SigAction)
                .is_err()
            {
                return Err(SysError::EINVAL);
            }
            if new_action.is_ignored() {
                inner.pending_signals.remove(signal);
                clear_task_pending = true;
            }
        }
    }
    if clear_task_pending {
        // Do this after dropping process.inner to avoid process -> task lock order.
        let task = current_task().unwrap();
        task.inner_exclusive_access().pending_signals.remove(signal);
    }

    if let Some(old) = old_action {
        if oldact != 0 {
            *translated_refmut(token, oldact as *mut LinuxRtSigAction)? =
                kernel_to_linux_sigaction(old);
            if oldact == 0 {
                return Err(SysError::EFAULT);
            }
        }
    }
    return Ok(0);
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

/// 检查当前任务的阻塞系统调用是否应该返回用户态处理信号。
/// Linux 标准行为：
/// - 如果有 pending 的、未被阻塞的、非忽略的信号
/// - 且信号的 handler 是 Default（终止行为）或 Custom
/// - 则当前内核必须先打断阻塞 syscall，让返回用户态路径安装/执行 handler。
///
/// 注意：SA_RESTART 控制的是 handler 返回后的 syscall 重启语义；本内核目前没有
/// 完整 syscall restart 机制。如果这里因为 SA_RESTART 继续阻塞，像 hackbench 这种
/// 使用 signal(SIGINT, handler) 清理 worker 的程序会永远进不了 handler。
pub fn should_interrupt_syscall() -> bool {
    let task = match current_task() {
        Some(t) => t,
        None => return false,
    };
    let t_inner = task.inner_exclusive_access();
    let blocked = t_inner.blocked_signals.bits();

    if let Some(process) = task.process.upgrade() {
        let p_inner = process.inner_exclusive_access();
        let pending = (t_inner.pending_signals.bits() | p_inner.pending_signals.bits()) & !blocked;

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
                            return true;
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

fn mark_tasks_zombie(tasks: &[Arc<TaskControlBlock>]) {
    for task in tasks {
        // Do not remove these tasks here: Running/Ready tasks will observe the
        // zombie flag before returning to userspace, and Blocked tasks are woken
        // after the process lock has been released.
        let t_inner = task.inner_exclusive_access();
        t_inner
            .zombie_flag
            .store(true, core::sync::atomic::Ordering::SeqCst);
    }
}

fn finish_signaled_process(proc: &Arc<ProcessControlBlock>, signal: Signal, core_dump: bool) {
    let current_is_target = crate::task::current_task()
        .and_then(|task| task.process.upgrade())
        .is_some_and(|current_proc| Arc::ptr_eq(proc, &current_proc));

    let (tasks, parent, exit_signal) = {
        let mut inner = proc.inner_exclusive_access();
        inner.is_zombie = true;
        inner
            .zombie_flag
            .store(true, core::sync::atomic::Ordering::SeqCst);
        inner.exit_code = 128 + signal.as_i32();
        inner.term_status = crate::task::TermStatus::Signaled(signal.as_i32(), core_dump);
        let tasks = inner
            .tasks
            .iter()
            .filter_map(|task| task.as_ref().map(Arc::clone))
            .collect::<alloc::vec::Vec<_>>();
        let parent = inner.parent.as_ref().and_then(|w| w.upgrade());
        (tasks, parent, inner.exit_signal)
    };

    mark_tasks_zombie(&tasks);
    let mut running_count = 0usize;
    for task in tasks {
        let mut task_inner = task.inner_exclusive_access();
        task_inner.interrupted_by_signal = true;
        if task_inner.task_status == crate::task::TaskStatus::Running {
            running_count += 1;
            continue;
        }
        task_inner.task_status = crate::task::TaskStatus::Zombie;
        drop(task_inner);
        crate::task::remove_task(task);
    }

    let should_wake_parent = {
        let mut inner = proc.inner_exclusive_access();
        inner.alive_thread_count = running_count;
        inner.alive_thread_count == 0
    };
    if current_is_target {
        crate::task::exit_current_and_run_next(128 + signal.as_i32());
    }
    if !should_wake_parent {
        return;
    }

    if let Some(parent) = parent {
        if let Some(signal) = crate::task::signal::Signal::from_i32(exit_signal) {
            deliver_signal(&parent, signal);
        }
        wakeup_first_blocked_task(&parent);
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
        wakeup_first_blocked_task(proc);
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
            inner.handle_default_action(signal);
            let action = signal.default_action();
            match action {
                SignalAction::Terminate | SignalAction::Core => {
                    let core_dump = matches!(action, SignalAction::Core);
                    drop(inner);
                    finish_signaled_process(proc, signal, core_dump);
                }
                SignalAction::Stop => {
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
                    drop(inner);
                    wakeup_first_blocked_task(proc);
                }
            }
            0
        }
        SigHandler::Custom(_) => {
            // 用户自定义，标记为需要处理
            inner.pending_signals.add(signal);
            inner.need_signal_handle = true;
            drop(inner);
            wakeup_first_blocked_task(proc);
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
    let _process = current_process();
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

            // 解除阻塞后，检查是否有待处理的信号（线程级 + 进程级）
            if how == 1 || how == 2 {
                let ready = t_inner.pending_signals.bits() & !t_inner.blocked_signals.bits();
                if ready != 0 {
                    t_inner.need_signal_handle = true;
                }
            }
        }
    }

    if let Some(mask) = old_mask {
        *translated_refmut(token, oldset as *mut u64)? = mask;
    }

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

#[cfg(target_arch = "riscv64")]
/// ========== 5. handle_pending_signals ==========
/// 在返回用户态前检查并投递异步信号。
/// 从进程级 pending_signals 中取出第一个未被阻塞的信号，
/// 如果是自定义 handler，则修改 TrapFrame 并保存上下文到 PCB 的栈。
pub fn handle_pending_signals() {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    if !inner.need_signal_handle {
        return;
    }
    let pending = inner.pending_signals.bits() & !inner.blocked_signals.bits();
    if pending == 0 {
        inner.need_signal_handle = false;
        return;
    }
    let idx = pending.trailing_zeros() as usize;
    let signo = (idx + 1) as i32;
    let signal = match Signal::from_i32(signo) {
        Some(s) => s,
        None => {
            inner.need_signal_handle = false;
            return;
        }
    };
    let action = inner.signals_handler.get(signal);
    if let SigHandler::Custom(handler) = action.sa_handler {
        let trap_cx = current_trap_cx();
        let original_sepc = trap_cx.pc();
        let original_sstatus = trap_cx.sstatus;
        let original_fsx = trap_cx.fsx;
        let original_x: [usize; 32] = trap_cx.x;
        let saved_mask = inner.blocked_signals;

        trap_cx[polyhal_trap::trapframe::TrapFrameArgs::SEPC] = handler as usize;
        trap_cx[polyhal_trap::trapframe::TrapFrameArgs::ARG0] = signo as usize;
        if action.sa_restorer != 0 {
            trap_cx[polyhal_trap::trapframe::TrapFrameArgs::RA] = action.sa_restorer;
        }

        // 统一在用户栈构建信号帧（Linux 风格，避免 longjmp 导致内核内存泄漏）
        const SIGINFO_SIZE: usize = 128;
        const UCONTEXT_SIZE: usize = 960;
        const SIGFRAME_SIZE: usize = SIGINFO_SIZE + UCONTEXT_SIZE + 8;
        // addi a7, zero, 139; ecall
        const RESTORER_CODE: [u8; 8] = [0x93, 0x08, 0xb0, 0x08, 0x73, 0x00, 0x00, 0x00];

        let sp = trap_cx[polyhal_trap::trapframe::TrapFrameArgs::SP];
        let new_sp = sp.saturating_sub(SIGFRAME_SIZE);
        let token = inner.vm_set.page_table.token();

        let mut frame = [0u8; SIGFRAME_SIZE];
        frame[0..4].copy_from_slice(&signo.to_ne_bytes());

        let mask = saved_mask.bits();
        frame[SIGINFO_SIZE + 40..SIGINFO_SIZE + 48].copy_from_slice(&mask.to_ne_bytes());

        let mcontext_base = SIGINFO_SIZE + 176;
        frame[mcontext_base..mcontext_base + 8].copy_from_slice(&original_sepc.to_ne_bytes());
        for i in 1..32 {
            let offset = mcontext_base + i * 8;
            frame[offset..offset + 8].copy_from_slice(&original_x[i].to_ne_bytes());
        }
        frame[mcontext_base + 256..mcontext_base + 264]
            .copy_from_slice(&original_sstatus.bits().to_ne_bytes());
        frame[mcontext_base + 264..mcontext_base + 272]
            .copy_from_slice(&original_fsx[0].to_ne_bytes());
        frame[mcontext_base + 272..mcontext_base + 280]
            .copy_from_slice(&original_fsx[1].to_ne_bytes());

        frame[SIGINFO_SIZE + UCONTEXT_SIZE..SIGFRAME_SIZE].copy_from_slice(&RESTORER_CODE);

        let bufs = match translated_byte_buffer(token, new_sp as *const u8, SIGFRAME_SIZE) {
            Ok(bufs) => bufs,
            Err(_) => return,
        };
        let mut written = 0;
        for buf in bufs {
            let len = buf.len().min(SIGFRAME_SIZE - written);
            buf[..len].copy_from_slice(&frame[written..written + len]);
            written += len;
        }

        trap_cx[polyhal_trap::trapframe::TrapFrameArgs::SP] = new_sp;
        trap_cx[polyhal_trap::trapframe::TrapFrameArgs::ARG1] = new_sp;
        trap_cx[polyhal_trap::trapframe::TrapFrameArgs::ARG2] = new_sp + SIGINFO_SIZE;

        if action.sa_restorer == 0 {
            trap_cx[polyhal_trap::trapframe::TrapFrameArgs::RA] =
                new_sp + SIGINFO_SIZE + UCONTEXT_SIZE;
        }

        let mut new_mask = inner.blocked_signals.bits() | action.sa_mask.bits();
        if (action.sa_flags & 0x40000000) == 0 {
            // SA_NODEFER = 0x40000000
            new_mask |= 1 << (signo - 1);
        }
        inner.blocked_signals = SignalSet::from_bits(new_mask);

        inner.pending_signals.remove(signal);
        inner.need_signal_handle =
            (inner.pending_signals.bits() & !inner.blocked_signals.bits()) != 0;
    } else {
        // Default 或 Ignore：清除 pending
        inner.pending_signals.remove(signal);
        inner.need_signal_handle =
            (inner.pending_signals.bits() & !inner.blocked_signals.bits()) != 0;
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
#[cfg(target_arch = "riscv64")]
/// ========== 6. sys_rt_sigreturn (139) ==========
/// 从信号 handler 恢复用户态上下文。
/// 对于 SA_SIGINFO 帧，从用户栈的 ucontext 读取可能修改过的寄存器和掩码；
/// 对于非 SA_SIGINFO 帧，从 PCB 的 sig_context_stack 弹出保存的 TrapFrame。
pub fn sys_rt_sigreturn() -> SyscallResult {
    const SIGINFO_SIZE: usize = 128;
    #[allow(dead_code)]
    const UCONTEXT_SIZE: usize = 960;
    #[allow(dead_code)]
    const SIGFRAME_SIZE: usize = SIGINFO_SIZE + UCONTEXT_SIZE + 8; // +8 for restorer code

    let task = current_task().unwrap();
    let token = current_user_token();
    let current_sp = current_trap_cx()[polyhal_trap::trapframe::TrapFrameArgs::SP];

    // 从用户栈读取 uc_sigmask
    let sigmask_addr = current_sp + SIGINFO_SIZE + 40;
    let bufs = translated_byte_buffer(token, sigmask_addr as *const u8, 16)?;
    let mut bytes = [0u8; 16];
    let mut copied = 0;
    for buf in bufs {
        let len = buf.len().min(16 - copied);
        bytes[copied..copied + len].copy_from_slice(&buf[..len]);
        copied += len;
    }
    let mask_val = u64::from_ne_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]);
    let restored_mask = SignalSet::from_bits(mask_val);

    // 从用户栈读取 __gregs[0..32]、sstatus、fsx
    let mcontext_addr = current_sp + SIGINFO_SIZE + 176;
    let bufs = translated_byte_buffer(token, mcontext_addr as *const u8, 280)?;
    let mut mcontext_bytes = [0u8; 280];
    let mut copied = 0;
    for buf in bufs {
        let len = buf.len().min(280 - copied);
        mcontext_bytes[copied..copied + len].copy_from_slice(&buf[..len]);
        copied += len;
    }

    let mut gregs = [0u64; 32];
    for i in 0..32 {
        gregs[i] = u64::from_ne_bytes([
            mcontext_bytes[i * 8],
            mcontext_bytes[i * 8 + 1],
            mcontext_bytes[i * 8 + 2],
            mcontext_bytes[i * 8 + 3],
            mcontext_bytes[i * 8 + 4],
            mcontext_bytes[i * 8 + 5],
            mcontext_bytes[i * 8 + 6],
            mcontext_bytes[i * 8 + 7],
        ]);
    }
    let sstatus_bits = usize::from_ne_bytes([
        mcontext_bytes[256],
        mcontext_bytes[257],
        mcontext_bytes[258],
        mcontext_bytes[259],
        mcontext_bytes[260],
        mcontext_bytes[261],
        mcontext_bytes[262],
        mcontext_bytes[263],
    ]);
    let fsx0 = usize::from_ne_bytes([
        mcontext_bytes[264],
        mcontext_bytes[265],
        mcontext_bytes[266],
        mcontext_bytes[267],
        mcontext_bytes[268],
        mcontext_bytes[269],
        mcontext_bytes[270],
        mcontext_bytes[271],
    ]);
    let fsx1 = usize::from_ne_bytes([
        mcontext_bytes[272],
        mcontext_bytes[273],
        mcontext_bytes[274],
        mcontext_bytes[275],
        mcontext_bytes[276],
        mcontext_bytes[277],
        mcontext_bytes[278],
        mcontext_bytes[279],
    ]);

    let mut t_inner = task.inner_exclusive_access();
    t_inner.blocked_signals = restored_mask;
    t_inner.need_signal_handle = (t_inner.pending_signals.bits() & !restored_mask.bits()) != 0;
    // 如果是从 sigsuspend 返回，恢复 sigsuspend 之前的旧掩码
    if let Some(old_mask) = t_inner.sigsuspend_old_mask.take() {
        t_inner.blocked_signals = old_mask;
        t_inner.need_signal_handle = (t_inner.pending_signals.bits() & !old_mask.bits()) != 0;
    }
    drop(t_inner);

    let trap_cx = current_trap_cx();
    // trap_cx.sepc = gregs[0] as usize;
    trap_cx.set_pc(gregs[0] as usize);

    for i in 1..32 {
        trap_cx.x[i] = gregs[i] as usize;
    }
    trap_cx.x[0] = 0;
    // sstatus 和 fsx 从用户栈帧的扩展区域恢复
    trap_cx.sstatus = unsafe { core::mem::transmute(sstatus_bits) };
    trap_cx.fsx = [fsx0, fsx1];

    Ok(gregs[10] as usize)
}

/// 在 trap 返回用户态前投递 pending 信号
///
/// 找到第一个 pending 且未被阻塞的信号，根据 handler 类型处理：
/// - Ignore：直接清除
/// - Default：调用 handle_default_action，必要时标记进程退出
/// - Custom：保存 TrapFrame 到 sig_context_stack，修改 ctx 跳转到用户态 handler
pub fn handle_signals(ctx: &mut polyhal_trap::trapframe::TrapFrame) {
    let task = match crate::task::current_task() {
        Some(t) => t,
        None => {
            trace!("handle_signals: current_task is None, skipping");
            return;
        }
    };
    let process = match task.process.upgrade() {
        Some(p) => p,
        None => {
            trace!(
                "handle_signals: process is None for tid={}, skipping",
                task.inner_exclusive_access()
                    .res
                    .as_ref()
                    .map(|r| r.tid)
                    .unwrap_or(999)
            );
            return;
        }
    };

    let (task_tid, task_pending, task_blocked, task_needs_signal) = {
        let t_inner = task.inner_exclusive_access();
        (
            t_inner.res.as_ref().map(|r| r.tid).unwrap_or(999),
            t_inner.pending_signals,
            t_inner.blocked_signals,
            t_inner.need_signal_handle,
        )
    };
    let (proc_pending, proc_needs_signal) = {
        let p_inner = process.inner_exclusive_access();
        (p_inner.pending_signals, p_inner.need_signal_handle)
    };

    if !task_needs_signal && !proc_needs_signal {
        return;
    }

    let mut pending = task_pending.bits() & !task_blocked.bits();
    trace!(
        "handle_signals: tid={}, task_pending={:#x}, task_blocked={:#x}, proc_pending={:#x}, pending={:#x}",
        task_tid,
        task_pending.bits(),
        task_blocked.bits(),
        proc_pending.bits(),
        pending
    );
    let mut is_task_level = true;
    if pending == 0 {
        pending = proc_pending.bits() & !task_blocked.bits();
        is_task_level = false;
    }

    if pending == 0 {
        let mut t_inner = task.inner_exclusive_access();
        t_inner.need_signal_handle = false;
        drop(t_inner);
        let mut p_inner = process.inner_exclusive_access();
        p_inner.need_signal_handle = false;
        return;
    }

    let mut target_sig = None;
    let mut target_action = SigAction::default();
    let mut last_siginfo = None;
    let mut token = 0usize;
    {
        let p_inner = process.inner_exclusive_access();
        for i in 1..64 {
            let signal = match Signal::from_i32(i) {
                Some(s) => s,
                None => continue,
            };
            let in_pending = if is_task_level {
                task_pending.contains(signal)
            } else {
                proc_pending.contains(signal)
            };
            if in_pending && !task_blocked.contains(signal) {
                target_sig = Some(signal);
                target_action = p_inner.signals_handler.get(signal);
                last_siginfo = p_inner.last_siginfo;
                token = p_inner.vm_set.page_table.token();
                break;
            }
        }
    }

    let signal = match target_sig {
        Some(signal) => signal,
        None => return,
    };

    let handler_addr = target_action.sa_handler.as_ptr() as usize;
    let restorer_addr = target_action.sa_restorer;
    let sa_mask = target_action.sa_mask;
    match target_action.sa_handler {
        crate::task::signal::SigHandler::Ignore => {
            if is_task_level {
                let mut t_inner = task.inner_exclusive_access();
                t_inner.pending_signals.remove(signal);
                t_inner.need_signal_handle =
                    (t_inner.pending_signals.bits() & !t_inner.blocked_signals.bits()) != 0;
            } else {
                let mut p_inner = process.inner_exclusive_access();
                p_inner.pending_signals.remove(signal);
                p_inner.need_signal_handle =
                    (p_inner.pending_signals.bits() & !task_blocked.bits()) != 0;
            }
        }
        crate::task::signal::SigHandler::Default => {
            if is_task_level {
                let mut t_inner = task.inner_exclusive_access();
                t_inner.pending_signals.remove(signal);
                t_inner.need_signal_handle =
                    (t_inner.pending_signals.bits() & !t_inner.blocked_signals.bits()) != 0;
            } else {
                let mut p_inner = process.inner_exclusive_access();
                p_inner.pending_signals.remove(signal);
                p_inner.need_signal_handle =
                    (p_inner.pending_signals.bits() & !task_blocked.bits()) != 0;
            }

            if let crate::task::signal::SignalAction::Terminate
            | crate::task::signal::SignalAction::Core = signal.default_action()
            {
                let core_dump = matches!(
                    signal.default_action(),
                    crate::task::signal::SignalAction::Core
                );
                finish_signaled_process(&process, signal, core_dump);
            } else {
                let mut p_inner = process.inner_exclusive_access();
                p_inner.handle_default_action(signal);
            }
        }
        crate::task::signal::SigHandler::Custom(handler) => {
            // 读取原始上下文，用于构建用户栈信号帧（Linux 风格）
            let original_sepc = ctx.pc();
            let original_sstatus = ctx.sstatus;
            let original_fsx = ctx.fsx;
            let original_x: [usize; 32] = ctx.x;
            let saved_mask = task_blocked;

            // 统一在用户栈构建信号帧（无论是否 SA_SIGINFO）
            const SIGINFO_SIZE: usize = 128;
            const UCONTEXT_SIZE: usize = 960;
            const SIGFRAME_SIZE: usize = SIGINFO_SIZE + UCONTEXT_SIZE + 8;
            // addi a7, zero, 139; ecall
            const RESTORER_CODE: [u8; 8] = [0x93, 0x08, 0xb0, 0x08, 0x73, 0x00, 0x00, 0x00];

            let sp = ctx[TrapFrameArgs::SP];
            let new_sp = sp.saturating_sub(SIGFRAME_SIZE);

            // 构建信号帧内容（清零后填充关键字段）
            let mut frame = [0u8; SIGFRAME_SIZE];
            // siginfo_t at offset 0
            if let Some(ref siginfo) = last_siginfo {
                frame[0..4].copy_from_slice(&siginfo.si_signo.to_ne_bytes());
                frame[4..8].copy_from_slice(&siginfo.si_errno.to_ne_bytes());
                frame[8..12].copy_from_slice(&siginfo.si_code.to_ne_bytes());
                frame[16..20].copy_from_slice(&siginfo.si_pid.to_ne_bytes());
                frame[20..24].copy_from_slice(&(siginfo.si_uid as i32).to_ne_bytes());
                let mut val_bytes = [0u8; 8];
                val_bytes[0..4].copy_from_slice(&siginfo.si_value.to_ne_bytes());
                frame[24..32].copy_from_slice(&val_bytes);
            } else {
                frame[0..4].copy_from_slice(&signal.as_i32().to_ne_bytes());
            }

            // ucontext_t at offset SIGINFO_SIZE (128)
            // uc_sigmask at ucontext + 40 (128 bytes in musl)
            let mask = saved_mask.bits();
            frame[SIGINFO_SIZE + 40..SIGINFO_SIZE + 48].copy_from_slice(&mask.to_ne_bytes());

            // uc_mcontext at ucontext + 176
            let mcontext_base = SIGINFO_SIZE + 176;
            // __gregs[0] (PC) = original sepc
            frame[mcontext_base..mcontext_base + 8].copy_from_slice(&original_sepc.to_ne_bytes());
            // __gregs[1..31] = original x[1..31]
            for i in 1..32 {
                let offset = mcontext_base + i * 8;
                frame[offset..offset + 8].copy_from_slice(&original_x[i].to_ne_bytes());
            }
            // 扩展：保存 sstatus 和 fsx（紧跟在 __gregs 之后）
            frame[mcontext_base + 256..mcontext_base + 264]
                .copy_from_slice(&original_sstatus.bits().to_ne_bytes());
            frame[mcontext_base + 264..mcontext_base + 272]
                .copy_from_slice(&original_fsx[0].to_ne_bytes());
            frame[mcontext_base + 272..mcontext_base + 280]
                .copy_from_slice(&original_fsx[1].to_ne_bytes());

            // restorer code at the end of the frame
            frame[SIGINFO_SIZE + UCONTEXT_SIZE..SIGFRAME_SIZE].copy_from_slice(&RESTORER_CODE);

            // Write to user stack
            let bufs = match translated_byte_buffer(token, new_sp as *const u8, SIGFRAME_SIZE) {
                Ok(bufs) => bufs,
                Err(_) => return,
            };
            let mut written = 0;
            for buf in bufs {
                let len = buf.len().min(SIGFRAME_SIZE - written);
                buf[..len].copy_from_slice(&frame[written..written + len]);
                written += len;
            }

            // 修改 TrapFrame 以跳转到用户态信号处理函数
            use polyhal_trap::trapframe::TrapFrameArgs;
            ctx[TrapFrameArgs::SEPC] = handler as usize;
            ctx[TrapFrameArgs::ARG0] = signal.as_i32() as usize;
            if restorer_addr != 0 {
                ctx[TrapFrameArgs::RA] = restorer_addr;
            }
            ctx[TrapFrameArgs::SP] = new_sp;
            ctx[TrapFrameArgs::ARG1] = new_sp; // a1 = &siginfo
            ctx[TrapFrameArgs::ARG2] = new_sp + SIGINFO_SIZE; // a2 = &ucontext

            // 提供内核 restorer（如果用户没有设置 sa_restorer）
            if restorer_addr == 0 {
                ctx[TrapFrameArgs::RA] = new_sp + SIGINFO_SIZE + UCONTEXT_SIZE;
            }

            // 屏蔽当前信号和 sa_mask
            let mut t_inner = task.inner_exclusive_access();
            t_inner.blocked_signals.add(signal);
            t_inner.blocked_signals |= sa_mask;

            // 清除该信号的 pending 状态
            if is_task_level {
                t_inner.pending_signals.remove(signal);
                t_inner.need_signal_handle =
                    (t_inner.pending_signals.bits() & !t_inner.blocked_signals.bits()) != 0;
            } else {
                let blocked = t_inner.blocked_signals.bits();
                drop(t_inner);
                let mut p_inner = process.inner_exclusive_access();
                p_inner.pending_signals.remove(signal);
                p_inner.need_signal_handle = (p_inner.pending_signals.bits() & !blocked) != 0;
            }

            info!(
                "handle_signals: current_tid={}, task_pending={:#x}, proc_pending={:#x}, deliver signal {} to handler {:#x}, restorer {:#x}",
                task_tid,
                task_pending.bits(),
                proc_pending.bits(),
                signal.as_i32(),
                handler_addr,
                restorer_addr
            );
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
        current_process().pid.0,
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
            let ticks =
                (value_usec as usize).saturating_mul(crate::config::_CLOCK_FREQ) / 1_000_000;
            Some(crate::timer::get_time().saturating_add(ticks))
        } else {
            None
        };
        let interval = if interval_usec > 0 {
            Some((interval_usec as usize).saturating_mul(crate::config::_CLOCK_FREQ) / 1_000_000)
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
            .insert(process.getpid(), Arc::downgrade(&process));
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
        let remaining_us = if let Some(deadline) = inner.alarm_deadline_us {
            deadline.saturating_sub(current_time().as_micros() as u128)
        } else {
            0
        };
        (remaining_us, inner.alarm_interval_us.unwrap_or(0))
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
