// src/signal/syscall.rs
use super::common::{
    LinuxStack, commit_signal_stack, consume_pending_signal, discard_pending_signal,
    finish_signaled_process, handle_signal_frame_failure, prepare_signal_stack,
    restore_signal_alt_stack, restore_wait_mask_without_signal_frame, stop_process,
    write_alt_stack_to_ucontext, write_linux_siginfo,
};
use crate::error::{SysError, SyscallResult};
use crate::mm::{translated_byte_buffer_for_write, translated_ref, write_user_value};
use crate::task::signal::*;
use crate::task::*;
#[cfg(target_arch = "riscv64")]
use crate::timer::get_time_us;
use crate::trap::_set_sum_bit;
use log::{debug, error, info, trace};
use polyhal_trap::trapframe::TrapFrameArgs;

// Linux LoongArch rt_sigframe layout.  In particular, sc_pc is separate from
// sc_regs[32], and floating-point/vector state is described by an extensible
// sctx_info record.  User programs such as QEMU inspect this layout directly
// from their SA_SIGINFO handler, so a private kernel-only layout is not ABI
// compatible even when our own rt_sigreturn can read it back symmetrically.
const SIGINFO_SIZE: usize = 128;
const UCONTEXT_MCONTEXT_OFFSET: usize = 176;
const SIGCONTEXT_REGS_OFFSET: usize = 8;
const SIGCONTEXT_FLAGS_OFFSET: usize = 264;
const SIGCONTEXT_EXTCONTEXT_OFFSET: usize = 272;
const SC_USED_FP: u32 = 1;

const SCTX_INFO_SIZE: usize = 16;
const LSX_CTX_MAGIC: u32 = 0x5358_0001;
const LSX_CONTEXT_REGS_SIZE: usize = 32 * 16;
const LSX_CONTEXT_SIZE: usize = 528;
const LSX_SCTX_SIZE: usize = SCTX_INFO_SIZE + LSX_CONTEXT_SIZE;
const LSX_CONTEXT_OFFSET: usize = SIGCONTEXT_EXTCONTEXT_OFFSET + SCTX_INFO_SIZE;
const LSX_CONTEXT_FCC_OFFSET: usize = LSX_CONTEXT_OFFSET + LSX_CONTEXT_REGS_SIZE;
const LSX_CONTEXT_FCSR_OFFSET: usize = LSX_CONTEXT_FCC_OFFSET + 8;
const END_SCTX_OFFSET: usize = SIGCONTEXT_EXTCONTEXT_OFFSET + LSX_SCTX_SIZE;
const LOONGARCH_MCONTEXT_SIZE: usize = END_SCTX_OFFSET + SCTX_INFO_SIZE;
const LOONGARCH_SIGFRAME_SIZE: usize =
    SIGINFO_SIZE + UCONTEXT_MCONTEXT_OFFSET + LOONGARCH_MCONTEXT_SIZE;

const _: () = {
    assert!(SIGCONTEXT_EXTCONTEXT_OFFSET % 16 == 0);
    assert!(LSX_CONTEXT_OFFSET % 16 == 0);
    assert!(LSX_CONTEXT_SIZE == LSX_CONTEXT_REGS_SIZE + 8 + 4 + 4);
    assert!(LSX_SCTX_SIZE % 16 == 0);
    assert!(END_SCTX_OFFSET == 816);
    assert!(LOONGARCH_MCONTEXT_SIZE == 832);
    assert!(LOONGARCH_SIGFRAME_SIZE == 1136);
    assert!(LOONGARCH_SIGFRAME_SIZE % 16 == 0);
};

