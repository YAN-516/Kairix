#![allow(non_upper_case_globals)]

use core::sync::atomic::{AtomicU32, Ordering};

use crate::trap::_set_sum_bit;

/// 信号编号定义（使用 POSIX 标准全名）
/// 以结构体形式实现，支持 1..=64 的所有信号，包含实时信号。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Signal(pub i32);

impl Signal {
    // 标准信号常量
    #[allow(non_upper_case_globals)]
    pub const SigHup: Signal = Signal(1);
    pub const SigInt: Signal = Signal(2);
    pub const SigQuit: Signal = Signal(3);
    pub const SigIll: Signal = Signal(4);
    pub const SigTrap: Signal = Signal(5);
    pub const SigAbrt: Signal = Signal(6);
    pub const SigBus: Signal = Signal(7);
    pub const SigFpe: Signal = Signal(8);
    pub const SigKill: Signal = Signal(9);
    pub const SigUsr1: Signal = Signal(10);
    pub const SigSegv: Signal = Signal(11);
    pub const SigUsr2: Signal = Signal(12);
    pub const SigPipe: Signal = Signal(13);
    pub const SigAlrm: Signal = Signal(14);
    pub const SigTerm: Signal = Signal(15);
    pub const SigChld: Signal = Signal(17);
    pub const SigCont: Signal = Signal(18);
    pub const SigStop: Signal = Signal(19);
    pub const SigTstp: Signal = Signal(20);
    pub const SigTtin: Signal = Signal(21);
    pub const SigTtou: Signal = Signal(22);
    pub const SigUrg: Signal = Signal(23);
    pub const SigXcpu: Signal = Signal(24);
    pub const SigXfsz: Signal = Signal(25);
    pub const SigVtalrm: Signal = Signal(26);
    pub const SigProf: Signal = Signal(27);
    pub const SigWinch: Signal = Signal(28);
    pub const SigIo: Signal = Signal(29);
    pub const SigPwr: Signal = Signal(30);
    pub const SigSys: Signal = Signal(31);

    /// 从 i32 转换为 Signal，支持 1..=64
    pub const fn from_i32(value: i32) -> Option<Self> {
        if value >= 1 && value <= 64 {
            Some(Self(value))
        } else {
            None
        }
    }

    /// 获取信号编号
    pub const fn as_i32(self) -> i32 {
        self.0
    }

    /// 信号是否可以被捕获或忽略
    pub const fn can_catch(self) -> bool {
        !(self.0 == Self::SigKill.0 || self.0 == Self::SigStop.0)
    }

    /// 获取信号的默认处理动作
    pub const fn default_action(self) -> SignalAction {
        match self {
            Self::SigChld | Self::SigCont | Self::SigUrg | Self::SigWinch => SignalAction::Ignore,
            Self::SigStop | Self::SigTstp | Self::SigTtin | Self::SigTtou => SignalAction::Stop,
            Self::SigIll
            | Self::SigAbrt
            | Self::SigFpe
            | Self::SigSegv
            | Self::SigSys
            | Self::SigBus
            | Self::SigXcpu
            | Self::SigXfsz => SignalAction::Core,
            _ => SignalAction::Terminate,
        }
    }

    /// 获取信号名称（用于调试）
    pub const fn name(self) -> &'static str {
        match self {
            Self::SigHup => "SIGHUP",
            Self::SigInt => "SIGINT",
            Self::SigQuit => "SIGQUIT",
            Self::SigIll => "SIGILL",
            Self::SigTrap => "SIGTRAP",
            Self::SigAbrt => "SIGABRT",
            Self::SigBus => "SIGBUS",
            Self::SigFpe => "SIGFPE",
            Self::SigKill => "SIGKILL",
            Self::SigUsr1 => "SIGUSR1",
            Self::SigSegv => "SIGSEGV",
            Self::SigUsr2 => "SIGUSR2",
            Self::SigPipe => "SIGPIPE",
            Self::SigAlrm => "SIGALRM",
            Self::SigTerm => "SIGTERM",
            Self::SigChld => "SIGCHLD",
            Self::SigCont => "SIGCONT",
            Self::SigStop => "SIGSTOP",
            Self::SigTstp => "SIGTSTP",
            Self::SigTtin => "SIGTTIN",
            Self::SigTtou => "SIGTTOU",
            Self::SigUrg => "SIGURG",
            Self::SigXcpu => "SIGXCPU",
            Self::SigXfsz => "SIGXFSZ",
            Self::SigVtalrm => "SIGVTALRM",
            Self::SigProf => "SIGPROF",
            Self::SigWinch => "SIGWINCH",
            Self::SigIo => "SIGIO",
            Self::SigPwr => "SIGPWR",
            Self::SigSys => "SIGSYS",
            _ => "SIGRT",
        }
    }
}

