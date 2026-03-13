//! Trap handling functionality
//!
//! For rCore, we have a single trap entry point, namely `__alltraps`. At
//! initialization in [`init()`], we set the `stvec` CSR to point to it.
//!
//! All traps go through `__alltraps`, which is defined in `trap.S`. The
//! assembly language code does just enough work restore the kernel space
//! context, ensuring that Rust code safely runs, and transfers control to
//! [`trap_handler()`].
//!
//! It then calls different functionality based on what exactly the exception
//! was. For example, timer interrupts trigger task preemption, and syscalls go
//! to [`syscall()`].
mod context;

use crate::config::TRAP_CONTEXT;
use crate::mm::{KERNEL_VMSET, VMSpace, VirtAddr};
use crate::syscall::syscall;
//use crate::task::{exit_current_and_run_next, suspend_current_and_run_next};
use crate::task_async::{
    current_task, current_trap_cx, current_user_token, exit_current_and_run_next, suspend_current,
};
use crate::timer::set_next_trigger;
use core::arch::{asm, global_asm};
use log::{info, warn};
use riscv::register::{
    mtvec::TrapMode,
    scause::{self, Exception, Interrupt, Trap},
    sie, sstatus, stval, stvec,
};

global_asm!(include_str!("trap.S"));
/// initialize CSR `stvec` as the entry of `__alltraps`
pub fn init() {
    set_kernel_trap_entry();
}

fn set_kernel_trap_entry() {
    unsafe {
        stvec::write(trap_from_kernel as usize, TrapMode::Direct);
    }
}

fn set_user_trap_entry() {
    unsafe extern "C" {
        unsafe fn __alltraps();
    }

    unsafe {
        stvec::write(__alltraps as usize, TrapMode::Direct);
    }
}
/// enable timer interrupt in sie CSR
pub fn enable_timer_interrupt() {
    unsafe {
        sie::set_stimer();
    }
}

#[unsafe(no_mangle)]
/// handle an interrupt, exception, or system call from user space
pub fn trap_handler() -> ! {
    //println!("--enter trap_handler--");
    set_kernel_trap_entry();
    let scause = scause::read();
    let stval = stval::read();
    match scause.cause() {
        Trap::Exception(Exception::UserEnvCall) => {
            // jump to next instruction anyway
            let mut cx = current_trap_cx();
            cx.sepc += 4;
            warn!(
                "Syscall #{} from PID {}",
                cx.x[17],
                current_task().unwrap().getpid(),
            );
            // get system call return value
            let result = syscall(cx.x[17], [cx.x[10], cx.x[11], cx.x[12]]);
            // cx is changed during sys_exec, so we have to call it again
            cx = current_trap_cx();
            cx.x[10] = result as usize;
        }
        Trap::Exception(Exception::StoreFault)
        | Trap::Exception(Exception::StorePageFault)
        | Trap::Exception(Exception::InstructionFault)
        | Trap::Exception(Exception::InstructionPageFault)
        | Trap::Exception(Exception::LoadFault)
        | Trap::Exception(Exception::LoadPageFault) => {
            println!(
                "[kernel] {:?} in application, bad addr = {:#x}, bad instruction = {:#x}, kernel killed it.",
                scause.cause(),
                stval,
                current_trap_cx().sepc,
            );
            // page fault exit code
            exit_current_and_run_next(-2);
        }
        Trap::Exception(Exception::IllegalInstruction) => {
            println!("[kernel] IllegalInstruction in application, kernel killed it.");
            // illegal instruction exit code
            exit_current_and_run_next(-3);
        }
        Trap::Interrupt(Interrupt::SupervisorTimer) => {
            set_next_trigger();
            suspend_current();
        }
        _ => {
            panic!(
                "Unsupported trap {:?}, stval = {:#x}!",
                scause.cause(),
                stval
            );
        }
    }
    //println!("before trap_return");
    trap_return();
}

fn _set_sum_bit() {
    unsafe {
        let mut sstatus_val: usize;
        // 读取当前值
        asm!("csrr {}, sstatus", out(reg) sstatus_val);

        // 设置 SUM 位
        sstatus_val |= 1 << 18;

        // 写回
        asm!("csrw sstatus, {}", in(reg) sstatus_val);
    }
}
fn _check_sum() -> bool {
    let sstatus_val: usize;
    unsafe {
        asm!("csrr {}, sstatus", out(reg) sstatus_val);
    }
    (sstatus_val >> 18) & 1 == 1
}

