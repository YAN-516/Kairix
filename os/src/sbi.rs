//! SBI call wrappers
#![allow(unused)]
use core::arch::asm;
#[cfg(target_arch = "riscv64")]
const KERNEL_ENTRY_PA: usize = 0x8020_0000;


/// use sbi call to putchar in console (qemu uart handler)
pub fn console_putchar(c: usize) {
    #[allow(deprecated)]
    sbi_rt::legacy::console_putchar(c);
}

/// use sbi call to getchar from console (qemu uart handler)
pub fn console_getchar() -> usize {
    #[allow(deprecated)]
    sbi_rt::legacy::console_getchar()
}

/// use sbi call to set timer
pub fn set_timer(timer: usize) {
    sbi_rt::set_timer(timer as _);
}

/// use sbi call to shutdown the kernel
pub fn shutdown(failure: bool) -> ! {
    use sbi_rt::{NoReason, Shutdown, SystemFailure, system_reset};
    if !failure {
        system_reset(Shutdown, NoReason);
    } else {
        system_reset(Shutdown, SystemFailure);
    }
    unreachable!()
}
#[allow(unused)]
#[allow(missing_docs)]
pub fn hart_start(hartid: usize, opaque: usize) {
    sbi_rt::hart_start(hartid, KERNEL_ENTRY_PA, opaque);
}

#[inline(always)]
#[allow(unused)]
#[allow(missing_docs)]
pub fn set_tp(hartid: usize) {
    unsafe {
        asm!(
           "mv tp, {}",
           in(reg) hartid,
        )
    }
}

#[inline(always)]
#[allow(missing_docs)]
pub fn get_tp() -> usize {
    let tp: usize;
    unsafe {
        asm!(
            "mv {}, tp",
            out(reg) tp,
        );
    }
    tp
}

#[inline(always)]
#[allow(missing_docs)]
pub fn get_sp() -> usize {
    let sp: usize;
    unsafe {
        asm!(
            "mv {}, sp",
            out(reg) sp,
        );
    }
    sp
}