/// 信号默认处理动作
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalAction {
    /// 终止进程
    Terminate,
    /// 忽略信号
    Ignore,
    /// 停止进程
    Stop,
    /// 继续进程
    Continue,
    /// 产生核心转储并终止
    Core,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignalSet {
    bits: u64,
}

impl SignalSet {
    pub const fn empty() -> Self {
        Self { bits: 0 }
    }

    pub const fn all() -> Self {
        Self { bits: !0 }
    }

    pub fn add(&mut self, signal: Signal) {
        self.bits |= 1 << (signal.as_i32() - 1);
    }

    pub fn remove(&mut self, signal: Signal) {
        self.bits &= !(1 << (signal.as_i32() - 1));
    }

    pub fn contains(&self, signal: Signal) -> bool {
        (self.bits & (1 << (signal.as_i32() - 1))) != 0
    }

    pub const fn bits(&self) -> u64 {
        self.bits
    }

    pub fn from_bits(bits: u64) -> Self {
        Self { bits }
    }

    pub fn is_empty(&self) -> bool {
        self.bits == 0
    }
}

impl core::ops::BitOrAssign for SignalSet {
    fn bitor_assign(&mut self, rhs: Self) {
        self.bits |= rhs.bits;
    }
}

/// 信号处理方式枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigHandler {
    /// 默认处理 (SIG_DFL)
    Default,
    /// 忽略信号 (SIG_IGN)
    Ignore,
    /// 用户自定义函数
    Custom(unsafe extern "C" fn(i32)),
}

/// SA_RESTART 标志：被该信号中断的系统调用会自动重启
pub const SA_RESTART: u32 = 0x10000000;

impl SigHandler {
    /// 转换为原始指针（用于系统调用传递）
    pub const fn as_ptr(self) -> *const core::ffi::c_void {
        match self {
            SigHandler::Default => 0 as *const _,
            SigHandler::Ignore => 1 as *const _,
            SigHandler::Custom(f) => f as *const _,
        }
    }

    /// 从原始指针转换（从系统调用获取）
    pub unsafe fn from_ptr(ptr: *const core::ffi::c_void) -> Self {
        unsafe {
            match ptr as usize {
                0 => SigHandler::Default,
                1 => SigHandler::Ignore,
                _ => SigHandler::Custom(core::mem::transmute(ptr)),
            }
        }
    }
}

/// 信号处理动作
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SigAction {
    /// 信号处理方式
    pub sa_handler: SigHandler,
    /// 在处理信号时要屏蔽的信号集
    pub sa_mask: SignalSet,
    /// 处理标志
    pub sa_flags: u32,
    /// 恢复函数地址（musl 使用）
    pub sa_restorer: usize,
}

impl SigAction {
    pub const fn default() -> Self {
        Self {
            sa_handler: SigHandler::Default,
            sa_mask: SignalSet::empty(),
            sa_flags: 0,
            sa_restorer: 0,
        }
    }

    pub const fn ignore() -> Self {
        Self {
            sa_handler: SigHandler::Ignore,
            sa_mask: SignalSet::empty(),
            sa_flags: 0,
            sa_restorer: 0,
        }
    }

    pub fn is_default(&self) -> bool {
        matches!(self.sa_handler, SigHandler::Default)
    }

    pub fn is_ignored(&self) -> bool {
        matches!(self.sa_handler, SigHandler::Ignore)
    }

