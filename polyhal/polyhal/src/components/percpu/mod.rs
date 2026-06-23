//! Per-cpu module.
//!
//!

super::define_arch_mods!();
use crate::consts::VIRT_ADDR_START;
use core::ptr::copy_nonoverlapping;

extern "Rust" {
    pub(crate) fn __start_percpu();
    pub(crate) fn __stop_percpu();
}

/// This is a empty seat for percpu section.
/// Force the linker to create the percpu section.
#[link_section = "percpu"]
#[used(linker)]
static _PERCPU_SEAT: [usize; 0] = [0; 0];

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

#[cfg(target_arch = "x86_64")]
const PERCPU_RESERVED: usize = size_of::<PerCPUReserved>();
#[cfg(not(target_arch = "x86_64"))]
const PERCPU_RESERVED: usize = 0;

/// Returns the base address of the per-CPU data area on the given CPU.
///
/// if `cpu_id` is 0, it returns the base address of all per-CPU data areas.
pub fn percpu_area_init(_cpu_id: usize, dst: *mut u8) -> usize {
    // Get initial per-CPU data area
    let start = __start_percpu as usize;
    let size = __stop_percpu as usize - start;

    // Init the area with original data.
    unsafe {
        copy_nonoverlapping(start as *const u8, dst, size);
    }

    dst as usize
}

/// Read the architecture-specific thread pointer register on the current CPU.
pub fn get_local_thread_pointer() -> usize {
    let tp;
    unsafe {
        cfg_if::cfg_if! {
            if #[cfg(target_arch = "x86_64")] {
                tp = x86::msr::rdmsr(x86::msr::IA32_GS_BASE) as usize
            } else if #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))] {
                core::arch::asm!("mv {}, gp", out(reg) tp)
            } else if #[cfg(target_arch = "aarch64")] {
                core::arch::asm!("mrs {}, TPIDR_EL1", out(reg) tp)
            } else if #[cfg(target_arch = "loongarch64")] {
                core::arch::asm!("move {}, $r21", out(reg) tp)
            }
        }
    }
    tp
}

#[inline]
pub fn get_percpu_ptr() -> usize {
    let tp;
    unsafe {
        cfg_if::cfg_if! {
            if #[cfg(target_arch = "x86_64")] {
                // Get Valid Percpu Pointer
                core::arch::asm!("mov {}, gs:8", out(reg) tp)
            } else if #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))] {
                core::arch::asm!("mv {}, gp", out(reg) tp)
            } else if #[cfg(target_arch = "aarch64")] {
                core::arch::asm!("mrs {}, TPIDR_EL1", out(reg) tp)
            } else if #[cfg(target_arch = "loongarch64")] {
                core::arch::asm!("move {}, $r21", out(reg) tp)
            }
        }
    }
    tp
}

/// Set the architecture-specific thread pointer register to the per-CPU data
/// area base on the current CPU.
///
/// `cpu_id` indicates which per-CPU data area to use.
pub fn set_local_thread_pointer(cpu_id: usize) {
    #[cfg(target_arch = "loongarch64")]
    early_uart_puts("Kairix: set_local_thread_pointer enter\n");

    // Get initial per-CPU data area
    let alloc_size = __stop_percpu as usize - __start_percpu as usize + PERCPU_RESERVED;
    #[cfg(target_arch = "loongarch64")]
    early_uart_puts("Kairix: percpu alloc_size ready\n");

    // Alloc PerCPU Area
    let dst = unsafe { crate::mem::alloc(alloc_size).add(VIRT_ADDR_START) };
    #[cfg(target_arch = "loongarch64")]
    early_uart_puts("Kairix: percpu alloc done\n");

    let tp = percpu_area_init(cpu_id, unsafe { dst.add(PERCPU_RESERVED) });
    #[cfg(target_arch = "loongarch64")]
    early_uart_puts("Kairix: percpu area init done\n");

    unsafe {
        cfg_if::cfg_if! {
            if #[cfg(target_arch = "x86_64")] {
                x86::msr::wrmsr(x86::msr::IA32_GS_BASE, dst as u64);
                // Write cpu_local pointer to the first usize of the per-CPU data area
                // Write the valid address to the second usize of the per-CPU data area
                let percpu_reserved = PerCPUReserved::mut_from_ptr(dst as _);
                percpu_reserved.self_ptr = dst as _;
                percpu_reserved.valid_ptr = tp;
            } else if #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))] {
                core::arch::asm!("mv gp, {}", in(reg) tp);
                crate::arch::CPU_ID.write(cpu_id);
            } else if #[cfg(target_arch = "aarch64")] {
                core::arch::asm!("msr TPIDR_EL1, {}", in(reg) tp);
            } else if #[cfg(target_arch = "loongarch64")] {
                core::arch::asm!("move $r21, {}", in(reg) tp);
            }
        }
    }
    #[cfg(target_arch = "loongarch64")]
    early_uart_puts("Kairix: set_local_thread_pointer leave\n");
}
