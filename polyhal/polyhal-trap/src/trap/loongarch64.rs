#[macro_use]
mod macros;
mod unaligned;
use super::{EscapeReason, TrapType};
use crate::trapframe::TrapFrame;
use core::arch::naked_asm;
use loongArch64::register::estat::{self, Exception, Trap};
use loongArch64::register::{
    badv, crmd, ecfg, eentry, euen, prmd, pwch, pwcl, stlbps, ticlr, tlbidx, tlbrehi, tlbrentry,
};
use polyhal::irq::TIMER_IRQ;
use polyhal::println;
use unaligned::emulate_load_store_insn;

#[repr(C)]
struct ExceptionTableEntry {
    fault: usize,
    fixup: usize,
}

unsafe extern "C" {
    static __ex_table_start: u8;
    static __ex_table_end: u8;
}

fn exception_fixup(era: usize) -> Option<usize> {
    let mut entry = core::ptr::addr_of!(__ex_table_start).cast::<ExceptionTableEntry>();
    let end = core::ptr::addr_of!(__ex_table_end).cast::<ExceptionTableEntry>();
    while entry < end {
        let current = unsafe { &*entry };
        if current.fault == era {
            return Some(current.fixup);
        }
        entry = unsafe { entry.add(1) };
    }
    None
}

#[naked]
pub unsafe extern "C" fn user_vec() {
    naked_asm!(
        includes_trap_macros!(),
        "
            csrrd   $sp,  KSAVE_CTX
            SAVE_REGS

            csrrd   $sp,  KSAVE_KSP
            ld.d    $ra,  $sp, 0*8
            ld.d    $tp,  $sp, 1*8
            ld.d    $r21, $sp, 2*8
            ld.d    $s9,  $sp, 3*8
            ld.d    $s0,  $sp, 4*8
            ld.d    $s1,  $sp, 5*8
            ld.d    $s2,  $sp, 6*8
            ld.d    $s3,  $sp, 7*8
            ld.d    $s4,  $sp, 8*8
            ld.d    $s5,  $sp, 9*8
            ld.d    $s6,  $sp, 10*8
            ld.d    $s7,  $sp, 11*8
            ld.d    $s8,  $sp, 12*8
            addi.d  $sp,  $sp, 13*8
            ret

        ",
        tf_vr = const core::mem::offset_of!(TrapFrame, vr),
        tf_fcc = const core::mem::offset_of!(TrapFrame, fcc),
        tf_fcsr = const core::mem::offset_of!(TrapFrame, fcsr),
    );
}

#[naked]
#[no_mangle]
pub extern "C" fn user_restore(context: *mut TrapFrame) {
    unsafe {
        naked_asm!(
            includes_trap_macros!(),
            r"
                la.local  $t0, __KAIRIX_SCHEDULER_PHASES
                slli.d   $t1, $tp, 3
                add.d    $t0, $t0, $t1
                li.w     $t1, 170
                st.d     $t1, $t0, 0

                addi.d  $sp,  $sp, -13*8
                st.d    $ra,  $sp, 0*8
                st.d    $tp,  $sp, 1*8
                st.d    $r21, $sp, 2*8
                st.d    $s9,  $sp, 3*8
                st.d    $s0,  $sp, 4*8
                st.d    $s1,  $sp, 5*8
                st.d    $s2,  $sp, 6*8
                st.d    $s3,  $sp, 7*8
                st.d    $s4,  $sp, 8*8
                st.d    $s5,  $sp, 9*8
                st.d    $s6,  $sp, 10*8
                st.d    $s7,  $sp, 11*8
                st.d    $s8,  $sp, 12*8

                csrwr    $sp, KSAVE_KSP   // SAVE kernel_sp to SAVEn(0)
                move     $sp, $a0         // TIPS: csrwr will write the old value to rd
                csrwr    $a0, KSAVE_CTX   // SAVE user context addr to SAVEn(1)

                la.local  $t0, __KAIRIX_SCHEDULER_PHASES
                slli.d   $t1, $tp, 3
                add.d    $t0, $t0, $t1
                li.w     $t1, 171
                st.d     $t1, $t0, 0

                la.local  $t0, __KAIRIX_SCHEDULER_PHASES
                slli.d   $t1, $tp, 3
                add.d    $t0, $t0, $t1
                li.w     $t1, 172
                st.d     $t1, $t0, 0

                LOAD_REGS

                // LOAD_REGS has installed user tp and sp. Recover the kernel
                // CPU id from the saved kernel context, publish the final
                // pre-ertn boundary, and restore the user temporaries touched
                // by this diagnostic sequence from KSAVE_CTX.
                csrrd    $t2, KSAVE_KSP
                ld.d     $t1, $t2, 1*8
                la.local  $t0, __KAIRIX_SCHEDULER_PHASES
                slli.d   $t1, $t1, 3
                add.d    $t0, $t0, $t1
                li.w     $t1, 173
                st.d     $t1, $t0, 0
                csrrd    $t2, KSAVE_CTX
                ld.d     $t0, $t2, 12*8
                ld.d     $t1, $t2, 13*8
                ld.d     $t2, $t2, 14*8
                ertn
            ",
            tf_vr = const core::mem::offset_of!(TrapFrame, vr),
            tf_fcc = const core::mem::offset_of!(TrapFrame, fcc),
            tf_fcsr = const core::mem::offset_of!(TrapFrame, fcsr),
        )
    }
}

