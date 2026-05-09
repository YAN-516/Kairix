//! The panic handler
// #[cfg(target_arch = "riscv64")]
// use crate::sbi::shutdown;
// #[cfg(target_arch = "loongarch64")]
// use crate::sbi_la::shutdown;
use polyhal::instruction::shutdown;
use core::panic::PanicInfo;
use log::*;
use polyhal::println;
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    if let Some(location) = info.location() {
        println!(
            "[kernel] Panicked at {}:{} {}",
            location.file(),
            location.line(),
            info.message()
        );
    } else {
        println!("[kernel] Panicked: {}", info.message());
    }
    shutdown()
}
