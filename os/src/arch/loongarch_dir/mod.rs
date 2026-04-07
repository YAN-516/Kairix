use core::arch::asm;
use polyhal::utils::addr::*;
#[allow(missing_docs)]
pub mod entry;

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