#[allow(dead_code)]
#[inline(always)]
pub fn enable_irq() {
    // crmd::set_ie(true);
    prmd::set_pie(true);
}

#[inline(always)]
pub fn disable_irq() {
    // crmd::set_ie(false);
    prmd::set_pie(false);
}

pub fn run_user_task(cx: &mut TrapFrame) -> EscapeReason {
    polyhal::multicore::record_interrupt_state(
        2,
        cx.prmd,
        crmd::read().raw(),
        ecfg::read().lie().bits(),
        estat::read().is(),
        cx.era,
    );
    user_restore(cx);
    loongarch64_trap_handler(cx).into()
}

#[naked]
pub unsafe extern "C" fn trap_vector_base() {
    naked_asm!(
        includes_trap_macros!(),
        "
            .balign 4096
            // Check whether it was from user privilege.
            csrwr   $sp, KSAVE_USP
            csrrd   $sp, 0x1
            andi    $sp, $sp, 0x3
            bnez    $sp, {user_vec} 
        
            csrrd   $sp, KSAVE_USP
            addi.d  $sp, $sp, -{trapframe_size} // allocate space
        
            // save the registers.

            SAVE_REGS
        
            move    $a0, $sp
            bl      {trap_handler}
        
            // Load registers from sp, include new sp
            LOAD_REGS
            ertn
        ",
        trapframe_size = const crate::trapframe::KERNEL_TRAPFRAME_SIZE,
        user_vec = sym user_vec,
        trap_handler = sym loongarch64_trap_handler,
        tf_vr = const core::mem::offset_of!(TrapFrame, vr),
        tf_fcc = const core::mem::offset_of!(TrapFrame, fcc),
        tf_fcsr = const core::mem::offset_of!(TrapFrame, fcsr),
    );
}

#[naked]
pub unsafe extern "C" fn tlb_fill() {
    naked_asm!(
        "
        .balign 4096
        csrwr  $t0, 0x8b
        csrrd  $t0, 0x1b
        lddir  $t0, $t0, 3
        andi   $t0, $t0, 1
        beqz   $t0, 1f

        csrrd  $t0, 0x1b
        lddir  $t0, $t0, 3
        addi.d $t0, $t0, -1
        lddir  $t0, $t0, 1
        andi   $t0, $t0, 1
        beqz   $t0, 1f
        csrrd  $t0, 0x1b
        lddir  $t0, $t0, 3
        addi.d $t0, $t0, -1
        lddir  $t0, $t0, 1
        addi.d $t0, $t0, -1

        ldpte  $t0, 0
        ldpte  $t0, 1
        csrrd  $t0, 0x8c
        csrrd  $t0, 0x8d
        csrrd  $t0, 0x0
    2:
        tlbfill
        csrrd  $t0, 0x89
        srli.d $t0, $t0, 13
        slli.d $t0, $t0, 13
        csrwr  $t0, 0x11
        tlbsrch
        tlbrd
        csrrd  $t0, 0x12
        csrrd  $t0, 0x13
        csrrd  $t0, 0x8b
        ertn
    1:
        csrrd  $t0, 0x8e
        ori    $t0, $t0, 0xC
        csrwr  $t0, 0x8e

        rotri.d $t0, $t0, 61
        ori    $t0, $t0, 3
        rotri.d $t0, $t0, 3

        csrwr  $t0, 0x8c
        csrrd  $t0, 0x8c
        csrwr  $t0, 0x8d
        b      2b
    ",
    );
}

pub const PS_4K: usize = 0x0c;
pub const _PS_16K: usize = 0x0e;
pub const _PS_2M: usize = 0x15;
pub const _PS_1G: usize = 0x1e;

