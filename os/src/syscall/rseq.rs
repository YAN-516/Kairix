//! Linux restartable-sequences registration and user-return handling.

use crate::error::{SysError, SyscallResult};
use crate::mm::{copy_to_user, translated_byte_buffer};
use crate::task::{current_task, current_user_token};
use polyhal_trap::trapframe::TrapFrame;

const RSEQ_FLAG_UNREGISTER: u32 = 1;
const RSEQ_ORIGINAL_SIZE: usize = 32;
const RSEQ_ALIGNMENT: usize = 32;

#[derive(Clone, Copy)]
struct Registration {
    address: usize,
    len: u32,
    signature: u32,
}

fn registration() -> Option<Registration> {
    let task = current_task()?;
    let inner = task.inner_exclusive_access();
    (inner.rseq_address != 0).then_some(Registration {
        address: inner.rseq_address,
        len: inner.rseq_len,
        signature: inner.rseq_signature,
    })
}

fn checked_user_range(address: usize, len: usize) -> Result<(), SysError> {
    if address == 0 {
        return Err(SysError::EFAULT);
    }
    let end = address.checked_add(len).ok_or(SysError::EFAULT)?;
    let user_end = polyhal::consts::USER_MEMORY_SPACE.1;
    if end == 0 || end - 1 > user_end {
        return Err(SysError::EFAULT);
    }
    Ok(())
}

fn read_user(address: usize, dst: &mut [u8]) -> Result<(), SysError> {
    checked_user_range(address, dst.len())?;
    let buffers = translated_byte_buffer(current_user_token(), address as *const u8, dst.len())?;
    let mut copied = 0;
    for buffer in buffers {
        let len = buffer.len().min(dst.len() - copied);
        dst[copied..copied + len].copy_from_slice(&buffer[..len]);
        copied += len;
    }
    if copied == dst.len() {
        Ok(())
    } else {
        Err(SysError::EFAULT)
    }
}

fn write_user(address: usize, src: &[u8]) -> Result<(), SysError> {
    checked_user_range(address, src.len())?;
    copy_to_user(current_user_token(), address as *mut u8, src)?;
    Ok(())
}

fn read_u64(address: usize) -> Result<u64, SysError> {
    let mut bytes = [0u8; 8];
    read_user(address, &mut bytes)?;
    Ok(u64::from_ne_bytes(bytes))
}

fn write_u32(address: usize, value: u32) -> Result<(), SysError> {
    write_user(address, &value.to_ne_bytes())
}

fn write_u64(address: usize, value: u64) -> Result<(), SysError> {
    write_user(address, &value.to_ne_bytes())
}

fn initialize_abi_area(address: usize) -> Result<(), SysError> {
    // struct rseq fields through mm_cid. The four trailing padding bytes in
    // the original 32-byte ABI object remain owned by userspace.
    let mut bytes = [0u8; 28];
    bytes[0..4].copy_from_slice(&u32::MAX.to_ne_bytes());
    bytes[4..8].copy_from_slice(&u32::MAX.to_ne_bytes());
    write_user(address, &bytes)
}

fn reset_abi_ids(address: usize) -> Result<(), SysError> {
    write_u32(address, 0)?;
    write_u32(address + 4, u32::MAX)?;
    write_u32(address + 20, 0)?;
    write_u32(address + 24, 0)
}

/// Register or unregister the calling thread's Linux rseq ABI area.
pub fn sys_rseq(address: usize, len: u32, flags: u32, signature: u32) -> SyscallResult {
    let task = current_task().ok_or(SysError::ESRCH)?;

    if flags & RSEQ_FLAG_UNREGISTER != 0 {
        if flags != RSEQ_FLAG_UNREGISTER {
            return Err(SysError::EINVAL);
        }
        let active = registration().ok_or(SysError::EINVAL)?;
        if active.address != address || active.len != len {
            return Err(SysError::EINVAL);
        }
        if active.signature != signature {
            return Err(SysError::EPERM);
        }
        reset_abi_ids(active.address)?;
        let mut inner = task.inner_exclusive_access();
        inner.rseq_address = 0;
        inner.rseq_len = 0;
        inner.rseq_signature = 0;
        inner.rseq_signal_fault_bypass = false;
        inner.rseq_prepare_fault_bypass = false;
        drop(inner);
        task.complete_rseq_resume_update();
        return Ok(0);
    }

    if flags != 0 {
        return Err(SysError::EINVAL);
    }

    if let Some(active) = registration() {
        if active.address != address || active.len != len {
            return Err(SysError::EINVAL);
        }
        if active.signature != signature {
            return Err(SysError::EPERM);
        }
        return Err(SysError::EBUSY);
    }

    if (len as usize) < RSEQ_ORIGINAL_SIZE || address & (RSEQ_ALIGNMENT - 1) != 0 {
        return Err(SysError::EINVAL);
    }
    checked_user_range(address, len as usize)?;
    initialize_abi_area(address)?;

    {
        let mut inner = task.inner_exclusive_access();
        inner.rseq_address = address;
        inner.rseq_len = len;
        inner.rseq_signature = signature;
        inner.rseq_signal_fault_bypass = false;
        inner.rseq_prepare_fault_bypass = false;
    }
    task.request_rseq_resume_update();
    Ok(0)
}

