//! Constants used in rCore
#[allow(unused)]
pub const USER_STACK_SIZE: usize = 4096 * 16;
pub const KERNEL_STACK_SIZE: usize = 4096 * 16;
pub const KERNEL_HEAP_SIZE: usize = 0x80_0000;

// #[allow(unused)]

// pub const VIRT_RAM_OFFSET: usize = 0xffff_ffc0_0000_0000;
// #[allow(unused)]
// pub const KERNEL_SPACE_OFFSET: usize = 0xffff_ffc0_0000_0000;
pub const _PTES_PER_PAGE: usize = 512;

pub const PAGE_SIZE: usize = 0x1000;
// pub const PAGE_SIZE_BITS: usize = 0xc;

pub const MAX_THREAD_NUM: usize = 16;
pub const MAX_CPU_NUM: usize = 4;

//pub const TRAMPOLINE: usize = usize::MAX - PAGE_SIZE + 1;
pub const TRAP_CONTEXT: usize = USER_MEMORY_SPACE.1 + 1 - PAGE_SIZE;

pub const USER_STACK_BASE: usize = TRAP_CONTEXT - MAX_THREAD_NUM * PAGE_SIZE;
pub const MMAP_BASE: usize = 0x4000_0000;
pub const KERNEL_CORE_STACK_BASE: usize = KERNEL_MEMORY_SPACE.1;

pub const KERNEL_THREAD_STACK_BASE: usize = KERNEL_CORE_STACK_BASE;

#[allow(unused)]
pub const KERNEL_MEMORY_SPACE: (usize, usize) = (0xffff_ffc0_0000_0000, 0xffff_ffff_ffff_ffff);
#[allow(unused)]
pub const USER_MEMORY_SPACE: (usize, usize) = (0x0, 0x3f_ffff_ffff);

pub use crate::board::{_CLOCK_FREQ, MEMORY_END, MMIO};

pub const BLOCK_SIZE: usize = 512;