pub const PAGE_SIZE_SHIFT: usize = 12;

pub fn tlb_init(tlbrentry: usize) {
    // // setup PWCTL
    // unsafe {
    // asm!(
    //     "li.d     $r21,  0x4d52c",     // (9 << 15) | (21 << 10) | (9 << 5) | 12
    //     "csrwr    $r21,  0x1c",        // LOONGARCH_CSR_PWCTL0
    //     "li.d     $r21,  0x25e",       // (9 << 6)  | 30
    //     "csrwr    $r21,  0x1d",         //LOONGARCH_CSR_PWCTL1
    //     )
    // }

    tlbidx::set_ps(PS_4K);
    stlbps::set_ps(PS_4K);
    tlbrehi::set_ps(PS_4K);

    // set hardware
    pwcl::set_pte_width(8); // 64-bits
    pwcl::set_ptbase(PAGE_SIZE_SHIFT);
    pwcl::set_ptwidth(PAGE_SIZE_SHIFT - 3);

    pwcl::set_dir1_base(PAGE_SIZE_SHIFT + PAGE_SIZE_SHIFT - 3);
    pwcl::set_dir1_width(PAGE_SIZE_SHIFT - 3);

    pwch::set_dir3_base(PAGE_SIZE_SHIFT + PAGE_SIZE_SHIFT - 3 + PAGE_SIZE_SHIFT - 3);
    pwch::set_dir3_width(PAGE_SIZE_SHIFT - 3);
    println!("tlb rentry {:#x}, ", tlbrentry);
    tlbrentry::set_tlbrentry(tlbrentry & 0xFFFF_FFFF_FFFF);
    // pgdl::set_base(kernel_pgd_base);
    // pgdh::set_base(kernel_pgd_base);
}

#[inline]
pub fn init() {
    println!("init --------------------------");

    tlb_init(tlb_fill as usize);
    ecfg::set_vs(0);
    eentry::set_eentry(trap_vector_base as usize);
    polyhal::multicore::enable_tlb_shootdown_ipi();
}

