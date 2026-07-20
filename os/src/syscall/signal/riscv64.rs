// src/signal/syscall.rs
use super::common::finish_signaled_process;
use crate::error::{SysError, SyscallResult};
use crate::mm::{
    translated_byte_buffer, translated_byte_buffer_for_write, translated_ref, translated_refmut,
};
use crate::task::signal::*;
use crate::task::*;
#[cfg(target_arch = "riscv64")]
use crate::timer::get_time_us;
use crate::trap::_set_sum_bit;
use log::{debug, error, info, trace};
use polyhal_trap::trapframe::TrapFrameArgs;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct LinuxRtSigAction {
    handler: usize,
    flags: usize,
    mask: usize,
}

/// Preserve the supervisor-generated status baseline while forcing the state
/// required by `sret` to enter user mode with interrupts enabled. Privileged
/// bits from the user signal frame must never be copied into sstatus.
fn sanitized_user_sstatus(kernel_bits: usize) -> usize {
    const SIE: usize = 1 << 1;
    const SPIE: usize = 1 << 5;
    const SPP: usize = 1 << 8;
    const SUM: usize = 1 << 18;
    const MXR: usize = 1 << 19;

    (kernel_bits & !(SIE | SPIE | SPP | SUM | MXR)) | SPIE
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

/// ========== 1. sys_sigaction ==========
/// 设置或查询信号处理函数
pub fn sys_sigaction(
    signum: usize,
    act: usize,
    oldact: usize,
    _sigsetsize: usize,
) -> SyscallResult {
    _set_sum_bit();
    debug!("PRINTLN sys_sigaction: signum={}", signum);
    debug!(
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
                debug!("[DEBUG sigaction] new handler = DEFAULT")
            }
            crate::task::signal::SigHandler::Ignore => {
                debug!("[DEBUG sigaction] new handler = IGNORE")
            }
            crate::task::signal::SigHandler::Custom(addr) => {
                debug!("[DEBUG sigaction] new handler = CUSTOM {:p}", addr)
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
        let original_f = trap_cx.f;
        let original_fcsr = trap_cx.fcsr;
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
        for (index, value) in original_f.iter().enumerate() {
            let offset = mcontext_base + 264 + index * 8;
            frame[offset..offset + 8].copy_from_slice(&value.to_ne_bytes());
        }
        frame[mcontext_base + 520..mcontext_base + 528]
            .copy_from_slice(&original_fcsr.to_ne_bytes());

        let bufs = match translated_byte_buffer_for_write(token, new_sp as *mut u8, SIGFRAME_SIZE) {
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
                crate::config::USER_RT_SIGRETURN_TRAMPOLINE;
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
/// 对于 SA_SIGINFO 帧，从用户栈的 ucontext 读取可能修改过的寄存器和掩码；
/// 对于非 SA_SIGINFO 帧，从 PCB 的 sig_context_stack 弹出保存的 TrapFrame。
pub fn sys_rt_sigreturn() -> SyscallResult {
    const SIGINFO_SIZE: usize = 128;
    #[allow(dead_code)]
    const UCONTEXT_SIZE: usize = 960;
    #[allow(dead_code)]
    const SIGFRAME_SIZE: usize = SIGINFO_SIZE + UCONTEXT_SIZE + 8; // keep frame layout aligned

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

    // 从用户栈读取 __gregs[0..32]、sstatus、f[0..32] 和 fcsr。
    let mcontext_addr = current_sp + SIGINFO_SIZE + 176;
    const MCONTEXT_SIZE: usize = 528;
    let bufs = translated_byte_buffer(token, mcontext_addr as *const u8, MCONTEXT_SIZE)?;
    let mut mcontext_bytes = [0u8; MCONTEXT_SIZE];
    let mut copied = 0;
    for buf in bufs {
        let len = buf.len().min(MCONTEXT_SIZE - copied);
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
    let mut fp_regs = [0u64; 32];
    for (index, value) in fp_regs.iter_mut().enumerate() {
        let offset = 264 + index * 8;
        *value = u64::from_ne_bytes(mcontext_bytes[offset..offset + 8].try_into().unwrap());
    }
    let fcsr = usize::from_ne_bytes(mcontext_bytes[520..528].try_into().unwrap());

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
    // Floating-point and general registers come from ucontext, but privileged
    // return state does not. In particular, SPIE=0 would let a signal frame
    // permanently disable this CPU's timer interrupts after sret.
    let kernel_sstatus_bits = unsafe { core::mem::transmute_copy(&trap_cx.sstatus) };
    let sanitized_sstatus_bits = sanitized_user_sstatus(kernel_sstatus_bits);
    const USER_RETURN_CRITICAL_MASK: usize = (1 << 1) | (1 << 5) | (1 << 8) | (1 << 18) | (1 << 19);
    if sstatus_bits & USER_RETURN_CRITICAL_MASK != 1 << 5 {
        log::error!(
            "[SIGRETURN_STATUS_SANITIZED] arch=riscv64 pid={} raw={:#x} sanitized={:#x}",
            task.process_id(),
            sstatus_bits,
            sanitized_sstatus_bits,
        );
    }
    trap_cx.sstatus = unsafe { core::mem::transmute(sanitized_sstatus_bits) };
    trap_cx.f = fp_regs;
    trap_cx.fcsr = fcsr;

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

    if !task_needs_signal
        && !proc_needs_signal
        && ((task_pending.bits() | proc_pending.bits()) & !task_blocked.bits()) == 0
    {
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
        t_inner.need_signal_handle =
            (t_inner.pending_signals.bits() & !t_inner.blocked_signals.bits()) != 0;
        drop(t_inner);
        let mut p_inner = process.inner_exclusive_access();
        if p_inner.pending_signals.is_empty() {
            p_inner.need_signal_handle = false;
        }
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
    if !matches!(target_action.sa_handler, crate::task::signal::SigHandler::Ignore) {
        if let Err(error) = crate::syscall::rseq::signal_deliver(ctx) {
            crate::syscall::rseq::force_sigsegv(ctx, error, true);
            return;
        }
    }
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
            let original_f = ctx.f;
            let original_fcsr = ctx.fcsr;
            let original_x: [usize; 32] = ctx.x;
            let saved_mask = task_blocked;

            // 统一在用户栈构建信号帧（无论是否 SA_SIGINFO）
            const SIGINFO_SIZE: usize = 128;
            const UCONTEXT_SIZE: usize = 960;
            const SIGFRAME_SIZE: usize = SIGINFO_SIZE + UCONTEXT_SIZE + 8;

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
            // 扩展：保存 sstatus 和完整浮点状态（紧跟在 __gregs 之后）。
            frame[mcontext_base + 256..mcontext_base + 264]
                .copy_from_slice(&original_sstatus.bits().to_ne_bytes());
            for (index, value) in original_f.iter().enumerate() {
                let offset = mcontext_base + 264 + index * 8;
                frame[offset..offset + 8].copy_from_slice(&value.to_ne_bytes());
            }
            frame[mcontext_base + 520..mcontext_base + 528]
                .copy_from_slice(&original_fcsr.to_ne_bytes());

            // Write to user stack
            let bufs =
                match translated_byte_buffer_for_write(token, new_sp as *mut u8, SIGFRAME_SIZE) {
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
                ctx[TrapFrameArgs::RA] = crate::config::USER_RT_SIGRETURN_TRAMPOLINE;
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