#[unsafe(no_mangle)]
/// set the new addr of __restore asm function in TRAMPOLINE page,
/// set the reg a0 = trap_cx_ptr, reg a1 = phy addr of usr page table,
/// finally, jump to new addr of __restore asm function
pub fn trap_return() -> ! {
    set_user_trap_entry();
    /*let kernel_stack_vaddr = VirtAddr::from(0xfffffffffffdf000);
    if let Some(pte) = KERNEL_VMSET.exclusive_access()
        .page_table().translate(kernel_stack_vaddr.floor()) {
        println!("kernel stack in kernel page table: {:?}", pte);
        println!("  PPN: {:#x}", pte.ppn().0 << 12);
        println!("  flags: {:?}", pte.flags());
    }*/

    /*println!("SUM before: {}", check_sum());
    set_sum_bit();
    println!("SUM after: {}", check_sum());*/

    let trap_cx_ptr = TRAP_CONTEXT;
    /*
        unsafe {
            let trap_cx = &*(TRAP_CONTEXT as *const TrapContext);
            println!("=== TrapContext Dump ===");
            println!("sepc: {:#x}", trap_cx.sepc);
            println!("sstatus: {:?}", trap_cx.sstatus);
            println!("kernel_sp: {:#x}", trap_cx.kernel_sp);
            println!("user registers:");
            println!("  x1 (ra): {:#x}", trap_cx.x[1]);
            println!("  x2 (sp): {:#x}", trap_cx.x[2]); // 用户栈指针
            println!("  x3 (gp): {:#x}", trap_cx.x[3]);
            println!("  x4 (tp): {:#x}", trap_cx.x[4]);
        }
    */

    //let vpn = VirtAddr::from(0xffffffffffff4fff).floor();
    //let satp = riscv::register::satp::read();
    //println!("{:#x}", satp.bits());
    //println!("current satp: {:#x}", satp.bits());

    /*if let Some(pte) = KERNEL_VMSET.exclusive_access().page_table().translate(vpn) {
        println!("kernel PTE: {:?}", pte);
        let ppn = pte.ppn().0;
        let phys = (ppn << 12) | (trap_cx_ptr & 0xfff);
        println!("  phys addr: {:#x}", phys);
    } else {
        println!("kernel PTE: NOT FOUND");
    }*/
    /*
        if let Some(task) = current_task() {
            if let Some(pte) = task
                .inner_exclusive_access()
                .vm_set
                .page_table()
                .translate(vpn)
            {
                println!("task PTE: {:?}", pte);
                let ppn = pte.ppn().0;
                let phys = (ppn << 12) | (trap_cx_ptr & 0xfff);
                println!("phys addr: {:#x}", phys);
            } else {
                println!("task PTE: NOT FOUND");
            }
        }
    */
    /*if let Some(pte) = KERNEL_VMSET.exclusive_access().page_table().translate(vpn) {
        let ppn = pte.ppn().0;
        let phys = (ppn << 12) | (trap_cx_ptr & 0xfff);
        unsafe {
            let ptr = phys as *const u8;
            let val = ptr.read_volatile();
            println!("first byte via phys: {:#x}", val);
        }
    }*/

    //let vpn = VirtAddr::from(trap_cx_ptr).floor();

    // 直接翻译，不需要保存引用
    /*let pte = if let Some(task) = current_task() {
        task.inner_exclusive_access().vm_set.page_table().translate(vpn)
    } else {
        KERNEL_VMSET.exclusive_access().page_table().translate(vpn)
    };

    if let Some(pte) = pte {
        println!("TrapContext mapped: {:?}", pte);
    } else {
        println!("TrapContext NOT mapped!");
    }*/

    /*unsafe {
        let trap_cx = &*(trap_cx_ptr as *const TrapContext);
        println!("sstatus to restore: {:#x}", trap_cx.sstatus.bits());
        println!("sstatus bits: {:b}", trap_cx.sstatus.bits());
    }*/

    //let _user_satp = current_user_token();
    unsafe extern "C" {
        //unsafe fn __alltraps();
        unsafe fn __restore();
    }
    let restore_va = __restore as usize;
    /*println!("ready to restore");
    println!("trap_cx_ptr: {:#x}", trap_cx_ptr);*/
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

#[unsafe(no_mangle)]
/// Unimplement: traps/interrupts/exceptions from kernel mode
/// Todo: Chapter 9: I/O device
pub fn trap_from_kernel() -> ! {
    use riscv::register::sepc;
    println!("stval = {:#x}, sepc = {:#x}", stval::read(), sepc::read());
    panic!("a trap {:?} from kernel!", scause::read().cause());
}

pub use context::TrapContext;
