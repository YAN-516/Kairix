#[macro_use]
mod macros;

use super::{EscapeReason, TrapType};
use crate::trapframe::TrapFrame;
use core::arch::naked_asm;
use polyhal::consts::VIRT_ADDR_START;
use riscv::{
    interrupt::{Exception, Interrupt},
    register::{
        scause::{self, Trap},
        sie, sip, sstatus, stval,
        stvec::{self, Stvec},
    },
};

// Initialize the trap handler.
pub(crate) fn init() {
    unsafe {
        let mut stvec = Stvec::from_bits(0);
        stvec.set_address(kernelvec as usize);
        stvec.set_trap_mode(stvec::TrapMode::Direct);
        stvec::write(stvec);
    }
    polyhal::multicore::enable_tlb_shootdown_ipi();

    // Initialize the timer component
    // #[cfg(target_arch = "riscv64")]
    // polyhal::timer::init();
}

// 内核中断回调
#[no_mangle]
fn kernel_callback(context: &mut TrapFrame) -> TrapType {
    let scause = scause::read();
    let stval = stval::read();
    let from_user = context.from_user();
    polyhal::multicore::record_trap_entry(scause.bits(), from_user, context.sepc, stval);
    polyhal::multicore::record_interrupt_state(
        1,
        context.sstatus.bits(),
        sstatus::read().bits(),
        sie::read().bits(),
        sip::read().bits(),
        context.sepc,
    );
    // println!("trap type from kernel_callback {:?}", scause.cause());

    let trap_type = match scause.cause().try_into().unwrap() {
        // 中断异常
        Trap::Exception(Exception::Breakpoint) => {
            context.sepc += 2;
            TrapType::Breakpoint
        }
        Trap::Exception(Exception::LoadFault) => {
            if stval > VIRT_ADDR_START {
                panic!("kernel error: {:#x}", stval);
            }
            TrapType::Unknown
        }
        Trap::Exception(Exception::UserEnvCall) => TrapType::SysCall,
        // 时钟中断
        Trap::Interrupt(Interrupt::SupervisorTimer) => TrapType::Timer,
        Trap::Interrupt(Interrupt::SupervisorSoft) => {
            polyhal::multicore::record_trap_stage(2);
            if from_user {
                // Software IPIs bypass the OS trap callback. Stop advertising
                // user execution before returning to the kernel task loop so
                // a concurrent sender cannot wait on a CPU taking kernel
                // locks with interrupts disabled.
                polyhal::multicore::mark_current_cpu_kernel_entry();
            }
            let reschedule = polyhal::multicore::handle_ipi();
            if from_user && reschedule {
                TrapType::Reschedule
            } else {
                // The shootdown handler is deliberately lock-free and complete;
                // do not enter the OS interrupt path while it may be nested inside
                // a no-IRQ kernel critical section.
                polyhal::multicore::record_trap_stage(4);
                return TrapType::Handled;
            }
        }
        Trap::Exception(Exception::StorePageFault) => TrapType::StorePageFault(stval),
        Trap::Exception(Exception::StoreFault) => TrapType::StorePageFault(stval),
        Trap::Exception(Exception::InstructionPageFault) => TrapType::InstructionPageFault(stval),
        Trap::Exception(Exception::IllegalInstruction) => TrapType::IllegalInstruction(stval),
        Trap::Exception(Exception::LoadPageFault) => TrapType::LoadPageFault(stval),
        Trap::Interrupt(Interrupt::SupervisorExternal) => TrapType::SupervisorExternal,
        _ => {
            log::error!(
                "内核态中断发生: {:#x} {:?}  stval {:#x}  sepc: {:#x}",
                scause.bits(),
                scause.cause(),
                stval,
                context.sepc
            );
            panic!("未知中断: {:#x?}", context);
        }
    };
    polyhal::multicore::record_trap_stage(3);
    unsafe { super::_interrupt_for_arch(context, trap_type, 0) };
    polyhal::multicore::record_trap_stage(4);
    trap_type
}

#[naked]
pub unsafe extern "C" fn kernelvec() {
    naked_asm!(
        includes_trap_macros!(),
        // 宏定义
        r"
            .align 4
            .altmacro
        
            csrrw   sp, sscratch, sp
            bnez    sp, uservec
            csrr    sp, sscratch

            addi    sp, sp, -{cx_size}
            
            SAVE_GENERAL_REGS
            csrw    sscratch, x0

            mv      a0, sp

            call kernel_callback

            LOAD_GENERAL_REGS
            sret
        ",
        // TrapFrame is currently 536 bytes after saving the complete floating
        // point state.  Reserving that exact size would misalign sp by eight
        // bytes before the call to kernel_callback, violating the RISC-V ABI.
        cx_size = const crate::trapframe::KERNEL_TRAPFRAME_SIZE,
    )
}

