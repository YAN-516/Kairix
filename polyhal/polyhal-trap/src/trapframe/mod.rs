//! Trapframe module.
//!
//!

use core::mem::size_of;

polyhal_macro::define_arch_mods!();

/// Trap Frame Arg Type
///
/// Using this by Index and IndexMut trait bound on TrapFrame
#[derive(Debug)]
pub enum TrapFrameArgs {
    SEPC,
    RA,
    SP,
    RET,
    ARG0,
    ARG1,
    ARG2,
    TLS,
    SYSCALL,
}

/// The size of the [TrapFrame]
pub const TRAPFRAME_SIZE: usize = size_of::<TrapFrame>();

/// Stack space reserved by a kernel-mode trap entry before it calls Rust.
///
/// A user trap stores `TrapFrame` in separately allocated task memory, but a
/// kernel trap constructs one directly below the current kernel stack pointer.
/// Keep that temporary allocation aligned to the 16-byte stack alignment
/// required by both the RISC-V and LoongArch ABIs even when `TrapFrame` itself
/// has a size that is only 8-byte aligned.
pub const KERNEL_TRAPFRAME_SIZE: usize = (TRAPFRAME_SIZE + 15) & !15;

const _: () = assert!(KERNEL_TRAPFRAME_SIZE % 16 == 0);
