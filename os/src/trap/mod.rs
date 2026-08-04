//! Trap handling functionality – 统一处理用户态和内核态的 trap，
//! 通过 sstatus.SPP 位区分来源，并在内核态 trap 时使用独立栈帧，
//! 确保嵌套 trap 不会破坏用户态 trap 的上下文。

use crate::config::MAX_CPU_NUM;
use crate::mm::exception::SetPageFaultException;
use crate::mm::vm_area::MapArea;
use crate::mm::vm_set::PageFaultError;
use crate::mm::{COW, vm_set};
use crate::mm::{KERNEL_VMSET, VMSpace, exception, vm_set::AccessType};
use polyhal::pagetable::{MapPermission, MappingFlags, PTE, PTEFlags, TLB};

use crate::task::signal::{SigHandler, Signal};
use crate::task::{
    current_task, current_trap_cx, current_trap_cx_user_va, current_user_token,
    exit_current_and_run_next, suspend_current_and_run_next,
};
#[cfg(target_arch = "riscv64")]
use crate::timer::set_next_trigger;

use core::arch::asm;
use core::sync::atomic::{AtomicUsize, Ordering};
use log::*;

pub use polyhal::utils::addr::*;
use polyhal_trap::trap::*;
use polyhal_trap::trapframe::*;