#[naked]
#[no_mangle]
extern "C" fn user_restore(context: *mut TrapFrame) {
    unsafe {
        naked_asm!(
            includes_trap_macros!(),
            // 在内核态栈中开一个空间来存储内核态信息
            // 下次发生中断必然会进入中断入口然后恢复这个上下文.
            // 仅保存 Callee-saved regs、gp、tp、ra.
            ".align 4
                la       t0, __KAIRIX_SCHEDULER_PHASES
                slli     t1, tp, 3
                add      t0, t0, t1
                li       t1, 170
                sd       t1, 0(t0)

                addi    sp, sp, -18*8

                # Slot zero is otherwise unused by the saved kernel context.
                # Keep the CPU id there so the final marker can still select
                # its per-CPU cell after LOAD_GENERAL_REGS restores user tp.
                sd       tp,  0*8(sp)
                STR      sp,  1
                STR      gp,  2
                STR      tp,  3
                STR      s0,  4
                STR      s1,  5
                STR      s2,  6
                STR      s3,  7
                STR      s4,  8
                STR      s5,  9
                STR      s6,  10
                STR      s7,  11
                STR      s8,  12
                STR      s9,  13
                STR      s10, 14
                STR      s11, 15
                STR      a0,  16
                STR      ra,  17
            ",
            // 将栈信息保存到用户栈.
            // a0 是传入的Context, 然后下面会再次恢复 sp 地址.
            "   sd       sp, 8*0(a0)
                csrw     sscratch, a0
                mv       sp, a0

                la       t0, __KAIRIX_SCHEDULER_PHASES
                slli     t1, tp, 3
                add      t0, t0, t1
                li       t1, 171
                sd       t1, 0(t0)

                LOAD_FP_REGS

                la       t0, __KAIRIX_SCHEDULER_PHASES
                slli     t1, tp, 3
                add      t0, t0, t1
                li       t1, 172
                sd       t1, 0(t0)

                LOAD_GENERAL_REGS

                # General registers, including user tp, are now live. Recover
                # the kernel CPU id through sscratch -> TrapFrame.x0 -> saved
                # kernel slot zero, publish the last pre-sret boundary, then
                # restore the three user temporaries touched by this sequence.
                csrr     t2, sscratch
                ld       t0, 0*8(t2)
                ld       t1, 0*8(t0)
                la       t0, __KAIRIX_SCHEDULER_PHASES
                slli     t1, t1, 3
                add      t0, t0, t1
                li       t1, 173
                sd       t1, 0(t0)

                # STIE/SSIE are per-hart state rather than part of TrapFrame. Some
                # supervisor-only paths (notably synchronous TLB shootdown)
                # temporarily mask it, and a direct/Handled trap return does
                # not necessarily pass through the OS user-return helper.
                # Re-establish both halves of the user preemption invariant at
                # the final boundary: enable supervisor timer/software delivery
                # and make sret restore supervisor interrupts from SPIE.
                li       t0, (1 << 5)
                csrs     sstatus, t0
                li       t0, ((1 << 5) | (1 << 1))
                csrs     sie, t0

                csrr     t2, sscratch
                ld       t0, 5*8(t2)
                ld       t1, 6*8(t2)
                ld       t2, 7*8(t2)
                sret
            ",
        )
    }
}

#[naked]
#[no_mangle]
#[allow(named_asm_labels)]
pub unsafe extern "C" fn uservec() {
    naked_asm!(
        includes_trap_macros!(),
        // 保存 general registers, 除了 sp
        "
        SAVE_GENERAL_REGS
        csrw    sscratch, x0

        SAVE_FP_REGS

        mv      a0, sp
        ld      sp, 0*8(a0)
        sd      x0, 0*8(a0)
    ",
        // 恢复内核上下文信息, 仅恢复 callee-saved 寄存器和 ra、gp、tp
        "  
        LDR      gp,  2
        LDR      tp,  3
        LDR      s0,  4
        LDR      s1,  5
        LDR      s2,  6
        LDR      s3,  7
        LDR      s4,  8
        LDR      s5,  9
        LDR      s6,  10
        LDR      s7,  11
        LDR      s8,  12
        LDR      s9,  13
        LDR      s10, 14
        LDR      s11, 15
        LDR      ra,  17
        
        LDR      sp,  1
    ",
        // 回收栈
        "addi sp, sp, 18*8
        ret
    ",
    );
}

/// Return EscapeReson related to interrupt type.
pub fn run_user_task(context: &mut TrapFrame) -> EscapeReason {
    polyhal::multicore::record_interrupt_state(
        2,
        context.sstatus.bits(),
        sstatus::read().bits(),
        sie::read().bits(),
        sip::read().bits(),
        context.sepc,
    );
    user_restore(context);
    kernel_callback(context).into()
}

/// Run user task until interrupt is received.
pub fn run_user_task_forever(context: &mut TrapFrame) -> ! {
    loop {
        polyhal::multicore::record_interrupt_state(
            2,
            context.sstatus.bits(),
            sstatus::read().bits(),
            sie::read().bits(),
            sip::read().bits(),
            context.sepc,
        );
        user_restore(context);
        kernel_callback(context);
    }
}
