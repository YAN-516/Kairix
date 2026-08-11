#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::arch::asm;
use core::ptr::{read_unaligned, write_volatile};
use core::sync::atomic::{AtomicUsize, Ordering};
use user_lib::{AT_FDCWD, OpenFlags, close, exit, fork, mmap, open, waitpid, write};

const PAGE_SIZE: usize = 4096;
const PROT_READ: usize = 0x1;
const MAP_SHARED: usize = 0x01;
const SIGBUS: i32 = 7;
const SIGSEGV: i32 = 11;
const SA_SIGINFO: usize = 4;
const SYSCALL_RT_SIGACTION: usize = 134;
const SEGV_ACCERR: i32 = 2;
const SIGSEGV_OK: i32 = 42;
const SIGBUS_BAD: i32 = 43;
const SIGINFO_NULL: i32 = 44;
const SIGINFO_CODE_BAD: i32 = 45;
const SIGINFO_ADDR_BAD: i32 = 46;

static EXPECTED_FAULT_ADDR: AtomicUsize = AtomicUsize::new(0);

#[repr(C)]
struct KernelSigAction {
    handler: usize,
    flags: usize,
    mask: usize,
}

unsafe extern "C" fn fault_handler(sig: i32, siginfo: *const u8, _ucontext: *mut u8) {
    if sig == SIGBUS {
        exit(SIGBUS_BAD);
    }
    if sig != SIGSEGV || siginfo.is_null() {
        exit(SIGINFO_NULL);
    }

    let code = unsafe { read_unaligned(siginfo.add(8).cast::<i32>()) };
    let address = unsafe { read_unaligned(siginfo.add(16).cast::<usize>()) };
    if code != SEGV_ACCERR {
        exit(SIGINFO_CODE_BAD);
    }
    if address != EXPECTED_FAULT_ADDR.load(Ordering::SeqCst) {
        exit(SIGINFO_ADDR_BAD);
    }
    exit(SIGSEGV_OK);
}

fn install_fault_handler(signal: i32) -> isize {
    let action = KernelSigAction {
        handler: fault_handler as usize,
        flags: SA_SIGINFO,
        mask: 0,
    };
    let ret: isize;
    unsafe {
        #[cfg(target_arch = "riscv64")]
        asm!(
            "ecall",
            inlateout("a0") signal as usize => ret,
            in("a1") &action as *const KernelSigAction as usize,
            in("a2") 0usize,
            in("a3") core::mem::size_of::<usize>(),
            in("a4") 0usize,
            in("a5") 0usize,
            in("a7") SYSCALL_RT_SIGACTION,
        );
        #[cfg(target_arch = "loongarch64")]
        asm!(
            "syscall 0",
            inlateout("$a0") signal as usize => ret,
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

fn decode_exit(status: i32) -> Option<i32> {
    if (status & 0x7f) == 0 {
        Some((status >> 8) & 0xff)
    } else {
        None
    }
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("[mmap_prot_sig] start");

    let path = "mmap_prot_sig.tmp";
    let fd = open(
        AT_FDCWD,
        path,
        OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::WRONLY,
        0,
    );
    if fd < 0 {
        println!("[mmap_prot_sig] create failed: {}", fd);
        return 1;
    }
    let data = [0x5au8];
    let wrote = write(fd as usize, &data);
    let _ = close(fd as usize);
    if wrote != data.len() as isize {
        println!("[mmap_prot_sig] write failed: {}", wrote);
        return 1;
    }

    let fd = open(AT_FDCWD, path, OpenFlags::RDONLY, 0);
    if fd < 0 {
        println!("[mmap_prot_sig] reopen failed: {}", fd);
        return 1;
    }

    let pid = fork();
    if pid == 0 {
        if install_fault_handler(SIGSEGV) != 0 {
            exit(10);
        }
        if install_fault_handler(SIGBUS) != 0 {
            exit(11);
        }

        let addr = mmap(0, PAGE_SIZE, PROT_READ, MAP_SHARED, fd, 0);
        if addr < 0 {
            exit(12);
        }
        EXPECTED_FAULT_ADDR.store(addr as usize, Ordering::SeqCst);

        unsafe {
            write_volatile(addr as *mut u8, 0xa5);
        }

        exit(13);
    }
    let _ = close(fd as usize);
    if pid < 0 {
        println!("[mmap_prot_sig] fork failed: {}", pid);
        return 1;
    }

    let mut status = 0;
    let waited = waitpid(pid as usize, &mut status);
    if waited != pid {
        println!(
            "[mmap_prot_sig] wait failed: pid {}, waited {}, status {}",
            pid, waited, status
        );
        return 1;
    }

    match decode_exit(status) {
        Some(SIGSEGV_OK) => {
            println!("[mmap_prot_sig] PASS: write to PROT_READ mmap raised SIGSEGV");
            0
        }
        Some(SIGBUS_BAD) => {
            println!("[mmap_prot_sig] FAIL: got SIGBUS, expected SIGSEGV");
            1
        }
        Some(SIGINFO_NULL) => {
            println!("[mmap_prot_sig] FAIL: invalid SA_SIGINFO arguments");
            1
        }
        Some(SIGINFO_CODE_BAD) => {
            println!("[mmap_prot_sig] FAIL: si_code was not SEGV_ACCERR");
            1
        }
        Some(SIGINFO_ADDR_BAD) => {
            println!("[mmap_prot_sig] FAIL: si_addr did not match the fault address");
            1
        }
        Some(13) => {
            println!("[mmap_prot_sig] FAIL: write to PROT_READ mmap succeeded");
            1
        }
        Some(code) => {
            println!("[mmap_prot_sig] FAIL: child exit {}", code);
            1
        }
        None => {
            println!(
                "[mmap_prot_sig] FAIL: child was killed before custom handler, status {}",
                status
            );
            1
        }
    }
}
