use core::{
    hint::spin_loop,
    sync::atomic::{AtomicBool, Ordering},
};

use polyhal::percpu::reserve_local_thread_pointer;
use polyhal::{
    common::get_cpu_num,
    consts::{MAX_CPU_NUM, VIRT_ADDR_START},
    ctor::{CtorType, ph_init_iter},
    println,
};

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
    // polyhal::multicore::boot_core(cpuid, addr, sp_top);
    static IS_BOOT: AtomicBool = AtomicBool::new(true);
    static INIT_DONE: AtomicBool = AtomicBool::new(false);
    extern "Rust" {
        fn _secondary_start();
        pub(crate) fn _main_for_arch(hartid: usize);
        pub(crate) fn _secondary_for_arch(hartid: usize);
    }

    if IS_BOOT.swap(false, Ordering::SeqCst) {
        const SP_SIZE: usize = 0x40_0000;

        let detected_cpu_num = get_cpu_num();
        let boot_cpu_num = detected_cpu_num.min(MAX_CPU_NUM);
        if detected_cpu_num > MAX_CPU_NUM {
            println!(
                "Detected {} CPUs, limiting boot to supported maximum {}",
                detected_cpu_num, MAX_CPU_NUM
            );
        }

        // Reserve every per-CPU area before starting a secondary. Otherwise a
        // secondary can enter set_local_thread_pointer() while this CPU is
        // still allocating boot stacks from the same early MEM_AREA array.
        for cpu_id in 0..boot_cpu_num {
            reserve_local_thread_pointer(cpu_id);
        }

        (0..boot_cpu_num).for_each(|x| unsafe {
            if x == hartid {
                return;
            }
            let stack_top = polyhal::mem::alloc(SP_SIZE).add(SP_SIZE);
            println!("Boot Core: {}   {:#p}", x, stack_top);
            polyhal::multicore::boot_core(
                x,
                _secondary_start as usize,
                stack_top as usize + VIRT_ADDR_START,
            );
        });
        polyhal::println!();

        // Run Kernel's Contructors Before Droping Into Kernel.
        ph_init_iter(CtorType::KernelService).for_each(|x| (x.func)());
        ph_init_iter(CtorType::Normal).for_each(|x| (x.func)());
        INIT_DONE.store(true, Ordering::SeqCst);
        // Declare the _main_for_arch exists.
        unsafe {
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