fn write_linux_mcontext(
    frame: &mut [u8],
    pc: usize,
    regs: &[usize; 32],
    vectors: &[[u64; 2]; 32],
    fcc: &[u8; 8],
    fcsr: usize,
) {
    let base = SIGINFO_SIZE + UCONTEXT_MCONTEXT_OFFSET;
    frame[base..base + 8].copy_from_slice(&pc.to_ne_bytes());
    for (index, value) in regs.iter().enumerate() {
        let value = if index == 0 { 0 } else { *value };
        let offset = base + SIGCONTEXT_REGS_OFFSET + index * 8;
        frame[offset..offset + 8].copy_from_slice(&value.to_ne_bytes());
    }
    frame[base + SIGCONTEXT_FLAGS_OFFSET..base + SIGCONTEXT_FLAGS_OFFSET + 4]
        .copy_from_slice(&SC_USED_FP.to_ne_bytes());

    let info = base + SIGCONTEXT_EXTCONTEXT_OFFSET;
    frame[info..info + 4].copy_from_slice(&LSX_CTX_MAGIC.to_ne_bytes());
    frame[info + 4..info + 8].copy_from_slice(&(LSX_SCTX_SIZE as u32).to_ne_bytes());

    for (index, vector) in vectors.iter().enumerate() {
        let offset = base + LSX_CONTEXT_OFFSET + index * 16;
        frame[offset..offset + 8].copy_from_slice(&vector[0].to_ne_bytes());
        frame[offset + 8..offset + 16].copy_from_slice(&vector[1].to_ne_bytes());
    }
    frame[base + LSX_CONTEXT_FCC_OFFSET..base + LSX_CONTEXT_FCC_OFFSET + 8].copy_from_slice(fcc);
    frame[base + LSX_CONTEXT_FCSR_OFFSET..base + LSX_CONTEXT_FCSR_OFFSET + 4]
        .copy_from_slice(&(fcsr as u32).to_ne_bytes());
    // The zero-filled sctx_info at END_SCTX_OFFSET terminates Linux's parser.
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct LinuxRtSigAction {
    handler: usize,
    flags: usize,
    mask: usize,
}

/// Build the only PRMD state that is valid for returning to a normal user
/// task. PRMD is privileged CPU state, not a user-controlled register: PPLV
/// must be 3 and PIE must be set so `ertn` re-enables timer interrupts.
fn sanitized_user_prmd() -> usize {
    const PPLV3: usize = 0b11;
    const PIE: usize = 1 << 2;
    PPLV3 | PIE
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

    let mut old_action = None;
    let mut clear_task_pending = false;
    {
        let mut inner = process.inner_exclusive_access();

        // 返回旧的信号处理动作
        if oldact != 0 {
            old_action = Some(inner.signals_handler.lock().get(signal));
        }

        // 设置新的信号处理动作
        if let Some(new_action) = new_action {
            if inner
                .signals_handler
                .lock()
                .set(signal, &new_action as *const SigAction)
                .is_err()
            {
                return Err(SysError::EINVAL);
            }
            if new_action.is_ignored() {
                let inner = &mut *inner;
                discard_pending_signal(
                    &mut inner.pending_signals,
                    &mut inner.pending_signal_queue,
                    signal,
                );
                clear_task_pending = true;
            }
        }
    }
    if clear_task_pending {
        // Do this after dropping process.inner to avoid process -> task lock order.
        let task = current_task().unwrap();
        let mut task_inner = task.inner_exclusive_access();
        let task_inner = &mut *task_inner;
        discard_pending_signal(
            &mut task_inner.pending_signals,
            &mut task_inner.pending_signal_queue,
            signal,
        );
    }

    if let Some(old) = old_action {
        if oldact != 0 {
            write_user_value(
                token,
                oldact as *mut LinuxRtSigAction,
                &kernel_to_linux_sigaction(old),
            )?;
            if oldact == 0 {
                return Err(SysError::EFAULT);
            }
        }
    }
    return Ok(0);
}

///
pub fn handle_pending_signals() {
    error!("handle_pending_signals: checking pending signals for process");
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
    let action = inner.signals_handler.lock().take_for_delivery(signal);
    if let SigHandler::Custom(handler) = action.sa_handler {
        let trap_cx = current_trap_cx();
        let original_sepc = trap_cx.pc();
        let _original_era = trap_cx.era;
        let original_regs: [usize; 32] = trap_cx.regs;
        let original_vectors = trap_cx.vr;
        let original_fcc = trap_cx.fcc;
        let original_fcsr = trap_cx.fcsr;
        let saved_mask = inner.blocked_signals;

        trap_cx.era = handler as usize;
        trap_cx[polyhal_trap::trapframe::TrapFrameArgs::ARG0] = signo as usize;
        // 统一在用户栈构建信号帧（Linux 风格，避免 longjmp 导致内核内存泄漏）
        let sp = trap_cx[polyhal_trap::trapframe::TrapFrameArgs::SP];
        let Some(frame_bottom) = sp.checked_sub(LOONGARCH_SIGFRAME_SIZE) else {
            return;
        };
        let new_sp = frame_bottom & !0xf;
        let token = process.user_token();

        let mut frame = [0u8; LOONGARCH_SIGFRAME_SIZE];
        frame[0..4].copy_from_slice(&signo.to_ne_bytes());

        let mask = saved_mask.bits();
        frame[SIGINFO_SIZE + 40..SIGINFO_SIZE + 48].copy_from_slice(&mask.to_ne_bytes());

        write_linux_mcontext(
            &mut frame,
            original_sepc,
            &original_regs,
            &original_vectors,
            &original_fcc,
            original_fcsr,
        );

        let bufs = match translated_byte_buffer_for_write(
            token,
            new_sp as *mut u8,
            LOONGARCH_SIGFRAME_SIZE,
        ) {
            Ok(bufs) => bufs,
            Err(_) => return,
        };
        let mut written = 0;
        for buf in bufs {
            let len = buf.len().min(LOONGARCH_SIGFRAME_SIZE - written);
            buf[..len].copy_from_slice(&frame[written..written + len]);
            written += len;
        }

        trap_cx[polyhal_trap::trapframe::TrapFrameArgs::SP] = new_sp;
        trap_cx[polyhal_trap::trapframe::TrapFrameArgs::ARG1] = new_sp;
        trap_cx[polyhal_trap::trapframe::TrapFrameArgs::ARG2] = new_sp + SIGINFO_SIZE;
        trap_cx[polyhal_trap::trapframe::TrapFrameArgs::RA] =
            crate::config::USER_RT_SIGRETURN_TRAMPOLINE;

        let mut new_mask = inner.blocked_signals.bits() | action.sa_mask.bits();
        if (action.sa_flags & 0x40000000) == 0 {
            // SA_NODEFER = 0x40000000
            new_mask |= 1 << (signo - 1);
        }
        inner.blocked_signals = SignalSet::from_bits(new_mask).without_unblockable();

        {
            let inner_ref = &mut *inner;
            consume_pending_signal(
                &mut inner_ref.pending_signals,
                &mut inner_ref.pending_signal_queue,
                signal,
            );
        }
        inner.need_signal_handle =
            (inner.pending_signals.bits() & !inner.blocked_signals.bits()) != 0;
    } else {
        // Default 或 Ignore：清除 pending
        {
            let inner_ref = &mut *inner;
            consume_pending_signal(
                &mut inner_ref.pending_signals,
                &mut inner_ref.pending_signal_queue,
                signal,
            );
        }
        inner.need_signal_handle =
            (inner.pending_signals.bits() & !inner.blocked_signals.bits()) != 0;
    }
}

///
pub fn sys_rt_sigreturn() -> SyscallResult {
    let task = current_task().unwrap();
    let token = current_user_token();
    let current_sp = current_trap_cx()[polyhal_trap::trapframe::TrapFrameArgs::SP];

    let alt_stack_addr = current_sp
        .checked_add(SIGINFO_SIZE + 16)
        .ok_or(SysError::EFAULT)?;
    let saved_alt_stack = *translated_ref(token, alt_stack_addr as *const LinuxStack)?;

    // 从用户栈读取 uc_sigmask
    let sigmask_addr = current_sp + SIGINFO_SIZE + 40;
    let bufs = crate::mm::translated_byte_buffer(token, sigmask_addr as *const u8, 16)?;
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

    // Read the Linux LoongArch sigcontext and its LSX extension.  The extension
    // header is part of the user ABI and must be validated before any restored
    // register state is committed.
    let mcontext_addr = current_sp + SIGINFO_SIZE + UCONTEXT_MCONTEXT_OFFSET;
    let bufs = crate::mm::translated_byte_buffer(
        token,
        mcontext_addr as *const u8,
        LOONGARCH_MCONTEXT_SIZE,
    )?;
    let mut mcontext_bytes = [0u8; LOONGARCH_MCONTEXT_SIZE];
    let mut copied = 0;
    for buf in bufs {
        let len = buf.len().min(LOONGARCH_MCONTEXT_SIZE - copied);
        mcontext_bytes[copied..copied + len].copy_from_slice(&buf[..len]);
        copied += len;
    }

    let restored_pc = usize::from_ne_bytes(mcontext_bytes[0..8].try_into().unwrap());
    let mut gregs = [0usize; 32];
    for (index, value) in gregs.iter_mut().enumerate() {
        let offset = SIGCONTEXT_REGS_OFFSET + index * 8;
        *value = usize::from_ne_bytes(mcontext_bytes[offset..offset + 8].try_into().unwrap());
    }
    let flags = u32::from_ne_bytes(
        mcontext_bytes[SIGCONTEXT_FLAGS_OFFSET..SIGCONTEXT_FLAGS_OFFSET + 4]
            .try_into()
            .unwrap(),
    );
    let info = SIGCONTEXT_EXTCONTEXT_OFFSET;
    let magic = u32::from_ne_bytes(mcontext_bytes[info..info + 4].try_into().unwrap());
    let size = u32::from_ne_bytes(mcontext_bytes[info + 4..info + 8].try_into().unwrap()) as usize;
    let end_magic = u32::from_ne_bytes(
        mcontext_bytes[END_SCTX_OFFSET..END_SCTX_OFFSET + 4]
            .try_into()
            .unwrap(),
    );
    let end_size = u32::from_ne_bytes(
        mcontext_bytes[END_SCTX_OFFSET + 4..END_SCTX_OFFSET + 8]
            .try_into()
            .unwrap(),
    );
    if flags & SC_USED_FP == 0
        || magic != LSX_CTX_MAGIC
        || size != LSX_SCTX_SIZE
        || end_magic != 0
        || end_size != 0
    {
        log::error!(
            "[SIGRETURN_EXTCONTEXT_INVALID] arch=loongarch64 pid={} flags={:#x} magic={:#x} size={} end_magic={:#x} end_size={}",
            task.process_id(),
            flags,
            magic,
            size,
            end_magic,
            end_size,
        );
        return Err(SysError::EINVAL);
    }

    let mut vectors = [[0u64; 2]; 32];
    for (index, vector) in vectors.iter_mut().enumerate() {
        let offset = LSX_CONTEXT_OFFSET + index * 16;
        vector[0] = u64::from_ne_bytes(mcontext_bytes[offset..offset + 8].try_into().unwrap());
        vector[1] = u64::from_ne_bytes(mcontext_bytes[offset + 8..offset + 16].try_into().unwrap());
    }
    let mut fcc = [0u8; 8];
    fcc.copy_from_slice(&mcontext_bytes[LSX_CONTEXT_FCC_OFFSET..LSX_CONTEXT_FCC_OFFSET + 8]);
    let fcsr = u32::from_ne_bytes(
        mcontext_bytes[LSX_CONTEXT_FCSR_OFFSET..LSX_CONTEXT_FCSR_OFFSET + 4]
            .try_into()
            .unwrap(),
    ) as usize;
    restore_signal_alt_stack(&task, saved_alt_stack)?;

    let mut t_inner = task.inner_exclusive_access();
    t_inner.blocked_signals = restored_mask.without_unblockable();
    t_inner.need_signal_handle = (t_inner.pending_signals.bits() & !restored_mask.bits()) != 0;
    t_inner.interrupted_by_signal = true;
    // Restore the mask that was active before an interrupted wait syscall.
    if let Some(old_mask) = t_inner.signal_wait_old_masks.pop() {
        t_inner.blocked_signals = old_mask.without_unblockable();
        t_inner.need_signal_handle = (t_inner.pending_signals.bits() & !old_mask.bits()) != 0;
    }
    drop(t_inner);

    let trap_cx = current_trap_cx();
    trap_cx.set_pc(restored_pc);
    // PRMD is privileged state and is intentionally absent from the Linux
    // sigcontext.  Always rebuild the only safe user-return value.
    trap_cx.prmd = sanitized_user_prmd();
    trap_cx.vr = vectors;
    trap_cx.fcc = fcc;
    trap_cx.fcsr = fcsr;
    for i in 1..32 {
        trap_cx.regs[i] = gregs[i];
    }
    trap_cx.regs[0] = 0;
    Ok(gregs[4])
}
/// 在 trap 返回用户态前投递 pending 信号
///
/// 找到第一个 pending 且未被阻塞的信号，根据 handler 类型处理：
/// - Ignore：直接清除
/// - Default：调用 handle_default_action，必要时标记进程退出
/// - Custom：保存 TrapFrame 到 sig_context_stack，修改 ctx 跳转到用户态 handler
pub fn handle_signals(ctx: &mut polyhal_trap::trapframe::TrapFrame) {
    // return;
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
    let mut token = 0usize;
    {
        let p_inner = process.inner_exclusive_access();
        for i in 1..=64 {
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
                target_action = p_inner.signals_handler.lock().take_for_delivery(signal);
                token = process.user_token();
                break;
            }
        }
    }

    let signal = match target_sig {
        Some(signal) => signal,
        None => return,
    };
    let last_siginfo = if is_task_level {
        task.inner_exclusive_access()
            .pending_signal_queue
            .iter()
            .find(|info| info.si_signo == signal.as_i32())
            .copied()
    } else {
        process
            .inner_exclusive_access()
            .pending_signal_queue
            .iter()
            .find(|info| info.si_signo == signal.as_i32())
            .copied()
    };

    let handler_addr = target_action.sa_handler.as_ptr() as usize;
    let restorer_addr = 0usize;
    let sa_mask = target_action.sa_mask;
    if !matches!(
        target_action.sa_handler,
        crate::task::signal::SigHandler::Ignore
    ) {
        if let Err(error) = crate::syscall::rseq::signal_deliver(ctx) {
            crate::syscall::rseq::force_sigsegv(ctx, error, true);
            return;
        }
    }
    match target_action.sa_handler {
        crate::task::signal::SigHandler::Ignore => {
            if is_task_level {
                let mut t_inner = task.inner_exclusive_access();
                let t_inner = &mut *t_inner;
                consume_pending_signal(
                    &mut t_inner.pending_signals,
                    &mut t_inner.pending_signal_queue,
                    signal,
                );
                t_inner.need_signal_handle =
                    (t_inner.pending_signals.bits() & !t_inner.blocked_signals.bits()) != 0;
            } else {
                let mut p_inner = process.inner_exclusive_access();
                let p_inner = &mut *p_inner;
                consume_pending_signal(
                    &mut p_inner.pending_signals,
                    &mut p_inner.pending_signal_queue,
                    signal,
                );
                p_inner.need_signal_handle =
                    (p_inner.pending_signals.bits() & !task_blocked.bits()) != 0;
            }
            restore_wait_mask_without_signal_frame(&task);
        }
        crate::task::signal::SigHandler::Default => {
            if is_task_level {
                let mut t_inner = task.inner_exclusive_access();
                let t_inner = &mut *t_inner;
                consume_pending_signal(
                    &mut t_inner.pending_signals,
                    &mut t_inner.pending_signal_queue,
                    signal,
                );
                t_inner.need_signal_handle =
                    (t_inner.pending_signals.bits() & !t_inner.blocked_signals.bits()) != 0;
            } else {
                let mut p_inner = process.inner_exclusive_access();
                let p_inner = &mut *p_inner;
                consume_pending_signal(
                    &mut p_inner.pending_signals,
                    &mut p_inner.pending_signal_queue,
                    signal,
                );
                p_inner.need_signal_handle =
                    (p_inner.pending_signals.bits() & !task_blocked.bits()) != 0;
            }

            restore_wait_mask_without_signal_frame(&task);

            match signal.default_action() {
                crate::task::signal::SignalAction::Terminate
                | crate::task::signal::SignalAction::Core => {
                    let core_dump = matches!(
                        signal.default_action(),
                        crate::task::signal::SignalAction::Core
                    );
                    finish_signaled_process(&process, signal, core_dump);
                }
                crate::task::signal::SignalAction::Stop => stop_process(&process, signal),
                _ => {
                    let mut p_inner = process.inner_exclusive_access();
                    p_inner.handle_default_action(signal);
                }
            }
        }
        crate::task::signal::SigHandler::Custom(handler) => {
            // 读取原始上下文，用于构建用户栈信号帧（Linux 风格）
            let original_era = ctx.era;
            let original_regs: [usize; 32] = ctx.regs;
            let original_vectors = ctx.vr;
            let original_fcc = ctx.fcc;
            let original_fcsr = ctx.fcsr;
            let saved_mask = task_blocked;
            info!("era {:#x}", original_era);

            // 统一在用户栈构建信号帧（无论是否 SA_SIGINFO）
            let sp = ctx.regs[3]; // $sp
            let Some(stack_plan) =
                prepare_signal_stack(&task, sp, target_action.sa_flags, LOONGARCH_SIGFRAME_SIZE)
            else {
                handle_signal_frame_failure(
                    &process,
                    &task,
                    signal,
                    is_task_level,
                    SysError::EFAULT,
                );
                return;
            };
            let new_sp = stack_plan.frame_sp;

            // 构建信号帧内容（清零后填充关键字段）
            let mut frame = [0u8; LOONGARCH_SIGFRAME_SIZE];
            // siginfo_t at offset 0
            write_linux_siginfo(&mut frame, signal, last_siginfo.as_ref());

            // ucontext_t at offset SIGINFO_SIZE (128)
            // uc_sigmask at ucontext + 40
            let mask = saved_mask.bits();
            frame[SIGINFO_SIZE + 40..SIGINFO_SIZE + 48].copy_from_slice(&mask.to_ne_bytes());
            write_alt_stack_to_ucontext(&mut frame, SIGINFO_SIZE, stack_plan.saved_alt_stack);

            write_linux_mcontext(
                &mut frame,
                original_era,
                &original_regs,
                &original_vectors,
                &original_fcc,
                original_fcsr,
            );

            // Write to user stack
            let bufs = match translated_byte_buffer_for_write(
                token,
                new_sp as *mut u8,
                LOONGARCH_SIGFRAME_SIZE,
            ) {
                Ok(bufs) => bufs,
                Err(error) => {
                    handle_signal_frame_failure(&process, &task, signal, is_task_level, error);
                    return;
                }
            };
            let mut written = 0;
            for buf in bufs {
                let len = buf.len().min(LOONGARCH_SIGFRAME_SIZE - written);
                buf[..len].copy_from_slice(&frame[written..written + len]);
                written += len;
            }
            commit_signal_stack(&task, &stack_plan);

            // 修改 TrapFrame 以跳转到用户态信号处理函数
            ctx.era = handler as usize; // 直接设置 era（类似 RISC-V 的 SEPC）
            ctx.regs[4] = signal.as_i32() as usize; // a0 = signal
            ctx.regs[3] = new_sp; // sp = new_sp
            ctx.regs[5] = new_sp; // a1 = &siginfo
            ctx.regs[6] = new_sp + SIGINFO_SIZE; // a2 = &ucontext
            ctx.regs[1] = crate::config::USER_RT_SIGRETURN_TRAMPOLINE; // ra = rt_sigreturn trampoline

            // 屏蔽当前信号和 sa_mask
            let mut t_inner = task.inner_exclusive_access();
            if target_action.sa_flags & SA_NODEFER == 0 {
                t_inner.blocked_signals.add(signal);
            }
            t_inner.blocked_signals |= sa_mask;
            t_inner.blocked_signals = t_inner.blocked_signals.without_unblockable();

            // 清除该信号的 pending 状态
            if is_task_level {
                {
                    let inner = &mut *t_inner;
                    consume_pending_signal(
                        &mut inner.pending_signals,
                        &mut inner.pending_signal_queue,
                        signal,
                    );
                }
                t_inner.need_signal_handle =
                    (t_inner.pending_signals.bits() & !t_inner.blocked_signals.bits()) != 0;
            } else {
                let blocked = t_inner.blocked_signals.bits();
                drop(t_inner);
                let mut p_inner = process.inner_exclusive_access();
                let p_inner = &mut *p_inner;
                consume_pending_signal(
                    &mut p_inner.pending_signals,
                    &mut p_inner.pending_signal_queue,
                    signal,
                );
                p_inner.need_signal_handle = (p_inner.pending_signals.bits() & !blocked) != 0;
            }
            error!("ctx era {:#x}", ctx.era);
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
