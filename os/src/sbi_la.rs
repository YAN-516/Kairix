
#[cfg(target_arch = "loongarch64")]
const KERNEL_ENTRY_PA: usize = 0x8000_0000;

pub fn get_tp() -> usize {
    let tp: usize;
    unsafe { core::arch::asm!("move {}, $tp", out(reg) tp); }
    tp
}

#[inline]
pub fn shutdown() -> ! {
    let ged_addr = PhysAddr(0x100E001C);
    log::info!("Shutting down...");
    unsafe { ged_addr.get_mut_ptr::<u8>().write_volatile(0x34) };
    unsafe { loongArch64::asm::idle() };
    log::warn!("It should shutdown!");
    unreachable!()
}