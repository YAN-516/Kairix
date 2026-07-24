use crate::consts::VIRT_ADDR_START;
use riscv::register::{sie, sip, sstatus};

// Boot a core with top pointer of the stack
pub fn boot_core(cpuid: usize, addr: usize, sp_top: usize) {
    // PERCPU DATA ADDRESS RANGE END
    let aux_core_func = addr & !VIRT_ADDR_START;

    log::info!("secondary addr: {:#x}", addr);
    let ret = sbi_rt::hart_start(cpuid, aux_core_func, sp_top);
    match ret.is_ok() {
        true => log::info!("hart {} Startting successfully", cpuid),
        false => log::warn!("hart {} Startting failed", cpuid),
    }
}

pub fn enable_ipi() {
    unsafe { sie::set_ssoft() };
}

pub fn acknowledge_ipi() {
    unsafe { sip::clear_ssoft() };
}

pub fn send_ipi(cpu: usize) -> bool {
    if cpu >= usize::BITS as usize {
        return false;
    }
    send_ipi_mask(1usize << cpu)
}

/// Submit one SBI IPI request for the complete hart mask.
///
/// Only SSIP is admitted around the firmware call. This closes the window in
/// which two concurrent shootdown initiators enter SBI with supervisor
/// interrupts masked and each becomes the other's target.
pub fn send_ipi_mask(mask: usize) -> bool {
    if mask == 0 {
        return true;
    }
    let old_sie = sie::read();
    let old_global_ie = sstatus::read().sie();
    unsafe {
        sie::clear_stimer();
        sie::clear_sext();
        sie::set_ssoft();
        sstatus::set_sie();
    }
    let sent = sbi_rt::send_ipi(mask, 0).is_ok();
    unsafe {
        sstatus::clear_sie();
        sie::set_ssoft();
        if old_sie.stimer() {
            sie::set_stimer();
        }
        if old_sie.sext() {
            sie::set_sext();
        }
        if old_global_ie {
            sstatus::set_sie();
        }
    }
    sent
}

pub fn wait_for_tlb_shootdown(generation: usize, target_mask: usize) {
    const RESEND_SPINS: usize = 1 << 20;
    let old_sie = sie::read();
    let old_global_ie = sstatus::read().sie();
    unsafe {
        sie::clear_stimer();
        sie::clear_sext();
        sie::set_ssoft();
        sstatus::set_sie();
    }
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
    unsafe {
        sstatus::clear_sie();
        // SSIE is a permanent per-hart capability of this kernel, not a
        // temporary mask owned by the shootdown wait. Restoring a previously
        // clear SSIE bit here can permanently disable future TLB IPIs. A later
        // shootdown would then wait forever for this hart's acknowledgement
        // while its sender also stops servicing timer interrupts.
        sie::set_ssoft();
        if old_sie.stimer() {
            sie::set_stimer();
        }
        if old_sie.sext() {
            sie::set_sext();
        }
        if old_global_ie {
            sstatus::set_sie();
        }
    }
}

pub fn wait_for_memory_barrier(generation: usize, target_mask: usize) {
    const RESEND_SPINS: usize = 1 << 20;
    let old_sie = sie::read();
    let old_global_ie = sstatus::read().sie();
    unsafe {
        sie::clear_stimer();
        sie::clear_sext();
        sie::set_ssoft();
        sstatus::set_sie();
    }
    let mut spins = 0usize;
    loop {
        super::service_local_memory_barrier_generation();
        let pending = super::memory_barrier_pending_mask(generation, target_mask);
        if pending == 0 {
            break;
        }
        spins += 1;
        if spins == RESEND_SPINS {
            let _ = super::send_memory_barrier_ipi_mask(pending);
            spins = 0;
        }
        core::hint::spin_loop();
    }
    unsafe {
        sstatus::clear_sie();
        sie::set_ssoft();
        if old_sie.stimer() {
            sie::set_stimer();
        }
        if old_sie.sext() {
            sie::set_sext();
        }
        if old_global_ie {
            sstatus::set_sie();
        }
    }
}
