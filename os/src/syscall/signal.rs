// src/signal/syscall.rs
use crate::mm::{translated_ref, translated_refmut};
use crate::task::signal::*;
use crate::task::*;
#[cfg(target_arch = "riscv64")]
use crate::timer::get_time_us;
use crate::trap::_set_sum_bit;
use alloc::sync::Arc;
use log::{error, info};
use polyhal::timer::current_time;

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
        restorer: 0,
        mask: action.sa_mask.bits(),
    }
}

fn linux_to_kernel_sigaction(action: LinuxRtSigAction) -> SigAction {
    SigAction {
        sa_handler: unsafe { SigHandler::from_ptr(action.handler as *const core::ffi::c_void) },
        sa_mask: SignalSet::from_bits(action.mask),
        sa_flags: action.flags as u32,
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
pub fn sys_sigaction(signum: usize, act: usize, oldact: usize, _sigsetsize: usize) -> isize {
    const EINVAL: isize = -22;
    const EFAULT: isize = -14;
    _set_sum_bit();
    info!(
        "sys_sigaction: signum={}, act={:#x}, oldact={:#x}",
        signum, act, oldact
    );
    let process = current_process();
    // 检查信号编号
    let signal = match Signal::from_i32(signum as i32) {
        Some(s) => s,
        None => return EINVAL,
    };

    if !signal.can_catch() && act != 0 {
        return EINVAL;
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
                return EINVAL;
            }
            if new_action.is_ignored() {
                inner.pending_signals.remove(signal);
            }
        }
    }

    if let Some(old) = old_action {
        if oldact == 0 {
            return EFAULT;
        }
        *translated_refmut(token, oldact as *mut LinuxRtSigAction) = kernel_to_linux_sigaction(old);
    }

    0
}

/// ========== 2. sys_kill ==========
/// 向进程发送信号
pub fn sys_kill(pid: isize, sig: usize) -> isize {
    const EINVAL: isize = -22;
    const ESRCH: isize = -3;

    _set_sum_bit();
    info!("sys_kill: pid={}, sig={}", pid, sig);
    let current = current_process();

    // 检查信号编号
    if sig >= 64 {
        return EINVAL;
    }

    // 查找目标进程
    let target = {
        if pid > 0 {
            match pid2process(pid as usize) {
                Some(t) => t,
                None => return ESRCH,
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
        None => return EINVAL,
    };

    // 投递信号
    deliver_signal(&target, signal)
}

/// tgkill: send a signal to a specific thread in a thread group.
/// Since Kairix handles signals at process granularity, we verify that
/// the given tid exists inside the target process and then deliver.
pub fn sys_tgkill(tgid: isize, tid: isize, sig: usize) -> isize {
    _set_sum_bit();
    info!("sys_tgkill: tgid={}, tid={}, sig={}", tgid, tid, sig);

    const EINVAL: isize = -22;
    const ESRCH: isize = -3;

    if tid <= 0 || tgid <= 0 {
        return EINVAL;
    }
    if sig >= 64 {
        return EINVAL;
    }

    let target_proc = match pid2process(tgid as usize) {
        Some(p) => p,
        None => return ESRCH,
    };

    // Verify the tid belongs to this process
    let inner = target_proc.inner_exclusive_access();
    let tid_exists = (tid as usize) < inner.tasks.len() && inner.tasks[tid as usize].is_some();
    drop(inner);

    if !tid_exists {
        return ESRCH;
    }

    if sig == 0 {
        return 0;
    }

    let signal = match Signal::from_i32(sig as i32) {
        Some(s) => s,
        None => return EINVAL,
    };

    deliver_signal(&target_proc, signal)
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
    const EINVAL: isize = -22;
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
                _ => return EINVAL,
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

    0
}

/// ========== 4. sys_rt_sigtimedwait (137) ==========
/// 从给定信号集中取一个待处理信号，可选超时。
/// 返回值：成功返回信号编号；失败返回负 errno。
pub fn sys_rt_sigtimedwait(set: usize, info: usize, timeout: usize, _sigsetsize: usize) -> isize {
    const EINVAL: isize = -22;
    const EAGAIN: isize = -11;

    _set_sum_bit();
    if set == 0 {
        return EINVAL;
    }

    let token = current_user_token();
    let wait_set = SignalSet::from_bits(*translated_ref(token, set as *const u64));

    let deadline_us = if timeout != 0 {
        let ts = *translated_ref(token, timeout as *const LinuxTimeSpec);
        if ts.tv_sec < 0 || ts.tv_nsec < 0 || ts.tv_nsec >= 1_000_000_000 {
            return EINVAL;
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
                return sig.as_i32() as isize;
            }
        }
        drop(inner);

        if let Some(deadline) = deadline_us {
            if (current_time().as_micros() as i128) >= deadline {
                return EAGAIN;
            }
        }
        crate::syscall::process::sys_yield();
    }
}
