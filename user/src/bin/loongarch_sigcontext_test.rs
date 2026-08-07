#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

#[cfg(target_arch = "loongarch64")]
mod loongarch_test {
    use core::arch::asm;
    use core::ptr::{read_unaligned, write_unaligned};
    use core::sync::atomic::{AtomicUsize, Ordering};
    use user_lib::{SIGUSR1, getpid, kill};

    const SYSCALL_RT_SIGACTION: usize = 134;
    const SA_SIGINFO: usize = 4;
    const UCONTEXT_MCONTEXT_OFFSET: usize = 176;
    const SIGCONTEXT_REGS_OFFSET: usize = 8;
    const SIGCONTEXT_FLAGS_OFFSET: usize = 264;
    const SIGCONTEXT_EXTCONTEXT_OFFSET: usize = 272;
    const END_SCTX_OFFSET: usize = 816;
    const SC_USED_FP: u32 = 1;
    const LSX_CTX_MAGIC: u32 = 0x5358_0001;
    const LSX_SCTX_SIZE: u32 = 544;
    const MODIFIED_A0: usize = 0x4b41_4952;

    static HANDLER_RESULT: AtomicUsize = AtomicUsize::new(0);

    #[repr(C)]
    struct KernelSigAction {
        handler: usize,
        flags: usize,
        mask: usize,
    }

    unsafe extern "C" fn siginfo_handler(signal: i32, siginfo: *const u8, ucontext: *mut u8) {
        if signal != SIGUSR1 || siginfo.is_null() || ucontext.is_null() {
            HANDLER_RESULT.store(1, Ordering::SeqCst);
            return;
        }

        let mcontext = unsafe { ucontext.add(UCONTEXT_MCONTEXT_OFFSET) };
        let pc = unsafe { read_unaligned(mcontext.cast::<u64>()) } as usize;
        let regs = unsafe { mcontext.add(SIGCONTEXT_REGS_OFFSET).cast::<u64>() };
        let r0 = unsafe { read_unaligned(regs.add(0)) };
        let saved_sp = unsafe { read_unaligned(regs.add(3)) } as usize;
        let flags = unsafe { read_unaligned(mcontext.add(SIGCONTEXT_FLAGS_OFFSET).cast::<u32>()) };
        let info = unsafe { mcontext.add(SIGCONTEXT_EXTCONTEXT_OFFSET) };
        let magic = unsafe { read_unaligned(info.cast::<u32>()) };
        let size = unsafe { read_unaligned(info.add(4).cast::<u32>()) };
        let end = unsafe { mcontext.add(END_SCTX_OFFSET) };
        let end_magic = unsafe { read_unaligned(end.cast::<u32>()) };
        let end_size = unsafe { read_unaligned(end.add(4).cast::<u32>()) };

        if pc < 0x1000
            || pc & 3 != 0
            || r0 != 0
            || saved_sp <= ucontext as usize
            || flags & SC_USED_FP == 0
            || magic != LSX_CTX_MAGIC
            || size != LSX_SCTX_SIZE
            || end_magic != 0
            || end_size != 0
        {
            HANDLER_RESULT.store(2, Ordering::SeqCst);
            return;
        }

        // QEMU-style handlers inspect and may update the interrupted context.
        // Changing a0 proves that rt_sigreturn consumes the Linux ABI layout,
        // rather than a matching private layout understood only by the kernel.
        unsafe { write_unaligned(regs.add(4) as *mut u64, MODIFIED_A0 as u64) };
        HANDLER_RESULT.store(3, Ordering::SeqCst);
    }

    fn install_handler() -> isize {
        let action = KernelSigAction {
            handler: siginfo_handler as usize,
            flags: SA_SIGINFO,
            mask: 0,
        };
        let ret: isize;
        unsafe {
            asm!(
                "syscall 0",
                inlateout("$a0") SIGUSR1 as usize => ret,
                in("$a1") &action as *const KernelSigAction as usize,
                in("$a2") 0usize,
                in("$a3") core::mem::size_of::<usize>(),
                in("$a4") 0usize,
                in("$a5") 0usize,
                in("$a7") SYSCALL_RT_SIGACTION,
            );
        }
        ret
    }

    pub fn run() -> i32 {
        println!("[loongarch_sigcontext_test] start");
        let install = install_handler();
        let signal_return = if install == 0 {
            kill(getpid(), SIGUSR1 as usize)
        } else {
            install
        };
        let handler_result = HANDLER_RESULT.load(Ordering::SeqCst);
        println!(
            "[loongarch_sigcontext_test] install={} handler={} restored_a0={:#x}",
            install, handler_result, signal_return,
        );
        if install == 0 && handler_result == 3 && signal_return == MODIFIED_A0 as isize {
            println!("[loongarch_sigcontext_test] PASS");
            0
        } else {
            println!("[loongarch_sigcontext_test] FAIL");
            1
        }
    }
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    #[cfg(target_arch = "loongarch64")]
    {
        loongarch_test::run()
    }
    #[cfg(not(target_arch = "loongarch64"))]
    {
        println!("[loongarch_sigcontext_test] SKIP: loongarch64 only");
        0
    }
}
