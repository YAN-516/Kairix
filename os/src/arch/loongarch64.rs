//! src/arch/loongarch64/entry.rs
//! 龙芯架构启动代码 - 完全内联版本（无中断入口）

use core::arch::global_asm;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use polyhal::utils::addr::*;
use polyhal::arch::consts::*;
use super::TLB;

use core::arch::global_asm;

use core::arch::global_asm;

#[naked]
#[no_mangle]
#[link_section = ".text.entry"]
pub unsafe extern "C" fn _start() -> ! {
    unsafe {
        naked_asm!(
            "la.local $sp, boot_stack_top",
            "la.local $t0, rust_main",
            "jirl $ra, $t0, 0",
            options(noreturn)
        );
    }
}

#[link_section = ".bss.stack"]
static mut BOOT_STACK: [u8; 65536] = [0; 65536];

#[no_mangle]
pub extern "C" fn rust_main() -> ! {
    loop {}
}


impl TLB {
    #[inline]
    pub fn flush_vaddr(vaddr: VirtAddr) {
        unsafe {
            core::arch::asm!("dbar 0; invtlb 0x05, $r0, {reg}", reg = in(reg) vaddr.0);
        }
    }

    #[inline]
    pub fn flush_all() {
        unsafe {
            core::arch::asm!("dbar 0; invtlb 0x00, $r0, $r0");
        }
    }
}