fn abort_active_sequence(ctx: &mut TrapFrame, registration: Registration) -> Result<(), SysError> {
    let cs_address = read_u64(registration.address + 8)? as usize;
    if cs_address == 0 {
        return Ok(());
    }
    if cs_address & (RSEQ_ALIGNMENT - 1) != 0 {
        return Err(SysError::EFAULT);
    }

    let mut descriptor = [0u8; 32];
    read_user(cs_address, &mut descriptor)?;
    let version = u32::from_ne_bytes(descriptor[0..4].try_into().unwrap());
    let flags = u32::from_ne_bytes(descriptor[4..8].try_into().unwrap());
    let start_ip = u64::from_ne_bytes(descriptor[8..16].try_into().unwrap()) as usize;
    let post_commit_offset = u64::from_ne_bytes(descriptor[16..24].try_into().unwrap()) as usize;
    let abort_ip = u64::from_ne_bytes(descriptor[24..32].try_into().unwrap()) as usize;

    let end_ip = start_ip
        .checked_add(post_commit_offset)
        .ok_or(SysError::EFAULT)?;
    let user_end = polyhal::consts::USER_MEMORY_SPACE.1;
    if version != 0
        || flags != 0
        || start_ip > user_end
        || end_ip > user_end
        || end_ip < start_ip
        || abort_ip > user_end
        || abort_ip < core::mem::size_of::<u32>()
        || (abort_ip >= start_ip && abort_ip < end_ip)
    {
        return Err(SysError::EFAULT);
    }

    if ctx.pc() >= start_ip && ctx.pc() < end_ip {
        let mut signature = [0u8; 4];
        read_user(abort_ip - 4, &mut signature)?;
        if u32::from_ne_bytes(signature) != registration.signature {
            return Err(SysError::EFAULT);
        }
        write_u64(registration.address + 8, 0)?;
        ctx.set_pc(abort_ip);
    } else {
        write_u64(registration.address + 8, 0)?;
    }
    Ok(())
}

/// Abort an active restartable sequence before constructing a signal frame.
pub(crate) fn signal_deliver(ctx: &mut TrapFrame) -> Result<(), SysError> {
    if let Some(task) = current_task() {
        let mut inner = task.inner_exclusive_access();
        if inner.rseq_signal_fault_bypass {
            inner.rseq_signal_fault_bypass = false;
            return Ok(());
        }
    }
    if let Some(active) = registration() {
        abort_active_sequence(ctx, active)?;
    }
    Ok(())
}

/// Apply pending scheduler rseq work before returning the current thread to userspace.
pub(crate) fn prepare_user_return(ctx: &mut TrapFrame) -> Result<(), SysError> {
    let task = match current_task() {
        Some(task) => task,
        None => return Ok(()),
    };
    {
        let mut inner = task.inner_exclusive_access();
        if inner.rseq_prepare_fault_bypass {
            inner.rseq_prepare_fault_bypass = false;
            return Ok(());
        }
    }
    if !task.rseq_resume_update_pending() {
        return Ok(());
    }
    let Some(active) = registration() else {
        task.complete_rseq_resume_update();
        return Ok(());
    };

    let cpu = polyhal::arch::hart_id() as u32;
    write_u32(active.address, cpu)?;
    write_u32(active.address + 4, cpu)?;
    write_u32(active.address + 20, 0)?;
    // CPU IDs are unique among concurrently running threads, and therefore
    // also form valid per-mm concurrency IDs for Kairix's one-thread-per-CPU
    // scheduler model.
    write_u32(active.address + 24, cpu)?;
    abort_active_sequence(ctx, active)?;
    task.complete_rseq_resume_update();
    Ok(())
}

/// Convert an invalid registered rseq area or descriptor into Linux's
/// catchable SIGSEGV behavior while preserving the registration for a handler
/// that repairs the mapping or descriptor.
pub(crate) fn force_sigsegv(ctx: &mut TrapFrame, error: SysError, defer_resume_once: bool) {
    let Some(task) = current_task() else {
        return;
    };
    {
        let mut inner = task.inner_exclusive_access();
        inner.rseq_signal_fault_bypass = true;
        inner.rseq_prepare_fault_bypass = defer_resume_once;
        inner
            .blocked_signals
            .remove(crate::task::signal::Signal::SigSegv);
    }
    task.request_rseq_resume_update();
    let Some(process) = task.process.upgrade() else {
        return;
    };
    process
        .inner_exclusive_access()
        .blocked_signals
        .remove(crate::task::signal::Signal::SigSegv);
    log::error!(
        "[rseq] invalid userspace ABI state: err={:?}; delivering SIGSEGV",
        error
    );
    crate::syscall::signal::deliver_signal(&process, crate::task::signal::Signal::SigSegv);
    crate::syscall::signal::handle_signals(ctx);
    task.inner_exclusive_access().rseq_signal_fault_bypass = false;
}
