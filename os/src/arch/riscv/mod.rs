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