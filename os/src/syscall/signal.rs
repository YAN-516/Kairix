// src/signal/syscall.rs
use crate::task::signal::*;
use crate::task::*;
use crate::trap::_set_sum_bit;
use alloc::sync::Arc;
use log::{error, info};

/// ========== 1. sys_sigaction ==========
/// 设置或查询信号处理函数
pub fn sys_sigaction(signum: usize, act: usize, oldact: usize, _sigsetsize: usize) -> isize {
    _set_sum_bit();
    info!(
        "sys_sigaction: signum={}, act={:#x}, oldact={:#x}",
        signum, act, oldact
    );
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    // 检查信号编号
    let signal = match Signal::from_i32(signum as i32) {
        Some(s) => s,
        None => return -1, // EINVAL
    };

    // 返回旧的信号处理动作
    if oldact != 0 {
        unsafe {
            *(oldact as *mut SigAction) = inner.signals_handler.get(signal);
        }
    }

    // 设置新的信号处理动作
    if act != 0 {
        let new_act = act as *const SigAction;
        // SIGKILL 和 SIGSTOP 不能被改变
        if !signal.can_catch() {
            return -1; // EINVAL
        }

        // 设置新动作
        if let Err(_) = inner.signals_handler.set(signal, new_act) {
            return -1;
        }

        // 如果设置为忽略，清除未决信号
        unsafe {
            if (*new_act).is_ignored() {
                inner.pending_signals.remove(signal);
            }
        }
    }

    0
}

/// ========== 2. sys_kill ==========
/// 向进程发送信号
pub fn sys_kill(pid: isize, sig: usize) -> isize {
    _set_sum_bit();
    info!("sys_kill: pid={}, sig={}", pid, sig);
    let current = current_process();

    // 检查信号编号
    if sig >= 64 {
        return -1; // EINVAL
    }

    // 查找目标进程
    let target = {
        if pid > 0 {
            match pid2process(pid as usize) {
                Some(t) => t,
                None => return -1, // ESRCH
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
        return 0;
    }

    // 转换信号
    let signal = match Signal::from_i32(sig as i32) {
        Some(s) => s,
        None => return -1,
    };

    // 投递信号
    deliver_signal(&target, signal)
}

/// 投递信号到进程
fn deliver_signal(proc: &Arc<ProcessControlBlock>, signal: Signal) -> isize {
    let mut inner = proc.inner_exclusive_access();
    // 特殊处理：SIGKILL 和 SIGSTOP 不能被阻塞
    match signal {
        Signal::SigKill => {
            inner.is_zombie = true;
            return 0;
        }
        Signal::SigStop => {
            inner.state = crate::task::process::ProcessStatus::Terminal;
            return 0;
        }
        _ => {}
    }

    // 检查是否被阻塞
    if inner.blocked_signals.contains(signal) {
        inner.pending_signals.add(signal);
        inner.need_signal_handle = true;
        return 0;
    }

    // 获取处理动作
    let action = inner.signals_handler.get(signal);

    match action.sa_handler {
        SigHandler::Ignore => {
            // 忽略
            0
        }
        SigHandler::Default => {
            // 默认处理
            inner.handle_default_action(signal);
            0
        }
        SigHandler::Custom(_) => {
            // 用户自定义，标记为需要处理
            inner.pending_signals.add(signal);
            inner.need_signal_handle = true;
            0
        }
    }
}

/// ========== 3. sys_sigprocmask ==========
/// 检查或更改阻塞信号掩码
pub fn sys_sigprocmask(how: usize, set: usize, oldset: usize, _sigsetsize: usize) -> isize {
    _set_sum_bit();
    info!(
        "sys_sigprocmask: how={}, set={:#x}, oldset={:#x}",
        how, set, oldset
    );
    let process = current_process();
    let mut inner = process.inner_exclusive_access();

    // 返回旧的阻塞掩码
    if oldset != 0 {
        unsafe {
            *(oldset as *mut SignalSet) = inner.blocked_signals;
        }
    }

    // 设置新的阻塞掩码
    if set != 0 {
        let new_set = unsafe { *(set as *const SignalSet) };
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
            _ => return -1, // EINVAL
        }

        // 解除阻塞后，检查是否有待处理的信号
        if how == 1 || how == 2 {
            let ready = inner.pending_signals.bits() & !inner.blocked_signals.bits();
            if ready != 0 {
                inner.need_signal_handle = true;
            }
        }
    }

    0
}
