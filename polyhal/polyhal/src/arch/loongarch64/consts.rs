use crate::utils::addr::*;

pub const VIRT_ADDR_START: usize = 0x9000_0000_0000_0000;
pub const KERNEL_MEMORY_SPACE: (usize, usize) = (0x9000_0000_0000_0000, 0xffff_ffff_ffff_ffff);
pub const USER_MEMORY_SPACE: (usize, usize) = (0x0, 0x0000_003f_ffff_ffff);
#[allow(unused)]
pub const USER_STACK_SIZE: usize = 4096 * 64;
pub const KERNEL_STACK_SIZE: usize = 4096 * 8;
pub const KERNEL_HEAP_SIZE: usize = 0x800_0000;
pub const MAX_THREAD_NUM: usize = 16;
pub const MAX_CPU_NUM: usize = 4;
// pub const TRAP_CONTEXT: usize = 0x9000_0000_8600_0000 - PAGE_SIZE;
pub const TRAP_CONTEXT: usize = USER_MEMORY_SPACE.1 + 1 - PAGE_SIZE;

pub const PAGE_SIZE: usize = 4096;
pub const USER_STACK_BASE: usize = TRAP_CONTEXT - MAX_THREAD_NUM * PAGE_SIZE;
// pub const KERNEL_CORE_STACK_BASE: usize = KERNEL_MEMORY_SPACE.1;
pub const KERNEL_CORE_STACK_BASE: usize = 0xffff_ffff_ffef_ffff;

pub const PTES_PER_PAGE: usize = 512;
pub const KERNEL_THREAD_STACK_BASE: usize = KERNEL_CORE_STACK_BASE;
pub const PAGE_SIZE_BITS: usize = 12;
/// QEMU Loongarch64 Virt Machine:
///     https://github.com/qemu/qemu/blob/master/include/hw/loongarch/virt.h
pub const QEMU_DTB_ADDR: PhysAddr = PhysAddr(0x100000);

pub const STACK_EXPAND_LIMIT: usize = PAGE_SIZE * 2;
pub const MAX_STACK_SIZE: usize = 8 * 1024 * 1024; // 8MB 总上限