fn loongarch64_trap_handler(tf: &mut TrapFrame) -> TrapType {
    let estat = estat::read();
    let from_user = tf.prmd & 0b11 == 0b11;
    polyhal::multicore::record_trap_entry(estat.raw(), from_user, tf.era, badv::read().vaddr());
    polyhal::multicore::record_interrupt_state(
        1,
        tf.prmd,
        crmd::read().raw(),
        ecfg::read().lie().bits(),
        estat.is(),
        tf.era,
    );
    // The unaligned-access helpers deliberately touch the current user's
    // address space. Recover at their annotated fixup site if that access
    // faults in kernel mode; the outer user trap will then report the fault.
    if tf.prmd & 0b11 == 0 {
        if let Some(fixup) = exception_fixup(tf.era) {
            tf.era = fixup;
            polyhal::multicore::record_trap_stage(4);
            return TrapType::Handled;
        }
    }
    let trap_type = match estat.cause() {
        Trap::Exception(Exception::Breakpoint) => {
            tf.era += 4;
            TrapType::Breakpoint
        }
        Trap::Exception(Exception::AddressNotAligned) => {
            let fault_addr = badv::read().vaddr();
            if tf.prmd & 0b11 == 0b11 && fault_addr < 0x1000 {
                return TrapType::LoadPageFault(fault_addr);
            }
            // error!("address not aligned: {:#x?}", tf);
            unsafe { emulate_load_store_insn(tf) }
        }
        Trap::Exception(Exception::MemoryAccessAddressError) => {
            let badv = badv::read().vaddr();
            panic!(
                "Unhandled trap {:?} @ {:#x} BADV: {:#x}:\n{:#x?}",
                estat.cause(),
                tf.era,
                badv,
                tf
            );
        }
        Trap::Interrupt(_) => {
            let irq_num: usize = estat.is().trailing_zeros() as usize;
            match irq_num {
                // TIMER_IRQ
                TIMER_IRQ => {
                    ticlr::clear_timer_interrupt();
                    TrapType::Timer
                }
                12 => {
                    polyhal::multicore::record_trap_stage(2);
                    if from_user {
                        // Unlike ordinary user traps, this fast path does not
                        // enter the OS callback. Withdraw the user-active bit
                        // before returning to the kernel task loop; otherwise
                        // another shootdown could wait for this CPU while it
                        // is already taking kernel locks with IRQs masked.
                        polyhal::multicore::mark_current_cpu_kernel_entry();
                    }
                    let reschedule = polyhal::multicore::handle_ipi();
                    if from_user && reschedule {
                        TrapType::Reschedule
                    } else {
                        // This lock-free IPI can arrive while a no-IRQ kernel lock
                        // is held. It is fully handled here and must not re-enter
                        // the OS trap/scheduler path.
                        polyhal::multicore::record_trap_stage(4);
                        return TrapType::Handled;
                    }
                }
                _ => panic!("unknown interrupt: {}", irq_num),
            }
        }
        Trap::Exception(Exception::Syscall) => TrapType::SysCall,
        Trap::Exception(Exception::FetchInstructionAddressError) => {
            TrapType::InstructionPageFault(badv::read().vaddr())
        }
        Trap::Exception(Exception::StorePageFault)
        | Trap::Exception(Exception::PageModifyFault) => {
            TrapType::StorePageFault(badv::read().vaddr())
        }
        Trap::Exception(Exception::PageNonExecutableFault)
        | Trap::Exception(Exception::FetchPageFault) => {
            TrapType::InstructionPageFault(badv::read().vaddr())
        }
        // Load Fault
        Trap::Exception(Exception::LoadPageFault)
        | Trap::Exception(Exception::PageNonReadableFault)
        | Trap::Exception(Exception::PagePrivilegeIllegal) => {
            TrapType::LoadPageFault(badv::read().vaddr())
        }
        Trap::Exception(Exception::InstructionNotExist) => TrapType::IllegalInstruction(tf.era),
        Trap::Exception(Exception::InstructionPrivilegeIllegal)
        | Trap::Exception(Exception::BoundsCheckFault) => TrapType::IllegalInstruction(tf.era),
        Trap::Exception(Exception::FloatingPointUnavailable) => {
            // EUEN is per-CPU state. A task that reaches a CPU whose FPU bit
            // was cleared must retry the same instruction after enabling it;
            // treating this lazy-enable trap as SIGILL would be incorrect.
            euen::set_fpe(true);
            TrapType::Handled
        }
        Trap::MachineError(error) => {
            panic!(
                "LoongArch machine error {:?}: estat={:#x} ecode={:#x} esubcode={:#x} era={:#x} badv={:#x}\n{:#x?}",
                error,
                estat.raw(),
                estat.ecode(),
                estat.esubcode(),
                tf.era,
                badv::read().vaddr(),
                tf,
            );
        }
        Trap::Unknown => match estat.ecode() {
            0 if estat.is() == 0 => {
                // A level-triggered source may be withdrawn between vectoring
                // and ESTAT sampling. With no pending IS bit there is nothing
                // to acknowledge; resume the interrupted context.
                log::error!(
                    "[LA64_SPURIOUS_TRAP] estat={:#x} era={:#x} badv={:#x}",
                    estat.raw(),
                    tf.era,
                    badv::read().vaddr(),
                );
                TrapType::Handled
            }
            // The loongArch crate currently stops decoding at ECODE 0xf.
            // Linux-visible ECODE 0x10/0x11 are unavailable LSX/LASX
            // instructions; Kairix does not advertise or context-switch those
            // extensions, so user space receives SIGILL.
            0x10 | 0x11 => TrapType::IllegalInstruction(tf.era),
            // ECODE 0x12 is a floating-point arithmetic exception and must be
            // delivered as SIGFPE rather than crashing the kernel.
            0x12 => TrapType::FloatingPointException(tf.era),
            _ => {
                panic!(
                    "Unknown LoongArch trap: estat={:#x} is={:#x} ecode={:#x} esubcode={:#x} era={:#x} badv={:#x}\n{:#x?}",
                    estat.raw(),
                    estat.is(),
                    estat.ecode(),
                    estat.esubcode(),
                    tf.era,
                    badv::read().vaddr(),
                    tf,
                );
            }
        },
        _ => {
            // error!(
            //     "Unhandled trap {:?} @ {:#x} BADV: {:#x}:\n{:#x?}",
            //     estat.cause(),
            //     tf.era,
            //     badv::read().vaddr(),
            //     tf
            // );
            // loop{}
            panic!(
                "Unhandled trap {:?} @ {:#x} BADV: {:#x}:\n{:#x?}",
                estat.cause(),
                tf.era,
                badv::read().vaddr(),
                tf
            );
        }
    };
    // info!("return to addr: {:#x}", tf.era);
    polyhal::multicore::record_trap_stage(3);
    unsafe { super::_interrupt_for_arch(tf, trap_type, 0) };
    polyhal::multicore::record_trap_stage(4);
    trap_type
}
