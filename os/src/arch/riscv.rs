use super::TLB;
use core::arch::riscv64::sfence_vma;
// use crate::mm::address::*;
use polyhal::utils::addr::*;

impl TLB {
    /// flush the TLB entry by VirtualAddress
    /// just use it directly
    ///
    /// TLB::flush_vaddr(arg0); // arg0 is the virtual address(VirtAddr)
    #[inline]
    pub fn flush_vaddr(vaddr: VirtAddr) {
        unsafe {
            sfence_vma(vaddr.into(), 0);
        }
    }

    /// flush all tlb entry
    ///
    /// how to use ?
    /// just
    /// TLB::flush_all();
    #[inline]
    pub fn flush_all() {
        unsafe {
            riscv::asm::sfence_vma_all();
        }
    }
}
