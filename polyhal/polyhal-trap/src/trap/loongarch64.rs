#[macro_use]
mod macros;
use log::error;
mod unaligned;
use super::{EscapeReason, TrapType};
use crate::trapframe::TrapFrame;
use core::arch::naked_asm;
use loongArch64::register::estat::{self, Exception, Trap};
use loongArch64::register::{
    badv, ecfg, eentry, prmd, pwch, pwcl, stlbps, ticlr, tlbidx, tlbrehi, tlbrentry,
};
use polyhal::irq::TIMER_IRQ;
use polyhal::println;
use unaligned::emulate_load_store_insn;

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
    );
}

#[naked]
#[no_mangle]
pub extern "C" fn user_restore(context: *mut TrapFrame) {
    unsafe {
        naked_asm!(
            includes_trap_macros!(),
            r"
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

                LOAD_REGS

                ertn
            ",
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
        trapframe_size = const crate::trapframe::TRAPFRAME_SIZE,
        user_vec = sym user_vec,
        trap_handler = sym loongarch64_trap_handler,
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
}

fn loongarch64_trap_handler(tf: &mut TrapFrame) -> TrapType {
    let estat = estat::read();
    let trap_type = match estat.cause() {
        Trap::Exception(Exception::Breakpoint) => {
            tf.era += 4;
            TrapType::Breakpoint
        }
        Trap::Exception(Exception::AddressNotAligned) => {
            // The emulator updates the target register and advances ERA. This
            // exception has already been handled, so do not forward it to the
            // OS-level trap handler as an unknown kernel/user trap.
            unsafe { emulate_load_store_insn(tf) }
            return TrapType::Unknown;
        }
        Trap::Interrupt(_) => {
            let irq_num: usize = estat.is().trailing_zeros() as usize;
            match irq_num {
                // TIMER_IRQ
                TIMER_IRQ => {
                    ticlr::clear_timer_interrupt();
                    TrapType::Timer
                }
                _ => panic!("unknown interrupt: {}", irq_num),
            }
        }
        Trap::Exception(Exception::Syscall) => TrapType::SysCall,
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
        | Trap::Exception(Exception::PageNonReadableFault) => {
            TrapType::LoadPageFault(badv::read().vaddr())
        }
        Trap::Exception(Exception::InstructionNotExist) => TrapType::IllegalInstruction(tf.era),
        Trap::MachineError(_) => todo!(),
        Trap::Unknown => todo!(),
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
    unsafe { super::_interrupt_for_arch(tf, trap_type, 0) };
    trap_type
}
