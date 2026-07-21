use core::arch::asm;
use loongArch64::consts::{
    LOONGARCH_IOCSR_IPI_CLEAR, LOONGARCH_IOCSR_IPI_EN, LOONGARCH_IOCSR_IPI_STATUS,
};
use loongArch64::ipi::{csr_mail_send, send_ipi_single};
use loongArch64::register::crmd;
use loongArch64::register::ecfg::{self, LineBasedInterrupt};

const TLB_SHOOTDOWN_ACTION: u32 = 1 << 1;

#[inline]
fn iocsr_read_u32(addr: usize) -> u32 {
    let value: u32;
    unsafe {
        asm!("iocsrrd.w {}, {}", out(reg) value, in(reg) addr);
    }
    value
}

#[inline]
fn iocsr_write_u32(addr: usize, value: u32) {
    unsafe {
        asm!("iocsrwr.w {}, {}", in(reg) value, in(reg) addr);
    }
}

// TODO: Boot a core with top pointer of the stack
pub fn boot_core(hart_id: usize, addr: usize, sp_top: usize) {
    csr_mail_send(addr as _, hart_id, 0);
    csr_mail_send(sp_top as _, hart_id, 1);
    send_ipi_single(hart_id, 1);
}

pub fn enable_tlb_shootdown_ipi() {
    let enabled = iocsr_read_u32(LOONGARCH_IOCSR_IPI_EN);
    iocsr_write_u32(LOONGARCH_IOCSR_IPI_EN, enabled | TLB_SHOOTDOWN_ACTION);
    let enabled = ecfg::read().lie();
    ecfg::set_lie(enabled | LineBasedInterrupt::IPI);
}

pub fn acknowledge_tlb_shootdown_ipi() -> bool {
    let pending = iocsr_read_u32(LOONGARCH_IOCSR_IPI_STATUS);
    let shootdown = pending & TLB_SHOOTDOWN_ACTION != 0;
    if shootdown {
        // Action bit 0 is also used by secondary-CPU startup. A TLB IPI must
        // acknowledge only its own action instead of consuming unrelated IPI
        // reasons that another subsystem may still need to observe.
        iocsr_write_u32(LOONGARCH_IOCSR_IPI_CLEAR, TLB_SHOOTDOWN_ACTION);
    }
    shootdown
}

pub fn send_tlb_shootdown_ipi(cpu: usize) -> bool {
    send_ipi_single(cpu, TLB_SHOOTDOWN_ACTION);
    true
}

pub fn wait_for_tlb_shootdown(generation: usize, target_mask: usize) {
    const RESEND_SPINS: usize = 1 << 20;
    let old_lie = ecfg::read().lie();
    let old_ie = crmd::read().ie();

    // Kernel critical sections normally mask interrupts. Permit only the IPI
    // line while waiting so concurrent shootdowns can acknowledge one another
    // without admitting timer/external interrupt reentrancy.
    ecfg::set_lie(LineBasedInterrupt::IPI);
    crmd::set_ie(true);
    let mut spins = 0usize;
    loop {
        let pending = super::tlb_shootdown_pending_mask(generation, target_mask);
        if pending == 0 {
            break;
        }
        spins += 1;
        if spins == RESEND_SPINS {
            let mut retry = pending;
            while retry != 0 {
                let cpu = retry.trailing_zeros() as usize;
                let bit = 1usize << cpu;
                let _ = send_tlb_shootdown_ipi(cpu);
                retry &= !bit;
            }
            spins = 0;
        }
        core::hint::spin_loop();
    }
    crmd::set_ie(false);
    // The TLB IPI line is a permanent per-CPU capability. Restoring a stale
    // mask without it would make a later synchronous shootdown wait forever.
    ecfg::set_lie(old_lie | LineBasedInterrupt::IPI);
    crmd::set_ie(old_ie);
}
