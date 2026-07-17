use core::{
    fmt::Debug,
    ops::{Index, IndexMut},
};

use riscv::register::sstatus::{self, SPP, Sstatus};

use super::TrapFrameArgs;

#[repr(C)]
#[derive(Clone)]
// 上下文
pub struct TrapFrame {
    pub x: [usize; 32], // 32 个通用寄存器
    pub sstatus: Sstatus,
    pub sepc: usize,
    /// Complete user floating-point register file.  A trap can preempt user
    /// code at any instruction, so caller-saved and callee-saved registers
    /// must both survive a task switch or CPU migration.
    pub f: [u64; 32],
    /// Floating-point control and status register.
    pub fcsr: usize,
}

const _: () = {
    assert!(core::mem::offset_of!(TrapFrame, f) == 34 * 8);
    assert!(core::mem::offset_of!(TrapFrame, fcsr) == 66 * 8);
    assert!(core::mem::size_of::<TrapFrame>() == 67 * 8);
};

impl Debug for TrapFrame {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Context")
            .field("ra", &self.x[1])
            .field("sp", &self.x[2])
            .field("gp", &self.x[3])
            .field("tp", &self.x[4])
            .field("t0", &self.x[5])
            .field("t1", &self.x[6])
            .field("t2", &self.x[7])
            .field("s0", &self.x[8])
            .field("s1", &self.x[9])
            .field("a0", &self.x[10])
            .field("a1", &self.x[11])
            .field("a2", &self.x[12])
            .field("a3", &self.x[13])
            .field("a4", &self.x[14])
            .field("a5", &self.x[15])
            .field("a6", &self.x[16])
            .field("a7", &self.x[17])
            .field("s2", &self.x[18])
            .field("s3", &self.x[19])
            .field("s4", &self.x[20])
            .field("s5", &self.x[21])
            .field("s6", &self.x[22])
            .field("s7", &self.x[23])
            .field("s8", &self.x[24])
            .field("s9", &self.x[25])
            .field("s10", &self.x[26])
            .field("s11", &self.x[27])
            .field("t3", &self.x[28])
            .field("t4", &self.x[29])
            .field("t5", &self.x[30])
            .field("t6", &self.x[31])
            .field("sstatus", &self.sstatus)
            .field("sepc", &self.sepc)
            .field("f", &self.f)
            .field("fcsr", &self.fcsr)
            .finish()
    }
}

impl TrapFrame {
    // 创建上下文信息
    #[inline]
    pub fn new() -> Self {
        TrapFrame {
            x: [0usize; 32],
            sstatus: sstatus::read(),
            sepc: 0,
            f: [0; 32],
            fcsr: 0,
        }
    }

    #[inline]
    pub fn args(&self) -> [usize; 6] {
        self.x[10..16].try_into().expect("args slice force convert")
    }

    /// Check if the trapframe was from user.
    #[inline]
    pub fn from_user(&self) -> bool {
        self.sstatus.spp() == SPP::User
    }

    #[inline]
    pub fn syscall_ok(&mut self) {
        self.sepc += 4;
    }
    pub fn pc(&self) -> usize {
        self.sepc
    }

    pub fn syscall_id(&self) -> usize {
        self.x[17]
    }

    pub fn ret_reg(&mut self) -> &mut usize {
        &mut self.x[10]
    }

    pub fn set_sp(&mut self, sp: usize) {
        self[TrapFrameArgs::SP] = sp;
    }

    pub fn set_pc(&mut self, pc: usize) {
        self.sepc = pc;
    }
}

impl Index<TrapFrameArgs> for TrapFrame {
    type Output = usize;

    fn index(&self, index: TrapFrameArgs) -> &Self::Output {
        match index {
            TrapFrameArgs::SEPC => &self.sepc,
            TrapFrameArgs::RA => &self.x[1],
            TrapFrameArgs::SP => &self.x[2],
            TrapFrameArgs::RET => &self.x[10],
            TrapFrameArgs::ARG0 => &self.x[10],
            TrapFrameArgs::ARG1 => &self.x[11],
            TrapFrameArgs::ARG2 => &self.x[12],
            TrapFrameArgs::TLS => &self.x[4],
            TrapFrameArgs::SYSCALL => &self.x[17],
        }
    }
}

impl IndexMut<TrapFrameArgs> for TrapFrame {
    fn index_mut(&mut self, index: TrapFrameArgs) -> &mut Self::Output {
        match index {
            TrapFrameArgs::SEPC => &mut self.sepc,
            TrapFrameArgs::RA => &mut self.x[1],
            TrapFrameArgs::SP => &mut self.x[2],
            TrapFrameArgs::RET => &mut self.x[10],
            TrapFrameArgs::ARG0 => &mut self.x[10],
            TrapFrameArgs::ARG1 => &mut self.x[11],
            TrapFrameArgs::ARG2 => &mut self.x[12],
            TrapFrameArgs::TLS => &mut self.x[4],
            TrapFrameArgs::SYSCALL => &mut self.x[17],
        }
    }
}
