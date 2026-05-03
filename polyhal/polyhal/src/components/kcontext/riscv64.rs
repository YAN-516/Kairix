use core::{
    arch::naked_asm,
    ops::{Index, IndexMut},
};

use crate::PageTable;

use crate::components::kcontext::KContextArgs;

use core::arch::global_asm;

global_asm!(include_str!("switch.S"));

unsafe extern "C" {
    /// Switch to the context of `next_task_cx_ptr`, saving the current context
    /// in `current_task_cx_ptr`.
    pub unsafe fn __switch(
        current_task_cx_ptr: *mut KContext,
        next_task_cx_ptr: *const KContext,
    );
}


/// Save the task context registers.
/// NOTE: tp (thread pointer) is intentionally NOT saved/restored.
/// tp is per-CPU identifier, not per-task state. It identifies which CPU
/// the code is running on and must be preserved across context switches.
macro_rules! save_callee_regs {
    () => {
        "
        sd      sp, 0*8(a0)
        sd      s0, 2*8(a0)
        sd      s1, 3*8(a0)
        sd      s2, 4*8(a0)
        sd      s3, 5*8(a0)
        sd      s4, 6*8(a0)
        sd      s5, 7*8(a0)
        sd      s6, 8*8(a0)
        sd      s7, 9*8(a0)
        sd      s8, 10*8(a0)
        sd      s9, 11*8(a0)
        sd      s10, 12*8(a0)
        sd      s11, 13*8(a0)
        sd      ra, 14*8(a0)
        "
    };
}

/// Restore the task context registers.
/// NOTE: tp (thread pointer) is intentionally NOT saved/restored.
/// tp is per-CPU identifier, not per-task state. It identifies which CPU
/// the code is running on and must be preserved across context switches.
macro_rules! restore_callee_regs {
    () => {
        "
        ld      sp, 0*8(a1)
        ld      s0, 2*8(a1)
        ld      s1, 3*8(a1)
        ld      s2, 4*8(a1)
        ld      s3, 5*8(a1)
        ld      s4, 6*8(a1)
        ld      s5, 7*8(a1)
        ld      s6, 8*8(a1)
        ld      s7, 9*8(a1)
        ld      s8, 10*8(a1)
        ld      s9, 11*8(a1)
        ld      s10, 12*8(a1)
        ld      s11, 13*8(a1)
        ld      ra, 14*8(a1)
        "
    };
}

/// Return instruction wrapper.
macro_rules! ret {
    () => {
        "ret"
    };
}

/// Kernel Context
///
/// Kernel Context is used to switch context between kernel task.
#[derive(Debug)]
#[repr(C)]
pub struct KContext {
    /// Kernel Stack Pointer
    ksp: usize,
    /// Kernel Thread Pointer
    ktp: usize,
    /// Kernel S regs, s0 - s11, just callee-saved registers
    /// just used in the context_switch function.
    _sregs: [usize; 12],
    /// Kernel Program Counter, Will return to this address.
    kpc: usize,
}

impl KContext {
    /// Create a new blank Kernel Context.
    pub fn blank() -> Self {
        Self {
            ksp: 0,
            ktp: 0,
            _sregs: [0; 12],
            kpc: 0,
        }
    }

    pub fn sp(&self) -> usize{
        self.ksp
    }

    pub fn ra(&self) -> usize {
        self._sregs[11]
    }
}

/// Indexing operations for KContext
///
/// Using it just like the Vector.
///
/// #[derive(Debug)]
/// pub enum KContextArgs {
///     /// Kernel Stack Pointer
///     KSP,
///     /// Kernel Thread Pointer
///     KTP,
///     /// Kernel Program Counter
///     KPC
/// }
///
/// etc. Get reg of the kernel stack:
///
/// let ksp = KContext[KContextArgs::KSP]
/// let kpc = KContext[KContextArgs::KPC]
/// let ktp = KContext[KContextArgs::KTP]
///
impl Index<KContextArgs> for KContext {
    type Output = usize;

    fn index(&self, index: KContextArgs) -> &Self::Output {
        match index {
            KContextArgs::KSP => &self.ksp,
            KContextArgs::KTP => &self.ktp,
            KContextArgs::KPC => &self.kpc,
        }
    }
}

/// Indexing Mutable operations for KContext
///
/// Using it just like the Vector.
///
/// etc. Change the value of the kernel Context using IndexMut
///
/// ```Rust
/// KContext[KContextArgs::KSP] = ksp;
/// KContext[KContextArgs::KPC] = kpc;
/// KContext[KContextArgs::KTP] = ktp;
/// ```
///
impl IndexMut<KContextArgs> for KContext {
    fn index_mut(&mut self, index: KContextArgs) -> &mut Self::Output {
        match index {
            KContextArgs::KSP => &mut self.ksp,
            KContextArgs::KTP => &mut self.ktp,
            KContextArgs::KPC => &mut self.kpc,
        }
    }
}

/// Context Switch
///
/// Save the context of current task and switch to new task.
/// 
#[naked]
pub unsafe extern "C" fn context_switch(from: *mut KContext, to: *const KContext) {
    naked_asm!(
        // Save Kernel Context.
        save_callee_regs!(),
        // Restore Kernel Context.
        restore_callee_regs!(),
        // Return to the caller.
        ret!(),
    )
}
// pub unsafe extern "C" fn context_switch(from: *mut KContext, to: *const KContext) {
//     __switch(from, to);
// }

/// Context Switch With Page Table
///
/// Save the context of current task and switch to new task.
#[inline]
pub unsafe extern "C" fn context_switch_pt(
    from: *mut KContext,
    to: *const KContext,
    pt_token: PageTable,
) {
    context_switch_pt_impl(from, to, pt_token.root().0 << 12);
}

/// Context Switch With Page Table Implement
///
/// The detail implementation of [context_switch_pt].
#[naked]
unsafe extern "C" fn context_switch_pt_impl(
    from: *mut KContext,
    to: *const KContext,
    pt_token: usize,
) {
    naked_asm!(
        // Save Kernel Context.
        save_callee_regs!(),
        // Switch to new page table.
        "
            srli    a2,   a2, 12
            li      a3,   8 << 60
            or      a2,   a2, a3
            csrw    satp, a2
            sfence.vma
        ",
        // Restore Kernel Context.
        restore_callee_regs!(),
        // Return to the caller.
        ret!(),
    )
}

#[naked]
pub extern "C" fn read_current_tp() -> usize {
    unsafe {
        naked_asm!(
            "
                mv      a0, tp
                ret
            ",
        )
    }
}
