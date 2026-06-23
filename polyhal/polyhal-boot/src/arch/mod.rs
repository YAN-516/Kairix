use core::{
    hint::spin_loop,
    sync::atomic::{AtomicBool, Ordering},
};

use polyhal::{
    common::get_cpu_num, consts::VIRT_ADDR_START, ctor::{CtorType, ph_init_iter}, println
};

#[cfg(target_arch = "loongarch64")]
fn early_uart_puts(s: &str) {
    const UART_BASE: usize = 0x8000_0000_1fe2_0000;

    for byte in s.bytes() {
        if byte == b'\n' {
            early_uart_put_byte(b'\r');
        }
        early_uart_put_byte(byte);
    }

    fn early_uart_put_byte(byte: u8) {
        const UART_BASE: usize = 0x8000_0000_1fe2_0000;
        let thr = UART_BASE as *mut u8;
        let lsr = (UART_BASE + 5) as *const u8;

        for _ in 0..10_000 {
            if unsafe { lsr.read_volatile() } & 0x20 != 0 {
                break;
            }
        }
        unsafe {
            thr.write_volatile(byte);
        }
    }
}

// Define multi-architecture modules and pub use them.
cfg_if::cfg_if! {
    if #[cfg(target_arch = "loongarch64")] {
        mod loongarch64;
    } else if #[cfg(target_arch = "aarch64")] {
        mod aarch64;
    } else if #[cfg(target_arch = "riscv64")] {
        mod riscv64;
    } else if #[cfg(target_arch = "x86_64")] {
        mod x86_64;
    } else {
        compile_error!("unsupported architecture!");
    }
}

/// Clear the bss section
pub(crate) fn clear_bss() {
    extern "C" {
        fn _sbss();
        fn _ebss();
    }
    unsafe {
        core::slice::from_raw_parts_mut(
            _sbss as usize as *mut u128,
            (_ebss as usize - _sbss as usize) / size_of::<u128>(),
        )
        .fill(0);
    }
}

fn call_real_main(hartid: usize) {
    #[cfg(target_arch = "loongarch64")]
    early_uart_puts("Kairix: call_real_main enter\n");

    // polyhal::multicore::boot_core(cpuid, addr, sp_top);
    static IS_BOOT: AtomicBool = AtomicBool::new(true);
    static INIT_DONE: AtomicBool = AtomicBool::new(false);
    extern "Rust" {
        fn _secondary_start();
        pub(crate) fn _main_for_arch(hartid: usize);
        pub(crate) fn _secondary_for_arch(hartid: usize);
    }

    if IS_BOOT.swap(false, Ordering::SeqCst) {
        #[cfg(target_arch = "loongarch64")]
        early_uart_puts("Kairix: call_real_main boot cpu\n");

        const SP_SIZE: usize = 0x40_0000;

        (0..get_cpu_num()).for_each(|x| unsafe {
            if x == hartid {
                return;
            }
            let stack_top = polyhal::mem::alloc(SP_SIZE).add(SP_SIZE);
            println!("Boot Core: {}   {:#p}", x, stack_top);
            polyhal::multicore::boot_core(x, _secondary_start as usize, stack_top as usize + VIRT_ADDR_START);
        });
        polyhal::println!();
        #[cfg(target_arch = "loongarch64")]
        early_uart_puts("Kairix: call_real_main before ctors\n");

        // Run Kernel's Contructors Before Droping Into Kernel.
        ph_init_iter(CtorType::KernelService).for_each(|x| (x.func)());
        #[cfg(target_arch = "loongarch64")]
        early_uart_puts("Kairix: call_real_main kernel service ctors done\n");
        ph_init_iter(CtorType::Normal).for_each(|x| (x.func)());
        #[cfg(target_arch = "loongarch64")]
        early_uart_puts("Kairix: call_real_main normal ctors done\n");
        INIT_DONE.store(true, Ordering::SeqCst);
        // Declare the _main_for_arch exists.
        unsafe {
            #[cfg(target_arch = "loongarch64")]
            early_uart_puts("Kairix: call _main_for_arch\n");
            _main_for_arch(hartid);
        }
    } else {
        while !INIT_DONE.load(Ordering::SeqCst) {
            spin_loop();
        }
        unsafe {
            _secondary_for_arch(hartid);
        }
    }
    loop {
        spin_loop();
    }
}
