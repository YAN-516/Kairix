#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

#[cfg(target_arch = "loongarch64")]
use core::arch::asm;

const AT_NULL: usize = 0;
const AT_HWCAP: usize = 16;

fn find_auxv(argc: usize, argv: *const usize, wanted: usize) -> Option<usize> {
    // Initial stack: argc, argv[], NULL, envp[], NULL, auxv pairs.
    let mut cursor = unsafe { argv.add(argc + 1) };
    while unsafe { *cursor } != 0 {
        cursor = unsafe { cursor.add(1) };
    }
    cursor = unsafe { cursor.add(1) };

    loop {
        let key = unsafe { *cursor };
        let value = unsafe { *cursor.add(1) };
        if key == AT_NULL {
            return None;
        }
        if key == wanted {
            return Some(value);
        }
        cursor = unsafe { cursor.add(2) };
    }
}

#[cfg(target_arch = "loongarch64")]
fn validate_arch_hwcap(hwcap: usize) -> bool {
    const HWCAP_LOONGARCH_CPUCFG: usize = 1 << 0;
    const HWCAP_LOONGARCH_UAL: usize = 1 << 2;
    const CPUCFG1_UAL: usize = 1 << 20;

    let cpucfg1: usize;
    unsafe {
        asm!("cpucfg {}, {}", out(reg) cpucfg1, in(reg) 1usize);
    }
    let cpu_has_ual = cpucfg1 & CPUCFG1_UAL != 0;
    let hwcap_has_ual = hwcap & HWCAP_LOONGARCH_UAL != 0;
    println!(
        "[auxv_hwcap_test] hwcap={:#x} cpucfg1={:#x} cpu_ual={} hwcap_ual={}",
        hwcap, cpucfg1, cpu_has_ual, hwcap_has_ual
    );

    hwcap & HWCAP_LOONGARCH_CPUCFG != 0 && hwcap_has_ual == cpu_has_ual
}

#[cfg(target_arch = "riscv64")]
fn validate_arch_hwcap(hwcap: usize) -> bool {
    const fn isa(extension: u8) -> usize {
        1usize << (extension - b'A')
    }
    let rv64gc = isa(b'I') | isa(b'M') | isa(b'A') | isa(b'F') | isa(b'D') | isa(b'C');
    println!(
        "[auxv_hwcap_test] hwcap={:#x} expected_rv64gc={:#x}",
        hwcap, rv64gc
    );
    hwcap & rv64gc == rv64gc
}

#[cfg(not(any(target_arch = "loongarch64", target_arch = "riscv64")))]
fn validate_arch_hwcap(_hwcap: usize) -> bool {
    true
}

#[unsafe(no_mangle)]
pub fn main_with_args(argc: usize, argv: *const usize) -> i32 {
    println!("[auxv_hwcap_test] start");
    let Some(hwcap) = find_auxv(argc, argv, AT_HWCAP) else {
        println!("[auxv_hwcap_test] FAIL: AT_HWCAP missing");
        return 1;
    };

    if validate_arch_hwcap(hwcap) {
        println!("[auxv_hwcap_test] PASS");
        0
    } else {
        println!("[auxv_hwcap_test] FAIL: AT_HWCAP does not match the CPU");
        1
    }
}
