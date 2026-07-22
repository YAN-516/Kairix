use core::arch::asm;
use loongArch64::consts::{
    LOONGARCH_IOCSR_IPI_CLEAR, LOONGARCH_IOCSR_IPI_EN, LOONGARCH_IOCSR_IPI_STATUS,
};
use loongArch64::ipi::{csr_mail_send, send_ipi_single};
use loongArch64::register::crmd;
use loongArch64::register::ecfg::{self, LineBasedInterrupt};

const KERNEL_IPI_ACTION: u32 = 1 << 1;

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

pub fn enable_ipi() {
    let enabled = iocsr_read_u32(LOONGARCH_IOCSR_IPI_EN);
    iocsr_write_u32(LOONGARCH_IOCSR_IPI_EN, enabled | KERNEL_IPI_ACTION);
    let enabled = ecfg::read().lie();
    ecfg::set_lie(enabled | LineBasedInterrupt::IPI);
}

pub fn acknowledge_ipi() {
    let pending = iocsr_read_u32(LOONGARCH_IOCSR_IPI_STATUS);
    if pending & KERNEL_IPI_ACTION != 0 {
        // Action bit 0 is also used by secondary-CPU startup. A TLB IPI must
        // acknowledge only its own action instead of consuming unrelated IPI
        // reasons that another subsystem may still need to observe.
        iocsr_write_u32(LOONGARCH_IOCSR_IPI_CLEAR, KERNEL_IPI_ACTION);
    }
}

pub fn send_ipi(cpu: usize) -> bool {
    send_ipi_single(cpu, KERNEL_IPI_ACTION);
    true
}

pub fn send_ipi_mask(mut mask: usize) -> bool {
    while mask != 0 {
        let cpu = mask.trailing_zeros() as usize;
        let bit = 1usize << cpu;
        send_ipi_single(cpu, KERNEL_IPI_ACTION);
        mask &= !bit;
    }
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
    super::begin_tlb_shootdown_wait(generation, target_mask);
    let mut spins = 0usize;
    loop {
        super::service_local_tlb_shootdown_generation();
        let pending = super::tlb_shootdown_pending_mask(generation, target_mask);
        super::update_tlb_shootdown_wait(pending);
        if pending == 0 {
            break;
        }
        spins += 1;
        if spins == RESEND_SPINS {
            let _ = super::send_tlb_shootdown_ipi_mask(pending);
            spins = 0;
        }
        core::hint::spin_loop();
    }
    super::end_tlb_shootdown_wait();
    crmd::set_ie(false);
    // The TLB IPI line is a permanent per-CPU capability. Restoring a stale
    // mask without it would make a later synchronous shootdown wait forever.
    ecfg::set_lie(old_lie | LineBasedInterrupt::IPI);
    crmd::set_ie(old_ie);
}