    pub fn is_custom(&self) -> bool {
        matches!(self.sa_handler, SigHandler::Custom(_))
    }
}
// 进程的信号处理表
#[derive(Debug, Clone, Copy)]
pub struct SignalHandlers {
    // 为每个信号（1-64）保存一个 SigAction
    actions: [SigAction; 64], // 索引 1 对应信号 1，索引 2 对应信号 2...
}

impl SignalHandlers {
    pub fn new() -> Self {
        let mut actions = [SigAction::default(); 64];

        if let Some(kill) = Signal::from_i32(9) {
            let idx = kill.as_i32() as usize;
            actions[idx].sa_flags = 0xDEAD; // 标记为不可改变
        }
        if let Some(stop) = Signal::from_i32(19) {
            let idx = stop.as_i32() as usize;
            actions[idx].sa_flags = 0xDEAD; // 标记为不可改变
        }

        Self { actions }
    }

    /// 获取指定信号的配置
    pub fn get(&self, signal: Signal) -> SigAction {
        let idx = signal.as_i32() as usize;
        if idx < self.actions.len() {
            self.actions[idx]
        } else {
            // 无效信号，返回默认配置
            SigAction::default()
        }
    }

    /// 获取指定信号的配置（可变引用）
    pub fn get_mut(&mut self, signal: Signal) -> Option<&mut SigAction> {
        let idx = signal.as_i32() as usize;
        if idx < self.actions.len() {
            Some(&mut self.actions[idx])
        } else {
            None
        }
    }
    pub fn set(&mut self, signal: Signal, action: *const SigAction) -> Result<(), &'static str> {
        // 检查信号是否可以被修改
        _set_sum_bit();
        if !signal.can_catch() {
            return Err("Cannot change action for SIGKILL or SIGSTOP");
        }

        let idx = signal.as_i32() as usize;
        if idx >= self.actions.len() {
            return Err("Invalid signal number");
        }

        // 检查是否为特殊信号（再次确认）
        if self.actions[idx].sa_flags == 0xDEAD {
            return Err("Cannot modify SIGKILL or SIGSTOP");
        }
        unsafe {
            self.actions[idx] = *action;
        }
        // 设置新的动作

        Ok(())
    }

    /// 重置指定信号为默认处理
    pub fn reset(&mut self, signal: Signal) -> Result<(), &'static str> {
        if !signal.can_catch() {
            return Err("Cannot reset SIGKILL or SIGSTOP");
        }

        let idx = signal.as_i32() as usize;
        if idx < self.actions.len() {
            self.actions[idx] = SigAction::default();
            Ok(())
        } else {
            Err("Invalid signal number")
        }
    }

    /// 重置所有信号为默认处理（但保留 SIG_IGN 的信号）
    /// 用于 exec() 系统调用
    pub fn reset_all(&mut self) {
        for i in 1..self.actions.len() {
            if let Some(signal) = Signal::from_i32(i as i32) {
                // SIGKILL 和 SIGSTOP 不能被改变
                if signal.can_catch() {
                    let action = self.actions[i];
                    // 如果之前是 SIG_IGN，保持忽略（exec 特殊规则）
                    if !action.is_ignored() {
                        self.actions[i] = SigAction::default();
                    }
                }
            }
        }
    }

    /// 检查指定信号是否被忽略
    pub fn is_ignored(&self, signal: Signal) -> bool {
        self.get(signal).is_ignored()
    }

    /// 检查指定信号是否使用默认处理
    pub fn is_default(&self, signal: Signal) -> bool {
        self.get(signal).is_default()
    }

    /// 检查指定信号是否有自定义处理器
    pub fn is_custom(&self, signal: Signal) -> bool {
        self.get(signal).is_custom()
    }

    /// 获取自定义处理器函数（如果有）
    pub fn get_handler(&self, signal: Signal) -> Option<unsafe extern "C" fn(i32)> {
        match self.get(signal).sa_handler {
            SigHandler::Custom(f) => Some(f),
            _ => None,
        }
    }
}
