include!("riscv64/shutdown.rs");

/// Riscv64 ebreak instruction.
#[inline]
pub fn ebreak() {
    unsafe {
        riscv::asm::ebreak();
    }
}

#[inline]
pub fn hlt() {
    unsafe {
        riscv::register::sstatus::clear_sie();
        riscv::asm::wfi();
        riscv::register::sstatus::set_sie();
    }
}

/// Wait until an interrupt becomes pending. The caller owns interrupt masks.
#[inline]
pub fn wait_for_interrupt() {
    riscv::asm::wfi()
}

/// Make prior data writes visible to subsequent instruction fetches on this hart.
#[inline]
pub fn synchronize_instruction_cache() {
    riscv::asm::fence_i()
}
