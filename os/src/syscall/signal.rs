// src/signal/syscall.rs
use crate::error::{SysError, SyscallResult};
use crate::mm::{translated_ref, translated_refmut};
use crate::task::signal::*;
use crate::task::*;
#[cfg(target_arch = "riscv64")]
use crate::timer::get_time_us;
use crate::trap::_set_sum_bit;
use alloc::sync::Arc;
use log::{error, info};
use polyhal::timer::current_time;
use crate::syscall::time::TimeVal;

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
pub fn sys_sigaction(signum: usize, act: usize, oldact: usize, _sigsetsize: usize) -> SyscallResult {
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
            }
        }
    }

    if let Some(old) = old_action {
        if oldact == 0 {
            return Err(SysError::EFAULT);
        }
        *translated_refmut(token, oldact as *mut LinuxRtSigAction) = kernel_to_linux_sigaction(old);
    }

    Ok(0)
}

/// ========== 2. sys_kill ==========
/// 向进程发送信号
pub fn sys_kill(pid: isize, sig: usize) -> SyscallResult {
    _set_sum_bit();
    info!("sys_kill: pid={}, sig={}", pid, sig);
    let current = current_process();

    // 检查信号编号
    if sig >= 64 {
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
pub fn sys_tgkill(tgid: isize, tid: isize, sig: usize) -> SyscallResult {
    _set_sum_bit();
    info!("sys_tgkill: tgid={}, tid={}, sig={}", tgid, tid, sig);

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

    deliver_signal(&target_proc, signal);
    Ok(0)
}

/// 唤醒目标进程中第一个处于 Blocked 状态的任务
fn wakeup_first_blocked_task(proc: &Arc<ProcessControlBlock>) {
    let inner = proc.inner_exclusive_access();
    for task_opt in inner.tasks.iter() {
        if let Some(task) = task_opt {
            let t_inner = task.inner_exclusive_access();
            if t_inner.task_status == crate::task::TaskStatus::Blocked {
                drop(t_inner);
                crate::task::wakeup_task(task.clone());
                break;
            }
        }
    }
}

/// 投递信号到进程
pub fn deliver_signal(proc: &Arc<ProcessControlBlock>, signal: Signal) -> isize {
    let mut inner = proc.inner_exclusive_access();
    // 特殊处理：SIGKILL 和 SIGSTOP 不能被阻塞
    match signal {
        Signal::SigKill => {
            inner.is_zombie = true;
            inner.exit_code = 128 + signal.as_i32();
            for task_opt in inner.tasks.iter() {
                if let Some(task) = task_opt {
                    remove_inactive_task(Arc::clone(task));
                }
            }
            drop(inner);
            wakeup_first_blocked_task(proc);
            return 0;
        }
        Signal::SigStop => {
            inner.state = crate::task::process::ProcessStatus::Terminal;
            drop(inner);
            wakeup_first_blocked_task(proc);
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
            if let SignalAction::Terminate | SignalAction::Core = signal.default_action() {
                inner.exit_code = 128 + signal.as_i32();
                for task_opt in inner.tasks.iter() {
                    if let Some(task) = task_opt {
                        remove_inactive_task(Arc::clone(task));
                    }
                }
            }
            drop(inner);
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
    let process = current_process();
    let token = current_user_token();

    // 先读用户输入，避免持锁访问用户地址触发缺页死锁。
    let new_set = if set != 0 {
        Some(SignalSet::from_bits(*translated_ref(
            token,
            set as *const u64,
        )))
    } else {
        None
    };

    let mut old_mask = None;
    {
        let mut inner = process.inner_exclusive_access();

        // 返回旧的阻塞掩码
        if oldset != 0 {
            old_mask = Some(inner.blocked_signals.bits());
        }

        // 设置新的阻塞掩码
        if let Some(new_set) = new_set {
            match how {
                0 => {
                    // SIG_BLOCK
                    let bits = inner.blocked_signals.bits() | new_set.bits();
                    inner.blocked_signals = SignalSet::from_bits(bits);
                }
                1 => {
                    // SIG_UNBLOCK
                    let bits = inner.blocked_signals.bits() & !new_set.bits();
                    inner.blocked_signals = SignalSet::from_bits(bits);
                }
                2 => {
                    // SIG_SETMASK
                    inner.blocked_signals = new_set;
                }
                _ => return Err(SysError::EINVAL),
            }

            // 解除阻塞后，检查是否有待处理的信号
            if how == 1 || how == 2 {
                let ready = inner.pending_signals.bits() & !inner.blocked_signals.bits();
                if ready != 0 {
                    inner.need_signal_handle = true;
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
pub fn sys_rt_sigtimedwait(set: usize, info: usize, timeout: usize, _sigsetsize: usize) -> SyscallResult {
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
        let mut inner = process.inner_exclusive_access();
        let matched = inner.pending_signals.bits() & wait_set.bits();
        if matched != 0 {
            let idx = matched.trailing_zeros() as usize;
            if let Some(sig) = Signal::from_i32((idx + 1) as i32) {
                inner.pending_signals.remove(sig);
                inner.need_signal_handle =
                    (inner.pending_signals.bits() & !inner.blocked_signals.bits()) != 0;
                drop(inner);

                if info != 0 {
                    *translated_refmut(token, info as *mut LinuxSigInfo) =
                        LinuxSigInfo::new(sig.as_i32());
                }
                return Ok(sig.as_i32() as usize);
            }
        }
        drop(inner);

        if let Some(deadline) = deadline_us {
            if (current_time().as_micros() as i128) >= deadline {
                return Err(SysError::EAGAIN);
            }
        }
        block_current_and_run_next();
    }
}

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
        let saved_tf = trap_cx.clone();
        let saved_mask = inner.blocked_signals;
        inner.sig_context_stack.push((saved_tf, saved_mask));

        trap_cx[polyhal_trap::trapframe::TrapFrameArgs::SEPC] = handler as usize;
        trap_cx[polyhal_trap::trapframe::TrapFrameArgs::ARG0] = signo as usize;
        if action.sa_restorer != 0 {
            trap_cx[polyhal_trap::trapframe::TrapFrameArgs::RA] = action.sa_restorer;
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

/// ========== 6. sys_rt_sigreturn (139) ==========
/// 从信号 handler 恢复用户态上下文。
/// 从 PCB 的 sig_context_stack 弹出保存的 TrapFrame 和信号掩码。
pub fn sys_rt_sigreturn() -> SyscallResult {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    if let Some((saved_tf, saved_mask)) = inner.sig_context_stack.pop() {
        inner.blocked_signals = saved_mask;
        inner.need_signal_handle =
            (inner.pending_signals.bits() & !saved_mask.bits()) != 0;
        drop(inner);
        let trap_cx = current_trap_cx();
        let ret = saved_tf[polyhal_trap::trapframe::TrapFrameArgs::RET];
        *trap_cx = saved_tf;
        Ok(ret)
    } else {
        Err(SysError::EINVAL)
    }
}

/// ========== 7. setitimer / getitimer ==========
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Itimerval {
    /// 间隔时间
    pub it_interval: TimeVal,
    /// 剩余到期时间
    pub it_value: TimeVal,
}

/// 设置间隔定时器（目前仅支持 ITIMER_REAL）
pub fn sys_setitimer(which: usize, new_value: *const Itimerval, old_value: *mut Itimerval) -> SyscallResult {
    info!("sys_setitimer: which={}, new_value={:?}, old_value={:?}", which, new_value, old_value);
    const ITIMER_REAL: usize = 0;

    if which != ITIMER_REAL {
        return Err(SysError::EINVAL);
    }

    let process = current_process();
    let token = current_user_token();

    let mut inner = process.inner_exclusive_access();

    if old_value != 0 as *mut Itimerval {
        let old = Itimerval {
            it_interval: TimeVal {
                sec: (inner.alarm_interval_us.unwrap_or(0) / 1_000_000) as i64,
                usec: (inner.alarm_interval_us.unwrap_or(0) % 1_000_000) as i64,
            },
            it_value: TimeVal {
                sec: if let Some(deadline) = inner.alarm_deadline_us {
                    let remaining = deadline.saturating_sub(current_time().as_micros() as u128);
                    (remaining / 1_000_000) as i64
                } else {
                    0
                },
                usec: if let Some(deadline) = inner.alarm_deadline_us {
                    let remaining = deadline.saturating_sub(current_time().as_micros() as u128);
                    (remaining % 1_000_000) as i64
                } else {
                    0
                },
            },
        };
        *translated_refmut(token, old_value) = old;
    }

    if new_value != 0 as *const Itimerval {
        let new_val = *translated_ref(token, new_value);
        let interval_us = (new_val.it_interval.sec as u128)
            .saturating_mul(1_000_000)
            .saturating_add(new_val.it_interval.usec as u128);
        let value_us = (new_val.it_value.sec as u128)
            .saturating_mul(1_000_000)
            .saturating_add(new_val.it_value.usec as u128);

        inner.alarm_interval_us = if interval_us > 0 { Some(interval_us) } else { None };
        inner.alarm_deadline_us = if value_us > 0 {
            Some(current_time().as_micros() as u128 + value_us)
        } else {
            None
        };
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
    let inner = process.inner_exclusive_access();
    let token = current_user_token();

    let remaining_us = if let Some(deadline) = inner.alarm_deadline_us {
        deadline.saturating_sub(current_time().as_micros() as u128)
    } else {
        0
    };

    *translated_refmut(token, curr_value) = Itimerval {
        it_interval: TimeVal {
            sec: (inner.alarm_interval_us.unwrap_or(0) / 1_000_000) as i64,
            usec: (inner.alarm_interval_us.unwrap_or(0) % 1_000_000) as i64,
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
