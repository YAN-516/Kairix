//! src/arch/loongarch64/entry.rs
//! 龙芯架构启动代码 - 完全内联版本（无中断入口）

use core::arch::global_asm;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use polyhal::utils::addr::*;
use polyhal::arch::consts::*;
use super::TLB;



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