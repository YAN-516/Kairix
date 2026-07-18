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

pub fn enable_tlb_shootdown_ipi() {
    unsafe { sie::set_ssoft() };
}

pub fn acknowledge_tlb_shootdown_ipi() -> bool {
    unsafe { sip::clear_ssoft() };
    true
}

pub fn send_tlb_shootdown_ipi(cpu: usize) -> bool {
    sbi_rt::send_ipi(1, cpu).is_ok()
}

pub fn wait_for_tlb_shootdown(generation: usize, target_mask: usize) {
    let old_sie = sie::read();
    let old_global_ie = sstatus::read().sie();
    unsafe {
        sie::clear_stimer();
        sie::clear_sext();
        sie::set_ssoft();
        sstatus::set_sie();
    }
    while !super::tlb_shootdown_acks_reached(generation, target_mask) {
        core::hint::spin_loop();
    }
    unsafe {
        sstatus::clear_sie();
        if !old_sie.ssoft() {
            sie::clear_ssoft();
        }
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
