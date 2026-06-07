// use loongarch64::asm::idle;
use polyhal::arch::hart_id;
use polyhal::utils::addr::*;
#[cfg(target_arch = "loongarch64")]
const _KERNEL_ENTRY_PA: usize = 0x8000_0000;
///
pub fn get_tp() -> usize {
    hart_id()
}
///
pub fn set_tp(id: usize) {
    unsafe {
        core::arch::asm!(
            "move $tp, {}",
            in(reg) id,
            options(nomem, nostack),
        );
    }
}

// #[inline]
// pub fn shutdown(_failure: bool) -> ! {
//     let ged_addr = PhysAddr(0x100E001C);
//     log::info!("Shutting down...");
//     unsafe { ged_addr.get_mut_ptr::<u8>().write_volatile(0x34) };
//     unsafe { loongarch64::asm::idle() };
//     log::warn!("It should shutdown!");
//     unreachable!()
// }
