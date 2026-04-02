// mod.rs

//! Trap handling functionality – 统一处理用户态和内核态的 trap，
//! 通过 sstatus.SPP 位区分来源，并在内核态 trap 时使用独立栈帧，
//! 确保嵌套 trap 不会破坏用户态 trap 的上下文。

mod context;

use crate::board::MEMORY_END;
use crate::config::TRAP_CONTEXT;
use crate::mm::exception::SetPageFaultException;
use crate::mm::{KERNEL_VMSET, VMSpace, VirtAddr, exception, vm_set::AccessType};
use crate::syscall::syscall;
use crate::task::{
    current_task, current_trap_cx, current_trap_cx_user_va, current_user_token,
    exit_current_and_run_next, suspend_current_and_run_next,
};
use crate::timer::{get_time, set_next_trigger};
use alloc::task;
use core::arch::{asm, global_asm};
use log::error;
use riscv::register::satp::{self, Satp};
use riscv::register::{
    mtvec::TrapMode,
    scause::{self, Exception, Interrupt, Trap},
    sepc, sie, sstatus, stval, stvec,
};

global_asm!(include_str!("trap.S"));

/// 初始化 trap 处理：设置 stvec 指向统一的入口 __alltraps
pub fn init() {
    set_unified_trap_entry();
}

/// 设置 stvec 为 __alltraps，使用 Direct 模式
fn set_unified_trap_entry() {
    unsafe extern "C" {
        unsafe fn __alltraps();
    }
    unsafe {
        stvec::write(__alltraps as usize, TrapMode::Direct);
        println!(
            "Unified trap handler initialized at {:#x}",
            __alltraps as usize
        );
    }
}

/// 开启 S 态时钟中断
pub fn enable_timer_interrupt() {
    unsafe {
        sie::set_stimer();
    }
}

/// 用户态 trap 处理函数（由 __alltraps 在用户态 trap 时调用）
#[unsafe(no_mangle)]
pub fn trap_handler() -> ! {
    let kernel_enter_time = get_time();
    let process = current_task().unwrap().process.upgrade().unwrap();
    let mut inner = process.inner_exclusive_access();

    inner.kstart = kernel_enter_time;
    inner.time.tms_utime += kernel_enter_time - inner.ustart;

    drop(inner);
    drop(process);

    let scause = scause::read();
    let stval = stval::read();
    match scause.cause() {
        Trap::Exception(Exception::UserEnvCall) => {
            // 系统调用：跳过 ecall 指令，执行系统调用，返回结果
            let mut cx = current_trap_cx();
            //error!("\nsyscall_id:{}", cx.x[17]);
            cx.sepc += 4;
            let result = syscall(cx.x[17], [cx.x[10], cx.x[11], cx.x[12],cx.x[13],cx.x[14],cx.x[15]]);
            cx = current_trap_cx(); // 可能被 sys_exec 改变，重新获取
            cx.x[10] = result as usize;
        }
        Trap::Exception(Exception::StorePageFault)
        | Trap::Exception(Exception::InstructionPageFault)
        | Trap::Exception(Exception::LoadPageFault) => {
            // 缺页异常：尝试处理（如按需分配、写时复制）
            let va = VirtAddr::from(stval);
            let access = match scause.cause() {
                Trap::Exception(Exception::StorePageFault) => AccessType::Write,
                Trap::Exception(Exception::LoadPageFault) => AccessType::Read,
                Trap::Exception(Exception::InstructionPageFault) => AccessType::Execute,
                _ => AccessType::None,
            };
            let recoverable = if let Some(task) = current_task() {
                task.process
                    .upgrade()
                    .unwrap()
                    .inner_exclusive_access()
                    .vm_set
                    .handle_store_page_fault_set(va, access)
                    .is_some()
            } else {
                false
            };
            if !recoverable {
                error!(
                    "[kernel] Unrecoverable {:?} at va={:#x}, sepc={:#x}, killing task",
                    scause.cause(),
                    stval,
                    current_trap_cx().sepc,
                );
                exit_current_and_run_next(-2);
            }
        }
        Trap::Exception(Exception::StoreFault)
        | Trap::Exception(Exception::InstructionFault)
        | Trap::Exception(Exception::LoadFault) => {
            error!(
                "[kernel] {:?} at va={:#x}, sepc={:#x}, killing task",
                scause.cause(),
                stval,
                current_trap_cx().sepc,
            );
            exit_current_and_run_next(-2);
        }
        Trap::Exception(Exception::IllegalInstruction) => {
            error!("[kernel] IllegalInstruction, killing task");
            exit_current_and_run_next(-3);
        }
        Trap::Interrupt(Interrupt::SupervisorTimer) => {
            set_next_trigger();
            suspend_current_and_run_next();
        }
        _ => {
            panic!("Unsupported trap {:?}, stval={:#x}", scause.cause(), stval);
        }
    }
    trap_return();
}

