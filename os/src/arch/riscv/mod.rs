use crate::mm::address::*;
use core::arch::asm;
#[allow(missing_docs)]
pub mod entry;

pub fn sfence_vma_va(va: VirtAddr){
    unsafe {
        asm!(
            "sfence.vma {}, x0", 
            in(reg) usize::from(va), 
            options(nostack)
        );
    }
}
use crate::config::{KERNEL_STACK_SIZE, MAX_CPU_NUM};
use core::arch::global_asm;

unsafe extern "Rust" {
    pub(crate) unsafe fn _main_for_arch(id: usize, first: bool) -> bool;
}

/// Boot Stack Size.
/// TODO: reduce the boot stack size. Map stack in boot step.
pub const BOOT_STACK_SIZE: usize = KERNEL_STACK_SIZE;

#[unsafe(link_section = ".bss.stack")]
#[allow(unused)]
pub(crate) static mut BOOT_STACK: [u8; MAX_CPU_NUM * BOOT_STACK_SIZE] =
    [0; MAX_CPU_NUM * BOOT_STACK_SIZE];

#[macro_export]
macro_rules! define_entry {
    ($main_fn: ident) => {
        #[unsafe(export_name = "_main_for_arch")]
        fn defined_main(id: usize, first: bool) -> bool {
            $main_fn(id, first)
        }
    };
}
