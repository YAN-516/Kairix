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
    sbi_rt::send_ipi(1, cpu).is_ok()
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
                let _ = super::send_tlb_shootdown_ipi(cpu);
                retry &= !bit;
            }
            spins = 0;
        }
        core::hint::spin_loop();
    }
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
