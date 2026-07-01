//! Trap handling functionality – 统一处理用户态和内核态的 trap，
//! 通过 sstatus.SPP 位区分来源，并在内核态 trap 时使用独立栈帧，
//! 确保嵌套 trap 不会破坏用户态 trap 的上下文。

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
use log::*;

pub use polyhal::utils::addr::*;
use polyhal_trap::trap::*;
use polyhal_trap::trapframe::*;

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
    match trap_type {
        TrapType::LoadPageFault(_va) => handle_load_page_fault(_va.into()),
        TrapType::StorePageFault(_va) => handle_store_page_fault(_va.into()),
        TrapType::InstructionPageFault(_va) => {
            let va = VirtAddr::from(_va);
            if let Some(result) =
                crate::mm::handle_file_backed_page_fault_current(va, AccessType::Execute, false)
            {
                return result;
            }
            if let Some(task) = current_task() {
                let Some(process) = task.process.upgrade() else {
                    return None;
                };
                let vm_set = &mut process.inner_exclusive_access().vm_set;
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
                            TLB::flush_vaddr(va);
                            return Some(PageFaultError::Normal);
                        }
                    }
                    error!("permission denied");
                    None
                } else {
                    // PTE 不存在（lazy 分配），尝试处理缺页
                    vm_set.handle_unalloc_page_fault(va, AccessType::Execute)
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
        let vm_set = &mut process.inner_exclusive_access().vm_set;
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
        let vm_set = &mut process.inner_exclusive_access().vm_set;
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