static PAGE_FAULT_PHASES: [AtomicUsize; MAX_CPU_NUM] = [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static PAGE_FAULT_ADDRESSES: [AtomicUsize; MAX_CPU_NUM] =
    [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static PAGE_FAULT_ACCESS: [AtomicUsize; MAX_CPU_NUM] = [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static PAGE_FAULT_PIDS: [AtomicUsize; MAX_CPU_NUM] =
    [const { AtomicUsize::new(usize::MAX) }; MAX_CPU_NUM];
static USER_SIGILL_SEQUENCES: [AtomicUsize; MAX_CPU_NUM] =
    [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static USER_SIGILL_PIDS: [AtomicUsize; MAX_CPU_NUM] =
    [const { AtomicUsize::new(usize::MAX) }; MAX_CPU_NUM];
static USER_SIGILL_PCS: [AtomicUsize; MAX_CPU_NUM] = [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static USER_SIGILL_DETAILS: [AtomicUsize; MAX_CPU_NUM] =
    [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static USER_SIGILL_MAPPED_INSTRUCTIONS: [AtomicUsize; MAX_CPU_NUM] =
    [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static USER_SIGILL_MAPPED_LENGTHS: [AtomicUsize; MAX_CPU_NUM] =
    [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static USER_SIGILL_STATUS: [AtomicUsize; MAX_CPU_NUM] =
    [const { AtomicUsize::new(0) }; MAX_CPU_NUM];

/// Lock-free user page-fault progress for cross-CPU stall diagnosis.
#[derive(Debug, Clone, Copy)]
pub struct PageFaultProgress {
    /// 0=not in a fault; 10-19=trap handler; 20-27=file-backed path.
    pub phase: usize,
    /// Faulting virtual address.
    pub address: usize,
    /// 1=read, 2=write, 3=execute.
    pub access: usize,
    /// Process that entered the page-fault handler.
    pub pid: usize,
}

/// Last user-mode illegal-instruction trap observed on one CPU.
///
/// The record remains available after the process has been reaped so a later
/// `/proc/kairix_perf` sample can still distinguish an unsupported instruction
/// from a damaged or unmapped executable page.
#[derive(Debug, Clone, Copy)]
pub struct UserSigillSnapshot {
    /// Per-CPU count of recorded illegal-instruction traps.
    pub sequence: usize,
    /// Process that took the trap.
    pub pid: usize,
    /// Faulting user program counter.
    pub pc: usize,
    /// Architecture-provided illegal-instruction detail (`stval` on RISC-V).
    pub detail: usize,
    /// Four bytes read from the mapped user page at `pc`, in native order.
    pub mapped_instruction: usize,
    /// Number of mapped instruction bytes that were available.
    pub mapped_len: usize,
    /// Saved user status (`sstatus` on RISC-V, `prmd` on LoongArch64).
    pub status: usize,
}

/// Persist one illegal-instruction trap for later procfs diagnostics.
pub(crate) fn record_user_sigill(
    pid: usize,
    pc: usize,
    detail: usize,
    mapped_instruction: usize,
    mapped_len: usize,
    status: usize,
) {
    let cpu = polyhal::arch::hart_id();
    if cpu >= MAX_CPU_NUM {
        return;
    }
    USER_SIGILL_PIDS[cpu].store(pid, Ordering::Relaxed);
    USER_SIGILL_PCS[cpu].store(pc, Ordering::Relaxed);
    USER_SIGILL_DETAILS[cpu].store(detail, Ordering::Relaxed);
    USER_SIGILL_MAPPED_INSTRUCTIONS[cpu].store(mapped_instruction, Ordering::Relaxed);
    USER_SIGILL_MAPPED_LENGTHS[cpu].store(mapped_len, Ordering::Relaxed);
    USER_SIGILL_STATUS[cpu].store(status, Ordering::Relaxed);
    USER_SIGILL_SEQUENCES[cpu].fetch_add(1, Ordering::Release);
}

/// Return the last illegal-instruction record from every CPU.
pub fn user_sigill_snapshots() -> [UserSigillSnapshot; MAX_CPU_NUM] {
    core::array::from_fn(|cpu| UserSigillSnapshot {
        sequence: USER_SIGILL_SEQUENCES[cpu].load(Ordering::Acquire),
        pid: USER_SIGILL_PIDS[cpu].load(Ordering::Relaxed),
        pc: USER_SIGILL_PCS[cpu].load(Ordering::Relaxed),
        detail: USER_SIGILL_DETAILS[cpu].load(Ordering::Relaxed),
        mapped_instruction: USER_SIGILL_MAPPED_INSTRUCTIONS[cpu].load(Ordering::Relaxed),
        mapped_len: USER_SIGILL_MAPPED_LENGTHS[cpu].load(Ordering::Relaxed),
        status: USER_SIGILL_STATUS[cpu].load(Ordering::Relaxed),
    })
}

struct PageFaultProgressGuard {
    cpu: usize,
}

impl PageFaultProgressGuard {
    fn new(address: usize, access: AccessType) -> Self {
        let cpu = polyhal::arch::hart_id();
        if cpu < MAX_CPU_NUM {
            let access = match access {
                AccessType::Read => 1,
                AccessType::Write => 2,
                AccessType::Execute => 3,
                AccessType::None => 0,
            };
            let pid = current_task()
                .map(|task| task.process_id())
                .unwrap_or(usize::MAX);
            PAGE_FAULT_ADDRESSES[cpu].store(address, Ordering::Relaxed);
            PAGE_FAULT_ACCESS[cpu].store(access, Ordering::Relaxed);
            PAGE_FAULT_PIDS[cpu].store(pid, Ordering::Relaxed);
            PAGE_FAULT_PHASES[cpu].store(1, Ordering::Release);
        }
        Self { cpu }
    }
}

impl Drop for PageFaultProgressGuard {
    fn drop(&mut self) {
        if self.cpu < MAX_CPU_NUM {
            PAGE_FAULT_PHASES[self.cpu].store(0, Ordering::Release);
        }
    }
}

pub(crate) fn record_page_fault_phase(phase: usize) {
    let cpu = polyhal::arch::hart_id();
    if cpu < MAX_CPU_NUM {
        PAGE_FAULT_PHASES[cpu].store(phase, Ordering::Release);
    }
}

/// Return the latest page-fault progress published by one CPU.
pub fn page_fault_progress(cpu: usize) -> PageFaultProgress {
    if cpu >= MAX_CPU_NUM {
        return PageFaultProgress {
            phase: 0,
            address: 0,
            access: 0,
            pid: usize::MAX,
        };
    }
    PageFaultProgress {
        phase: PAGE_FAULT_PHASES[cpu].load(Ordering::Acquire),
        address: PAGE_FAULT_ADDRESSES[cpu].load(Ordering::Relaxed),
        access: PAGE_FAULT_ACCESS[cpu].load(Ordering::Relaxed),
        pid: PAGE_FAULT_PIDS[cpu].load(Ordering::Relaxed),
    }
}

/// 开启 S 态时钟中断
pub fn enable_timer_interrupt() {
    polyhal::timer::enable_timer_interrupt();
}

///
pub fn disable_timer_interrupt() {
    polyhal::timer::disable_timer_interrupt();
}

#[allow(unused, missing_docs)]
pub fn handle_page_fault(trap_type: TrapType) -> Option<PageFaultError> {
    // info!("handle_page_fault: trap_type={:?}", trap_type);
    let (fault_address, access) = match &trap_type {
        TrapType::LoadPageFault(va) => (*va, AccessType::Read),
        TrapType::StorePageFault(va) => (*va, AccessType::Write),
        TrapType::InstructionPageFault(va) => (*va, AccessType::Execute),
        _ => (0, AccessType::None),
    };
    if let Some(task) = current_task() {
        let access_code = match access {
            AccessType::Read => 1,
            AccessType::Write => 2,
            AccessType::Execute => 3,
            AccessType::None => 0,
        };
        task.note_page_fault(access_code);
    }
    let _progress = PageFaultProgressGuard::new(fault_address, access);
    match trap_type {
        TrapType::LoadPageFault(_va) => handle_load_page_fault(_va.into()),
        TrapType::StorePageFault(_va) => handle_store_page_fault(_va.into()),
        TrapType::InstructionPageFault(_va) => {
            let va = VirtAddr::from(_va);
            record_page_fault_phase(10);
            if let Some(result) =
                crate::mm::handle_file_backed_page_fault_current(va, AccessType::Execute, false)
            {
                return result;
            }
            record_page_fault_phase(11);
            if let Some(task) = current_task() {
                let Some(process) = task.process.upgrade() else {
                    return None;
                };
                record_page_fault_phase(13);
                let mut vm_set = process.vm_exclusive_access();
                record_page_fault_phase(14);
                if let Some(pte) = vm_set.translate(va.floor()) {
                    // PTE 存在但权限不足（例如缺少 X 权限）
                    trace!(
                        "InstructionPageFault: pte flag {:?} at va={:#x}",
                        pte.flags(),
                        va.0
                    );
                    // 检查 area 是否有 X 权限，如果有则更新 PTE
                    if let Some(area) = vm_set.find_area(va) {
                        if area.perm().contains(MapPermission::X) {
                            info!("fixing PTE for exec permission at va={:#x}", va.0);
                            let new_flags =
                                PTEFlags::from(MappingFlags::from(*area.perm())) | PTEFlags::V;
                            if let Some(pte) = vm_set.page_table.find_pte(va.floor()) {
                                *pte = PTE::new(pte.ppn(), new_flags);
                            }
                            record_page_fault_phase(15);
                            polyhal::multicore::synchronize_instruction_cache(vm_set.token());
                            record_page_fault_phase(16);
                            return Some(PageFaultError::Normal);
                        }
                    }
                    error!("permission denied");
                    None
                } else {
                    // PTE 不存在（lazy 分配），尝试处理缺页
                    record_page_fault_phase(17);
                    let result = vm_set.handle_unalloc_page_fault(va, AccessType::Execute);
                    record_page_fault_phase(18);
                    result
                }
            } else {
                // error!("nothing");
                None
            }
        }
        _ => None,
    }
}
///
pub fn handle_store_page_fault(va: VirtAddr) -> Option<PageFaultError> {
    if let Some(result) =
        crate::mm::handle_file_backed_page_fault_current(va, AccessType::Write, false)
    {
        return result;
    }
    if let Some(task) = current_task() {
        let Some(process) = task.process.upgrade() else {
            return None;
        };
        let mut vm_set = process.vm_exclusive_access();
        let pte_opt = vm_set.translate(va.floor());
        if let Some(pte) = pte_opt {
            trace!("pte flag {:?} {:#x}", pte.flags(), pte.ppn().0);
        }

        // 先尝试查找 VMA
        if let Some(vma) = vm_set.find_area(va) {
            let cow_flag = vma.cow_flag();
            if cow_flag && pte_opt.is_some() {
                vm_set.handle_cow_page_fault(va)
            } else if let Some(pte) = pte_opt {
                // PTE 已存在但不是 COW：检查是否为真正的权限不足（如写入只读页）
                if !pte.writable() {
                    if let Some(area) = vm_set.find_area(va) {
                        if !area.perm().contains(MapPermission::W) {
                            // VMA 也没有写权限，这是非法访问，应触发 SIGSEGV
                            return None;
                        }
                    }
                    // VMA 有写权限但 PTE 没有，可能是 mprotect 后 PTE 未更新，
                    // 交给 handle_unalloc_page_fault 修正权限
                }
                vm_set.handle_unalloc_page_fault(va, AccessType::Write)
            } else {
                // PTE 不存在只能说明这一页还没 lazy 分配；不能绕过 VMA 权限。
                if !vma.perm().contains(MapPermission::W) {
                    return None;
                }
                vm_set.handle_unalloc_page_fault(va, AccessType::Write)
            }
        } else {
            // 没有找到 VMA，尝试自动扩展栈
            if vm_set.try_expand_stack(va).is_some() {
                return Some(PageFaultError::Normal);
            }
            error!("no vma found for va {:#x}", va.0);
            None
        }
    } else {
        None
    }
}

///
pub fn handle_load_page_fault(va: VirtAddr) -> Option<PageFaultError> {
    if let Some(result) =
        crate::mm::handle_file_backed_page_fault_current(va, AccessType::Read, true)
    {
        return result;
    }
    if let Some(task) = current_task() {
        let Some(process) = task.process.upgrade() else {
            return None;
        };
        let mut vm_set = process.vm_exclusive_access();
        // 校验读权限：若 VMA 无读权限，说明是非法访问，应触发 SIGSEGV
        if let Some(area) = vm_set.find_area(va) {
            info!(
                "[DEBUG] handle_load_page_fault: found area for va={:#x}",
                va.0
            );
            if !area.perm().contains(MapPermission::R) && !area.perm().contains(MapPermission::X) {
                return None;
            }
            vm_set.handle_unalloc_page_fault(va, AccessType::Read)
        } else {
            info!(
                "[DEBUG] handle_load_page_fault: no area found for va={:#x}",
                va.0
            );
            // 没有找到 VMA，尝试自动扩展栈（读栈也可能触发缺页）
            if vm_set.try_expand_stack(va).is_some() {
                return Some(PageFaultError::Normal);
            }
            error!("no vma found for va {:#x}", va.0);
            None
        }
    } else {
        None
    }
}

/// 设置 SUM 位（允许 S 态访问用户页）
#[cfg(target_arch = "riscv64")]
pub fn _set_sum_bit() {
    unsafe {
        let mut sstatus_val: usize;
        asm!("csrr {}, sstatus", out(reg) sstatus_val);
        sstatus_val |= 1 << 18;
        asm!("csrw sstatus, {}", in(reg) sstatus_val);
    }
}
#[cfg(target_arch = "loongarch64")]
///
pub fn _set_sum_bit() {}

/// 检查 SUM 位是否已设置
#[cfg(target_arch = "riscv64")]
pub fn _check_sum() -> bool {
    let sstatus_val: usize;
    unsafe {
        asm!("csrr {}, sstatus", out(reg) sstatus_val);
    }
    (sstatus_val >> 18) & 1 == 1
}

#[cfg(target_arch = "loongarch64")]
///
pub fn _check_sum() -> bool {
    true
}