/// 内核态 trap 处理函数（由 __alltraps 在内核态 trap 时调用）
/// 注意：此函数执行时可能发生嵌套 trap，但由于每次内核 trap 都会在内核栈上
/// 分配独立的保存区域，因此嵌套 trap 不会破坏外层的上下文。
#[unsafe(no_mangle)]
pub fn trap_from_kernel() {
    let scause = scause::read();
    let stval = stval::read();
    match scause.cause() {
        Trap::Exception(Exception::StorePageFault)
        | Trap::Exception(Exception::InstructionPageFault)
        | Trap::Exception(Exception::LoadPageFault) => {
            let va = VirtAddr::from(stval);
            let access = match scause.cause() {
                Trap::Exception(Exception::StorePageFault) => AccessType::Write,
                Trap::Exception(Exception::LoadPageFault) => AccessType::Read,
                Trap::Exception(Exception::InstructionPageFault) => AccessType::Execute,
                _ => AccessType::None,
            };
            let recoverable = if let Some(task) = current_task() {
                let process = task.process.upgrade().unwrap();
                let mut inner = process.inner_exclusive_access();
                inner
                    .vm_set
                    .handle_store_page_fault_set(va, access)
                    .is_some()
            } else {
                false
            };
            if !recoverable {
                error!(
                    "[kernel] Unrecoverable kernel trap {:?} at va={:#x}, sepc={:#x}, killing task",
                    scause.cause(),
                    stval,
                    current_trap_cx().sepc,
                );
                exit_current_and_run_next(-2);
            }
        }
        Trap::Exception(Exception::StoreFault)
        | Trap::Exception(Exception::InstructionFault)
        | Trap::Exception(Exception::LoadFault) => {
            error!(
                "[kernel] {:?} in kernel mode at va={:#x}, sepc={:#x}, killing task",
                scause.cause(),
                stval,
                current_trap_cx().sepc,
            );
            exit_current_and_run_next(-2);
        }
        Trap::Exception(Exception::IllegalInstruction) => {
            error!("[kernel] IllegalInstruction in kernel mode, killing task");
            exit_current_and_run_next(-3);
        }
        Trap::Interrupt(Interrupt::SupervisorTimer) => {
            set_next_trigger();
            suspend_current_and_run_next();
        }
        _ => {
            panic!(
                "Unsupported kernel trap {:?}, stval={:#x}",
                scause.cause(),
                stval
            );
        }
    }
    // 内核态 trap 处理完成，直接返回（由汇编代码恢复上下文并 sret）
}

/// 设置 SUM 位（允许 S 态访问用户页）
pub fn _set_sum_bit() {
    unsafe {
        let mut sstatus_val: usize;
        asm!("csrr {}, sstatus", out(reg) sstatus_val);
        sstatus_val |= 1 << 18;
        asm!("csrw sstatus, {}", in(reg) sstatus_val);
    }
}

/// 检查 SUM 位是否已设置
pub fn _check_sum() -> bool {
    let sstatus_val: usize;
    unsafe {
        asm!("csrr {}, sstatus", out(reg) sstatus_val);
    }
    (sstatus_val >> 18) & 1 == 1
}

/// 返回到用户态：将当前任务的 TrapContext 地址传入 __restore
#[unsafe(no_mangle)]
pub fn trap_return() -> ! {
    let user_enter_time = get_time();
    let process = current_task().unwrap().process.upgrade().unwrap();
    let mut inner = process.inner_exclusive_access();

    inner.ustart = user_enter_time;
    inner.time.tms_stime += user_enter_time - inner.kstart;

    drop(inner);
    drop(process);

    let trap_cx_ptr = current_trap_cx_user_va();
    unsafe extern "C" {
        unsafe fn __restore();
    }
    let restore_va = __restore as usize;
    unsafe {
        asm!(
            "fence.i",
            "jr {restore}",
            restore = in(reg) restore_va,
            in("a0") trap_cx_ptr,
            options(noreturn)
        );
    }
}

pub use context::TrapContext;
