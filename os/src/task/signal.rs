use core::sync::atomic::{AtomicU32, Ordering};

use crate::trap::_set_sum_bit;

/// 信号编号定义（使用 POSIX 标准全名）
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Signal {
    SigHup = 1,
    SigInt = 2,
    SigQuit = 3,
    SigIll = 4,
    SigTrap = 5,
    SigAbrt = 6,
    SigBus = 7,
    SigFpe = 8,
    SigKill = 9,
    SigUsr1 = 10,
    SigSegv = 11,
    SigUsr2 = 12,
    SigPipe = 13,
    SigAlrm = 14,
    SigTerm = 15,
    SigChld = 17,
    SigCont = 18,
    SigStop = 19,
    SigTstp = 20,
    SigTtin = 21,
    SigTtou = 22,
    SigUrg = 23,
    SigXcpu = 24,
    SigXfsz = 25,
    SigVtalrm = 26,
    SigProf = 27,
    SigWinch = 28,
    SigIo = 29,
    SigPwr = 30,
    SigSys = 31,
}

impl Signal {
    /// 从 i32 转换为 Signal
    pub const fn from_i32(value: i32) -> Option<Self> {
        match value {
            1 => Some(Signal::SigHup),
            2 => Some(Signal::SigInt),
            3 => Some(Signal::SigQuit),
            4 => Some(Signal::SigIll),
            5 => Some(Signal::SigTrap),
            6 => Some(Signal::SigAbrt),
            7 => Some(Signal::SigBus),
            8 => Some(Signal::SigFpe),
            9 => Some(Signal::SigKill),
            10 => Some(Signal::SigUsr1),
            11 => Some(Signal::SigSegv),
            12 => Some(Signal::SigUsr2),
            13 => Some(Signal::SigPipe),
            14 => Some(Signal::SigAlrm),
            15 => Some(Signal::SigTerm),
            17 => Some(Signal::SigChld),
            18 => Some(Signal::SigCont),
            19 => Some(Signal::SigStop),
            20 => Some(Signal::SigTstp),
            21 => Some(Signal::SigTtin),
            22 => Some(Signal::SigTtou),
            23 => Some(Signal::SigUrg),
            24 => Some(Signal::SigXcpu),
            25 => Some(Signal::SigXfsz),
            26 => Some(Signal::SigVtalrm),
            27 => Some(Signal::SigProf),
            28 => Some(Signal::SigWinch),
            29 => Some(Signal::SigIo),
            30 => Some(Signal::SigPwr),
            31 => Some(Signal::SigSys),
            _ => None,
        }
    }

    /// 获取信号编号
    pub const fn as_i32(self) -> i32 {
        self as i32
    }

    /// 信号是否可以被捕获或忽略
    pub const fn can_catch(self) -> bool {
        !matches!(self, Signal::SigKill | Signal::SigStop)
    }

    /// 获取信号的默认处理动作
    pub const fn default_action(self) -> SignalAction {
        match self {
            Signal::SigChld | Signal::SigCont | Signal::SigUrg | Signal::SigWinch => {
                SignalAction::Ignore
            }
            Signal::SigStop | Signal::SigTstp | Signal::SigTtin | Signal::SigTtou => {
                SignalAction::Stop
            }
            Signal::SigIll
            | Signal::SigAbrt
            | Signal::SigFpe
            | Signal::SigSegv
            | Signal::SigSys
            | Signal::SigBus
            | Signal::SigXcpu
            | Signal::SigXfsz => SignalAction::Core,
            _ => SignalAction::Terminate,
        }
    }

    /// 获取信号名称（用于调试）
    pub const fn name(self) -> &'static str {
        match self {
            Signal::SigHup => "SIGHUP",
            Signal::SigInt => "SIGINT",
            Signal::SigQuit => "SIGQUIT",
            Signal::SigIll => "SIGILL",
            Signal::SigTrap => "SIGTRAP",
            Signal::SigAbrt => "SIGABRT",
            Signal::SigBus => "SIGBUS",
            Signal::SigFpe => "SIGFPE",
            Signal::SigKill => "SIGKILL",
            Signal::SigUsr1 => "SIGUSR1",
            Signal::SigSegv => "SIGSEGV",
            Signal::SigUsr2 => "SIGUSR2",
            Signal::SigPipe => "SIGPIPE",
            Signal::SigAlrm => "SIGALRM",
            Signal::SigTerm => "SIGTERM",
            Signal::SigChld => "SIGCHLD",
            Signal::SigCont => "SIGCONT",
            Signal::SigStop => "SIGSTOP",
            Signal::SigTstp => "SIGTSTP",
            Signal::SigTtin => "SIGTTIN",
            Signal::SigTtou => "SIGTTOU",
            Signal::SigUrg => "SIGURG",
            Signal::SigXcpu => "SIGXCPU",
            Signal::SigXfsz => "SIGXFSZ",
            Signal::SigVtalrm => "SIGVTALRM",
            Signal::SigProf => "SIGPROF",
            Signal::SigWinch => "SIGWINCH",
            Signal::SigIo => "SIGIO",
            Signal::SigPwr => "SIGPWR",
            Signal::SigSys => "SIGSYS",
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
    /// 信号返回 trampoline 地址
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
