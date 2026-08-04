//! src/arch/loongarch64/entry.rs
//! 龙芯架构启动代码 - 完全内联版本（无中断入口）

use super::TLB;
use core::arch::global_asm;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use loongArch64::register::asid;
use polyhal::arch::consts::*;
use polyhal::utils::addr::*;

impl TLB {
    #[inline]
    pub fn flush_vaddr(vaddr: VirtAddr) {
        // LoongArch base-page TLB entries cover an even/odd 4 KiB page pair.
        // INVTLB address matching must use the pair's 8 KiB-aligned VPPN.
        let pair_addr = vaddr.0 & !((PAGE_SIZE << 1) - 1);
        // INVTLB op 0x05 matches both VPPN and ASID.  Do not assume that
        // firmware left the active ASID at zero.
        let current_asid = asid::read().asid();
        unsafe {
            core::arch::asm!(
                "dbar 0; invtlb 0x05, {asid}, {vaddr}",
                asid = in(reg) current_asid,
                vaddr = in(reg) pair_addr,
            );
        }
    }

    #[inline]
    pub fn flush_all() {
        unsafe {
            core::arch::asm!("dbar 0; invtlb 0x00, $r0, $r0");
        }
    }
}
