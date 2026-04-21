use core::arch::asm;
use polyhal::utils::addr::*;
#[allow(missing_docs)]
pub mod entry;

<<<<<<< HEAD:os/src/arch/loongarch_dir/mod.rs
=======
pub fn sfence_vma_va(va: VirtAddr) {
    unsafe {
        asm!(
            "sfence.vma {}, x0",
            in(reg) usize::from(va),
            options(nostack)
        );
    }
}
>>>>>>> busybox-fix:os/src/arch/riscv/mod.rs
use crate::config::{KERNEL_STACK_SIZE, MAX_CPU_NUM};
use core::arch::global_asm;

unsafe extern "Rust" {
    pub(crate) unsafe fn _main_for_arch(id: usize, first: bool) -> bool;
}

pub const BOOT_STACK_SIZE: usize = KERNEL_STACK_SIZE;

#[unsafe(link_section = ".bss.stack")]
#[allow(unused)]
pub(crate) static mut BOOT_STACK: [u8; MAX_CPU_NUM * BOOT_STACK_SIZE] =
    [0; MAX_CPU_NUM * BOOT_STACK_SIZE];
