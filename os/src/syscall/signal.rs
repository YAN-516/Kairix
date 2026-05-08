// src/signal/syscall.rs
use crate::error::{SysError, SyscallResult};
use crate::mm::{translated_ref, translated_refmut};
use crate::syscall::time::TimeVal;
use crate::task::signal::*;
use crate::task::*;
#[cfg(target_arch = "riscv64")]
use crate::timer::get_time_us;
use crate::trap::_set_sum_bit;
use alloc::sync::Arc;
use log::{error, info, trace};
use polyhal::println;
use polyhal::timer::current_time;
use polyhal_trap::trapframe::TrapFrameArgs;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct LinuxRtSigAction {
    handler: usize,
    flags: usize,
    restorer: usize,
    mask: u64,
}

fn kernel_to_linux_sigaction(action: SigAction) -> LinuxRtSigAction {
    LinuxRtSigAction {
        handler: action.sa_handler.as_ptr() as usize,
        flags: action.sa_flags as usize,
        restorer: action.sa_restorer,
        mask: action.sa_mask.bits(),
    }
}

fn linux_to_kernel_sigaction(action: LinuxRtSigAction) -> SigAction {
    SigAction {
        sa_handler: unsafe { SigHandler::from_ptr(action.handler as *const core::ffi::c_void) },
        sa_mask: SignalSet::from_bits(action.mask),
        sa_flags: action.flags as u32,
        sa_restorer: action.restorer,
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
    it_interval: super::time::TimeVal,
    it_value: super::time::TimeVal,
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
    info!(
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
        )))
    } else {
        None
    };

    let mut old_action = None;
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
                // 同时清除当前线程的 pending
                let task = current_task().unwrap();
                task.inner_exclusive_access().pending_signals.remove(signal);
            }
        }
    }

    if let Some(old) = old_action {
        if oldact != 0 {
            *translated_refmut(token, oldact as *mut LinuxRtSigAction) =
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
    let current = current_process();

    // 检查信号编号（有效范围 1..=64）
    if sig > 64 {
        return Err(SysError::EINVAL);
    }

    // 查找目标进程
    let target = {
        if pid > 0 {
            match pid2process(pid as usize) {
                Some(t) => t,
                None => return Err(SysError::ESRCH),
            }
        } else if pid == 0 {
            // 同一进程组（简化：发给自己）
            current
        } else if pid == -1 {
            // 所有进程（简化：只发给自己）
            current
        } else {
            // pid < -1: 指定进程组（简化）
            current
        }
    };
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
    deliver_signal(&target, signal);
    Ok(0)
}

/// tgkill: send a signal to a specific thread in a thread group.
/// Since Kairix handles signals at process granularity, we verify that
/// the given tid exists inside the target process and then deliver.
pub fn sys_tkill(tid: isize, sig: usize) -> SyscallResult {
    _set_sum_bit();
    info!("sys_tkill: tid={}, sig={}", tid, sig);
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

    // Verify the tid belongs to this process
    let target_task = {
        let inner = process.inner_exclusive_access();
        if (tid as usize) < inner.tasks.len() {
            inner.tasks[tid as usize].as_ref().cloned()
        } else {
            None
        }
    };

    let target_task = match target_task {
        Some(t) => t,
        None => return Err(SysError::ESRCH),
    };

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
            let _ = t_inner.pending_signals.add(signal);
            t_inner.need_signal_handle = true;
            info!(
                "sys_tkill: Custom handler -> added sig {} to target_task tid={} pending={:#x}",
                signal.as_i32(),
                t_inner.res.as_ref().map(|r| r.tid).unwrap_or(999),
                t_inner.pending_signals.mask_bits()
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

    // Verify the tid belongs to this process
    let inner = target_proc.inner_exclusive_access();
    let tid_exists = (tid as usize) < inner.tasks.len() && inner.tasks[tid as usize].is_some();
    drop(inner);

    if !tid_exists {
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
    let target_task = {
        let inner = target_proc.inner_exclusive_access();
        if let Some(Some(target_task)) = inner.tasks.get(tid as usize) {
            let target_task = target_task.clone();
            let mut t_inner = target_task.inner_exclusive_access();
            t_inner.interrupted_by_signal = true;
            let is_blocked = t_inner.task_status == crate::task::TaskStatus::Blocked;
            drop(t_inner);
            drop(inner);
            if is_blocked {
                crate::task::wakeup_task(target_task.clone());
            }
            Some(target_task)
        } else {
            None
        }
    };

    // 对于自定义 handler 的线程定向信号，投递到目标线程的 pending；
    // 对于 Default / Ignore / SIGKILL / SIGSTOP，走进程级 deliver_signal。
    let action = {
        let p_inner = target_proc.inner_exclusive_access();
        p_inner.signals_handler.get(signal)
    };
    match action.sa_handler {
        SigHandler::Custom(_) => {
            if let Some(target_task) = target_task {
                let mut t_inner = target_task.inner_exclusive_access();
                let _ = t_inner.pending_signals.add(signal);
                t_inner.need_signal_handle = true;
            }
        }
        _ => {
            deliver_signal(&target_proc, signal);
        }
    }
    Ok(0)
}

/// 检查当前任务的阻塞系统调用是否应该被信号中断（返回 -EINTR）。
/// Linux 标准行为：
/// - 如果有 pending 的、未被阻塞的、非忽略的信号
/// - 且信号的 handler 是 Default（终止行为）或 Custom 但没有 SA_RESTART 标志
/// - 则系统调用应该返回 -EINTR
pub fn should_interrupt_syscall() -> bool {
    let task = match current_task() {
        Some(t) => t,
        None => return false,
    };
    let t_inner = task.inner_exclusive_access();
    let blocked = t_inner.blocked_signals.bits();

    if let Some(process) = task.process.upgrade() {
        let p_inner = process.inner_exclusive_access();
        let pending =
            (t_inner.pending_signals.mask_bits() | p_inner.pending_signals.mask_bits()) & !blocked;

        if pending == 0 {
            return false;
        }

        for i in 1..=64 {
            if (pending >> (i - 1)) & 1 != 0 {
                if let Some(sig) = Signal::from_i32(i) {
                    let action = p_inner.signals_handler.get(sig);
                    match action.sa_handler {
                        SigHandler::Ignore => {}
                        SigHandler::Default => {
                            if sig.default_action() != SignalAction::Ignore {
                                return true;
                            }
                        }
                        SigHandler::Custom(_) => {
                            if (action.sa_flags & SA_RESTART) == 0 {
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

/// 唤醒目标进程中所有处于 Blocked 状态的任务，并标记为被信号中断。
/// 修复：多线程进程可能有多个线程同时阻塞在 IO/等待上，只唤醒一个会导致其他线程永远阻塞。
#[allow(dead_code)]
fn wakeup_all_blocked_tasks(proc: &Arc<ProcessControlBlock>) {
    let inner = proc.inner_exclusive_access();
    for task_opt in inner.tasks.iter() {
        if let Some(task) = task_opt {
            let mut t_inner = task.inner_exclusive_access();
            if t_inner.task_status == crate::task::TaskStatus::Blocked {
                t_inner.interrupted_by_signal = true;
                drop(t_inner);
                crate::task::wakeup_task(task.clone());
            }
        }
    }
}

/// 投递信号到进程。
/// 职责：路由 + 入队 + 唤醒。除 SIGKILL/SIGSTOP 外，不在这里执行 Default 动作，
/// 统一推迟到 handle_signals（返回用户态前）处理。
pub fn deliver_signal(proc: &Arc<ProcessControlBlock>, signal: Signal) -> isize {
    let mut inner = proc.inner_exclusive_access();
    // 特殊处理：SIGKILL 和 SIGSTOP 不能被阻塞、不能被忽略、不能捕获
    match signal {
        Signal::SigKill => {
            inner.is_zombie = true;
            inner
                .zombie_flag
                .store(true, core::sync::atomic::Ordering::SeqCst);
            inner.exit_code = 128 + signal.as_i32();
            for task_opt in inner.tasks.iter() {
                if let Some(task) = task_opt {
                    task.inner_exclusive_access()
                        .zombie_flag
                        .store(true, core::sync::atomic::Ordering::SeqCst);
                }
            }
            drop(inner);
            wakeup_all_blocked_tasks(proc);
            if let Some(current_task) = crate::task::current_task() {
                if let Some(current_proc) = current_task.process.upgrade() {
                    if Arc::ptr_eq(proc, &current_proc) {
                        crate::task::exit_current_and_run_next(128 + signal.as_i32());
                    }
                }
            }
            return 0;
        }
        Signal::SigStop => {
            inner.state = crate::task::process::ProcessStatus::Terminal;
            inner.is_stopped = true;
            drop(inner);
            wakeup_all_blocked_tasks(proc);
            return 0;
        }
        _ => {}
    }

    let action = inner.signals_handler.get(signal);
    match action.sa_handler {
        SigHandler::Ignore => {
            drop(inner);
            0
        }
        SigHandler::Default | SigHandler::Custom(_) => {
            // 统一入 pending，由 handle_signals 在返回用户态前处理
            let _ = inner.pending_signals.add(signal);
            inner.need_signal_handle = true;
            drop(inner);
            wakeup_all_blocked_tasks(proc);
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
        let bits = *translated_ref(token, set as *const u64);
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
                let task_ready = t_inner
                    .pending_signals
                    .available_bits(t_inner.blocked_signals);
                let proc_ready = if let Some(process) = task.process.upgrade() {
                    process
                        .inner_exclusive_access()
                        .pending_signals
                        .available_bits(t_inner.blocked_signals)
                } else {
                    0
                };
                if task_ready != 0 || proc_ready != 0 {
                    t_inner.need_signal_handle = true;
                }
            }
        }
    }

    if let Some(mask) = old_mask {
        *translated_refmut(token, oldset as *mut u64) = mask;
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
    let wait_set = SignalSet::from_bits(*translated_ref(token, set as *const u64));

    let deadline_us = if timeout != 0 {
        let ts = *translated_ref(token, timeout as *const LinuxTimeSpec);
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
        let mut t_inner = task.inner_exclusive_access();

        // 优先从线程级队列中匹配，再检查进程级队列
        // rt_sigtimedwait 可以捕获被阻塞的信号，因此匹配时不检查 blocked
        let blocked = t_inner.blocked_signals;
        let sig = if let Some(s) = t_inner
            .pending_signals
            .dequeue_matching(SignalSet::empty(), wait_set)
        {
            t_inner.need_signal_handle = t_inner.pending_signals.available_bits(blocked) != 0;
            Some(s)
        } else if let Some(s) = p_inner
            .pending_signals
            .dequeue_matching(SignalSet::empty(), wait_set)
        {
            p_inner.need_signal_handle = p_inner.pending_signals.available_bits(blocked) != 0;
            Some(s)
        } else {
            t_inner.need_signal_handle = t_inner.pending_signals.available_bits(blocked) != 0;
            p_inner.need_signal_handle = p_inner.pending_signals.available_bits(blocked) != 0;
            None
        };

        if let Some(sig) = sig {
            drop(t_inner);
            drop(p_inner);
            if info != 0 {
                *translated_refmut(token, info as *mut LinuxSigInfo) =
                    LinuxSigInfo::new(sig.as_i32());
            }
            return Ok(sig.as_i32() as usize);
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
            let task_pending =
                t_inner.pending_signals.mask_bits() & !t_inner.blocked_signals.bits();
            if task_pending != 0 {
                return Err(SysError::EINTR);
            }
        }
        {
            let p_inner = process.inner_exclusive_access();
            let t_inner = task.inner_exclusive_access();
            let proc_pending =
                p_inner.pending_signals.mask_bits() & !t_inner.blocked_signals.bits();
            if proc_pending != 0 {
                return Err(SysError::EINTR);
            }
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
        let bits = *translated_ref(token, mask_ptr as *const u64);
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
            let task_pending =
                t_inner.pending_signals.mask_bits() & !t_inner.blocked_signals.bits();
            if task_pending != 0 {
                return Err(SysError::EINTR);
            }
        }
        {
            let p_inner = process.inner_exclusive_access();
            let t_inner = task.inner_exclusive_access();
            let proc_pending =
                p_inner.pending_signals.mask_bits() & !t_inner.blocked_signals.bits();
            if proc_pending != 0 {
                return Err(SysError::EINTR);
            }
        }
        block_current_and_run_next();
        // 被强制终止信号或被非 SA_RESTART 信号中断后应直接返回 -EINTR
        if current_process().inner_exclusive_access().is_zombie || should_interrupt_syscall() {
            return Err(SysError::EINTR);
        }
    }
}

/// ========== 6. sys_rt_sigreturn (139) ==========
/// 从信号 handler 恢复用户态上下文。
/// 对于 SA_SIGINFO 帧，从用户栈的 ucontext 读取可能修改过的寄存器和掩码；
/// 对于非 SA_SIGINFO 帧，从 PCB 的 sig_context_stack 弹出保存的 TrapFrame。
pub fn sys_rt_sigreturn() -> SyscallResult {
    const SIGINFO_SIZE: usize = 128;
    const UCONTEXT_SIZE: usize = 960;
    const SIGFRAME_SIZE: usize = SIGINFO_SIZE + UCONTEXT_SIZE + 8; // +8 for restorer code

    let task = current_task().unwrap();
    let mut t_inner = task.inner_exclusive_access();
    if let Some((saved_tf, saved_mask, flags)) = t_inner.sig_context_stack.pop() {
        let is_siginfo = (flags & SA_SIGINFO) != 0;
        // 先释放 t_inner，避免 current_trap_cx() 尝试重入同一把锁导致死锁
        drop(t_inner);

        let (restored_mask, gregs) = if is_siginfo {
            let token = current_user_token();
            let original_sp = saved_tf[polyhal_trap::trapframe::TrapFrameArgs::SP];
            // 下溢保护：如果 original_sp 小于帧大小，无法读取有效帧
            if original_sp < SIGFRAME_SIZE {
                error!("sys_rt_sigreturn: original_sp underflow, falling back");
                (saved_mask, None)
            } else {
                let frame_base = original_sp - SIGFRAME_SIZE;
                let bufs = crate::mm::translated_byte_buffer_no_fault(
                    token,
                    frame_base as *const u8,
                    SIGFRAME_SIZE,
                );
                if bufs.is_empty() {
                    error!("sys_rt_sigreturn: cannot read sigframe from user stack, falling back");
                    (saved_mask, None)
                } else {
                    let mut frame = [0u8; SIGFRAME_SIZE];
                    let mut copied = 0;
                    for buf in bufs {
                        let len = buf.len().min(SIGFRAME_SIZE - copied);
                        frame[copied..copied + len].copy_from_slice(&buf[..len]);
                        copied += len;
                    }

                    // uc_sigmask at ucontext + 40
                    let mask_val = u64::from_ne_bytes([
                        frame[SIGINFO_SIZE + 40],
                        frame[SIGINFO_SIZE + 41],
                        frame[SIGINFO_SIZE + 42],
                        frame[SIGINFO_SIZE + 43],
                        frame[SIGINFO_SIZE + 44],
                        frame[SIGINFO_SIZE + 45],
                        frame[SIGINFO_SIZE + 46],
                        frame[SIGINFO_SIZE + 47],
                    ]);

                    // uc_mcontext.__gregs[0..32] at ucontext + 176
                    let mcontext_base = SIGINFO_SIZE + 176;
                    let mut gregs = [0u64; 32];
                    for i in 0..32 {
                        let offset = mcontext_base + i * 8;
                        gregs[i] = u64::from_ne_bytes([
                            frame[offset],
                            frame[offset + 1],
                            frame[offset + 2],
                            frame[offset + 3],
                            frame[offset + 4],
                            frame[offset + 5],
                            frame[offset + 6],
                            frame[offset + 7],
                        ]);
                    }

                    (SignalSet::from_bits(mask_val), Some(gregs))
                }
            }
        } else {
            (saved_mask, None)
        };

        let mut t_inner = task.inner_exclusive_access();
        t_inner.blocked_signals = restored_mask;
        t_inner.need_signal_handle = t_inner.pending_signals.available_bits(restored_mask) != 0;
        // 如果是从 sigsuspend 返回，恢复 sigsuspend 之前的旧掩码
        if let Some(old_mask) = t_inner.sigsuspend_old_mask.take() {
            t_inner.blocked_signals = old_mask;
            t_inner.need_signal_handle = t_inner.pending_signals.available_bits(old_mask) != 0;
        }
        drop(t_inner);

        let trap_cx = current_trap_cx();
        let ret_val = if let Some(gregs) = gregs {
            // 从用户栈的 uc_mcontext 恢复
            trap_cx.sepc = gregs[0] as usize;
            for i in 1..32 {
                trap_cx.x[i] = gregs[i] as usize;
            }
            trap_cx.x[0] = 0;
            // sstatus 和 fsx 从 saved_tf 恢复（uc_mcontext 不包含它们）
            trap_cx.sstatus = saved_tf.sstatus;
            trap_cx.fsx = saved_tf.fsx;
            gregs[10] as usize // 恢复后的 a0
        } else {
            let ret = saved_tf[polyhal_trap::trapframe::TrapFrameArgs::RET];
            *trap_cx = saved_tf;
            ret
        };
        Ok(ret_val)
    } else {
        Err(SysError::EINVAL)
    }
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

    let mut p_inner = process.inner_exclusive_access();
    let mut t_inner = task.inner_exclusive_access();

    // 快速检查：如果没有 pending 信号需要处理，直接返回
    if !t_inner.need_signal_handle && !p_inner.need_signal_handle {
        drop(t_inner);
        drop(p_inner);
        return;
    }

    let task_tid = t_inner.res.as_ref().map(|r| r.tid).unwrap_or(999);

    // 从线程级或进程级队列中取出第一个未被阻塞的信号（FIFO 顺序）
    let blocked = t_inner.blocked_signals;
    let (signal, is_task_level) =
        if let Some(sig) = t_inner.pending_signals.dequeue_first_unblocked(blocked) {
            (sig, true)
        } else if let Some(sig) = p_inner.pending_signals.dequeue_first_unblocked(blocked) {
            (sig, false)
        } else {
            t_inner.need_signal_handle = false;
            p_inner.need_signal_handle = false;
            drop(t_inner);
            drop(p_inner);
            return;
        };

    let action = p_inner.signals_handler.get(signal);
    match action.sa_handler {
        crate::task::signal::SigHandler::Ignore => {
            if is_task_level {
                t_inner.need_signal_handle = t_inner
                    .pending_signals
                    .available_bits(t_inner.blocked_signals)
                    != 0;
            } else {
                p_inner.need_signal_handle = p_inner
                    .pending_signals
                    .available_bits(t_inner.blocked_signals)
                    != 0;
            }
            drop(t_inner);
            drop(p_inner);
        }
        crate::task::signal::SigHandler::Default => {
            if is_task_level {
                t_inner.need_signal_handle = t_inner
                    .pending_signals
                    .available_bits(t_inner.blocked_signals)
                    != 0;
            } else {
                p_inner.need_signal_handle = p_inner
                    .pending_signals
                    .available_bits(t_inner.blocked_signals)
                    != 0;
            }
            p_inner.handle_default_action(signal);
            if let crate::task::signal::SignalAction::Terminate
            | crate::task::signal::SignalAction::Core = signal.default_action()
            {
                p_inner.exit_code = 128 + signal.as_i32();
                for task_opt in p_inner.tasks.iter() {
                    if let Some(t) = task_opt {
                        crate::task::remove_inactive_task(Arc::clone(t));
                    }
                }
            }
            drop(t_inner);
            drop(p_inner);
        }
        crate::task::signal::SigHandler::Custom(handler) => {
            let saved_tf = ctx.clone();
            let saved_mask = t_inner.blocked_signals;
            let sa_flags = action.sa_flags;
            // 预先读取 saved_tf 中的字段，因为 push 会 move saved_tf
            let original_sepc = saved_tf.sepc;
            let original_x: [usize; 32] = saved_tf.x;
            t_inner
                .sig_context_stack
                .push((saved_tf, saved_mask, sa_flags));

            use polyhal_trap::trapframe::TrapFrameArgs;
            ctx[TrapFrameArgs::SEPC] = handler as usize;
            ctx[TrapFrameArgs::ARG0] = signal.as_i32() as usize;
            if action.sa_restorer != 0 {
                ctx[TrapFrameArgs::RA] = action.sa_restorer;
            }

            // 为 SA_SIGINFO handler 构建信号帧
            if (sa_flags & SA_SIGINFO) != 0 {
                const SIGINFO_SIZE: usize = 128;
                const UCONTEXT_SIZE: usize = 960;
                const SIGFRAME_SIZE: usize = SIGINFO_SIZE + UCONTEXT_SIZE + 8;
                const RESTORER_CODE: [u8; 8] = [0x93, 0x08, 0xb0, 0x08, 0x73, 0x00, 0x00, 0x00];

                let sp = ctx[TrapFrameArgs::SP];
                let new_sp = sp.saturating_sub(SIGFRAME_SIZE);
                let token = p_inner.vm_set.page_table.token();

                let mut frame = [0u8; SIGFRAME_SIZE];
                frame[0..4].copy_from_slice(&signal.as_i32().to_ne_bytes());

                let mask = saved_mask.bits();
                frame[SIGINFO_SIZE + 40..SIGINFO_SIZE + 48].copy_from_slice(&mask.to_ne_bytes());

                let mcontext_base = SIGINFO_SIZE + 176;
                frame[mcontext_base..mcontext_base + 8]
                    .copy_from_slice(&original_sepc.to_ne_bytes());
                for i in 1..32 {
                    let offset = mcontext_base + i * 8;
                    frame[offset..offset + 8].copy_from_slice(&original_x[i].to_ne_bytes());
                }

                frame[SIGINFO_SIZE + UCONTEXT_SIZE..SIGFRAME_SIZE].copy_from_slice(&RESTORER_CODE);

                let bufs = crate::mm::translated_byte_buffer_no_fault(
                    token,
                    new_sp as *const u8,
                    SIGFRAME_SIZE,
                );
                if bufs.is_empty() {
                    // 用户栈不可写（溢出或未映射），降级为非 SA_SIGINFO 路径
                    error!("handle_signals: user stack not writable for sigframe, falling back");
                } else {
                    let mut written = 0;
                    for buf in bufs {
                        let len = buf.len().min(SIGFRAME_SIZE - written);
                        buf[..len].copy_from_slice(&frame[written..written + len]);
                        written += len;
                    }

                    ctx[TrapFrameArgs::SP] = new_sp;
                    ctx[TrapFrameArgs::ARG1] = new_sp;
                    ctx[TrapFrameArgs::ARG2] = new_sp + SIGINFO_SIZE;

                    if action.sa_restorer == 0 {
                        ctx[TrapFrameArgs::RA] = new_sp + SIGINFO_SIZE + UCONTEXT_SIZE;
                    }
                }
            }

            // 屏蔽当前信号和 sa_mask
            t_inner.blocked_signals.add(signal);
            t_inner.blocked_signals |= action.sa_mask;

            let new_blocked = t_inner.blocked_signals;
            if is_task_level {
                t_inner.need_signal_handle =
                    t_inner.pending_signals.available_bits(new_blocked) != 0;
            } else {
                p_inner.need_signal_handle =
                    p_inner.pending_signals.available_bits(new_blocked) != 0;
            }
            drop(t_inner);
            drop(p_inner);

            info!(
                "handle_signals: current_tid={}, deliver signal {} to handler {:#x}, restorer {:#x}",
                task_tid,
                signal.as_i32(),
                action.sa_handler.as_ptr() as usize,
                action.sa_restorer
            );
        }
    }
}

const SA_SIGINFO: u32 = 0x00000004;

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
    let mut inner = process.inner_exclusive_access();
    let token = inner.get_user_token();

    // 保存旧值
    if old_value != 0 {
        let old = translated_refmut(token, old_value as *mut Itimerval);
        // 简化：返回0，不计算剩余时间
        old.it_interval = super::time::TimeVal { sec: 0, usec: 0 };
        old.it_value = super::time::TimeVal { sec: 0, usec: 0 };
    }

    if new_value != 0 {
        let new = translated_ref(token, new_value as *const Itimerval);
        let value_usec = new
            .it_value
            .sec
            .max(0)
            .saturating_mul(1_000_000)
            .saturating_add(new.it_value.usec.max(0));
        if value_usec > 0 {
            let ticks =
                (value_usec as usize).saturating_mul(crate::config::_CLOCK_FREQ) / 1_000_000;
            let deadline = crate::timer::get_time().saturating_add(ticks);
            inner.itimer_real_deadline = Some(deadline);
            // P0: 加入 timer 进程列表，避免中断遍历所有进程
            crate::task::manager::TIMER_PROCS
                .lock()
                .insert(process.getpid(), Arc::downgrade(&process));
        } else {
            inner.itimer_real_deadline = None;
        }

        let interval_usec = new
            .it_interval
            .sec
            .max(0)
            .saturating_mul(1_000_000)
            .saturating_add(new.it_interval.usec.max(0));
        if interval_usec > 0 {
            let interval_ticks =
                (interval_usec as usize).saturating_mul(crate::config::_CLOCK_FREQ) / 1_000_000;
            inner.itimer_real_interval = Some(interval_ticks);
        } else {
            inner.itimer_real_interval = None;
        }
    } else {
        inner.itimer_real_deadline = None;
        inner.itimer_real_interval = None;
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
    let inner = process.inner_exclusive_access();

    let remaining_ticks = if let Some(deadline) = inner.itimer_real_deadline {
        let now_ticks = crate::timer::get_time();
        if deadline > now_ticks {
            deadline - now_ticks
        } else {
            0
        }
    } else {
        0
    };
    let remaining_us =
        (remaining_ticks as u128).saturating_mul(1_000_000) / (crate::config::_CLOCK_FREQ as u128);

    let interval_us = (inner.itimer_real_interval.unwrap_or(0) as u128 * 1_000_000)
        / crate::config::_CLOCK_FREQ as u128;
    *translated_refmut(token, curr_value) = Itimerval {
        it_interval: TimeVal {
            sec: (interval_us / 1_000_000).min(i64::MAX as u128) as i64,
            usec: (interval_us % 1_000_000).min(i64::MAX as u128) as i64,
        },
        it_value: TimeVal {
            sec: (remaining_us / 1_000_000).min(i64::MAX as u128) as i64,
            usec: (remaining_us % 1_000_000).min(i64::MAX as u128) as i64,
        },
    };

    Ok(0)
}

/// ========== 8. sys_sigaltstack ==========
/// 设置/获取备用信号栈（当前为桩实现）
pub fn sys_sigaltstack(_ss: usize, _old_ss: usize) -> SyscallResult {
    Ok(0)
}
