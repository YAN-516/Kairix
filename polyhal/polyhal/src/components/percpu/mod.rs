//! Per-cpu module.
//!
//!

super::define_arch_mods!();
use crate::consts::VIRT_ADDR_START;
use core::ptr::copy_nonoverlapping;
use core::sync::atomic::{AtomicUsize, Ordering};

extern "Rust" {
    pub(crate) fn __start_percpu();
    pub(crate) fn __stop_percpu();
}

/// This is a empty seat for percpu section.
/// Force the linker to create the percpu section.
#[link_section = "percpu"]
#[used(linker)]
static _PERCPU_SEAT: [usize; 0] = [0; 0];

#[cfg(target_arch = "x86_64")]
const PERCPU_RESERVED: usize = size_of::<PerCPUReserved>();
#[cfg(not(target_arch = "x86_64"))]
const PERCPU_RESERVED: usize = 0;

static PERCPU_AREA_PHYS: [AtomicUsize; crate::consts::MAX_CPU_NUM] =
    [const { AtomicUsize::new(0) }; crate::consts::MAX_CPU_NUM];

/// Reserve one CPU's per-CPU area from the single-use early allocator.
///
/// The boot CPU calls this for every CPU before any secondary is started, so
/// secondary initialization never races the boot-stack allocator or the
/// kernel frame-allocator handoff.
pub fn reserve_local_thread_pointer(cpu_id: usize) -> usize {
    assert!(cpu_id < PERCPU_AREA_PHYS.len(), "per-CPU id out of range");
    const RESERVING: usize = usize::MAX;
    loop {
        let existing = PERCPU_AREA_PHYS[cpu_id].load(Ordering::Acquire);
        match existing {
            0 => {
                if PERCPU_AREA_PHYS[cpu_id]
                    .compare_exchange(0, RESERVING, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    break;
                }
            }
            RESERVING => core::hint::spin_loop(),
            address => return address,
        }
    }

    let alloc_size = __stop_percpu as usize - __start_percpu as usize + PERCPU_RESERVED;
    let allocated = unsafe { crate::mem::alloc(alloc_size) as usize };
    assert_ne!(allocated, RESERVING, "invalid per-CPU physical address");
    PERCPU_AREA_PHYS[cpu_id].store(allocated, Ordering::Release);
    allocated
}

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
    let phys = reserve_local_thread_pointer(cpu_id);
    let dst = (phys + VIRT_ADDR_START) as *mut u8;

    let tp = percpu_area_init(cpu_id, unsafe { dst.add(PERCPU_RESERVED) });
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
}
