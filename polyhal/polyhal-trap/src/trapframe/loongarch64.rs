use super::TrapFrameArgs;
use core::ops::{Index, IndexMut};
use polyhal::println;

/// Saved registers when a trap (interrupt or exception) occurs.
#[allow(missing_docs)]
#[repr(C, align(16))]
#[derive(Debug, Default, Clone, Copy)]
pub struct TrapFrame {
    /// General Registers
    pub regs: [usize; 32],
    /// Pre-exception Mode information
    pub prmd: usize,
    /// Exception Return Address
    pub era: usize,
    /// Complete 128-bit LSX register file.
    ///
    /// LoongArch scalar floating-point registers alias the low 64-bit lanes of
    /// these vector registers. Keeping both lanes here is required once SXE is
    /// enabled: saving only the scalar lane loses user LSX state, while storing
    /// 128-bit vectors into the old scalar layout corrupts the following
    /// kernel context.
    pub vr: [[u64; 2]; 32],
    /// Floating-point condition-code registers, one byte per FCC bit.
    pub fcc: [u8; 8],
    /// Floating-point control and status register.
    pub fcsr: usize,
}

const _: () = {
    assert!(core::mem::offset_of!(TrapFrame, vr) == 34 * 8);
    assert!(core::mem::offset_of!(TrapFrame, fcc) == 34 * 8 + 32 * 16);
    assert!(core::mem::offset_of!(TrapFrame, fcsr) == 34 * 8 + 32 * 16 + 8);
    assert!(core::mem::size_of::<TrapFrame>() == 800);
};

impl TrapFrame {
    /// Return the scalar floating-point register view used by the base Linux
    /// signal context. Each scalar register is the low lane of its LSX vector.
    pub fn scalar_fp_regs(&self) -> [u64; 32] {
        core::array::from_fn(|index| self.vr[index][0])
    }

    /// Return the upper lanes needed to preserve complete LSX state across a
    /// signal handler.
    pub fn lsx_upper_regs(&self) -> [u64; 32] {
        core::array::from_fn(|index| self.vr[index][1])
    }

    /// Restore the scalar floating-point view without discarding LSX upper
    /// lanes, which are restored separately from the signal extension.
    pub fn set_scalar_fp_regs(&mut self, values: [u64; 32]) {
        for (register, value) in self.vr.iter_mut().zip(values) {
            register[0] = value;
        }
    }

    /// Restore all LSX upper lanes from the signal extension.
    pub fn set_lsx_upper_regs(&mut self, values: [u64; 32]) {
        for (register, value) in self.vr.iter_mut().zip(values) {
            register[1] = value;
        }
    }

    // 创建上下文信息
    #[inline]
    pub fn new() -> Self {
        Self {
            // bit 1:0 PLV
            // bit 2 PIE
            // bit 3 PWE
            prmd: (0b0111),
            ..Default::default()
        }
    }
}

impl TrapFrame {
    pub fn syscall_ok(&mut self) {
        self.era += 4;
    }

    #[inline]
    pub fn args(&self) -> [usize; 6] {
        [
            self.regs[4],
            self.regs[5],
            self.regs[6],
            self.regs[7],
            self.regs[8],
            self.regs[9],
        ]
    }

    pub fn syscall_id(&mut self) -> usize {
        self.regs[11]
    }
    pub fn pc(&self) -> usize {
        self.era
    }
    pub fn ret_reg(&mut self) -> &mut usize {
        &mut self.regs[4]
    }
    pub fn set_sp(&mut self, sp: usize) {
        println!("set sp {:#x}", sp);
        self[TrapFrameArgs::SP] = sp;
    }

    pub fn set_pc(&mut self, pc: usize) {
        self.era = pc;
    }
}

impl Index<TrapFrameArgs> for TrapFrame {
    type Output = usize;

    fn index(&self, index: TrapFrameArgs) -> &Self::Output {
        match index {
            TrapFrameArgs::SEPC => &self.era,
            TrapFrameArgs::RA => &self.regs[1],
            TrapFrameArgs::SP => &self.regs[3],
            TrapFrameArgs::RET => &self.regs[4],
            TrapFrameArgs::ARG0 => &self.regs[4],
            TrapFrameArgs::ARG1 => &self.regs[5],
            TrapFrameArgs::ARG2 => &self.regs[6],
            TrapFrameArgs::TLS => &self.regs[2],
            TrapFrameArgs::SYSCALL => &self.regs[11],
        }
    }
}

impl IndexMut<TrapFrameArgs> for TrapFrame {
    fn index_mut(&mut self, index: TrapFrameArgs) -> &mut Self::Output {
        match index {
            TrapFrameArgs::SEPC => &mut self.era,
            TrapFrameArgs::RA => &mut self.regs[1],
            TrapFrameArgs::SP => &mut self.regs[3],
            TrapFrameArgs::RET => &mut self.regs[4],
            TrapFrameArgs::ARG0 => &mut self.regs[4],
            TrapFrameArgs::ARG1 => &mut self.regs[5],
            TrapFrameArgs::ARG2 => &mut self.regs[6],
            TrapFrameArgs::TLS => &mut self.regs[2],
            TrapFrameArgs::SYSCALL => &mut self.regs[11],
        }
    }
}
