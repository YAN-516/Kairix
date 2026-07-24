#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::arch::global_asm;
use core::ptr::{addr_of_mut, read_volatile, write_volatile};
use core::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};
use user_lib::{
    OpenFlags, SIGUSR1, SigAction, SigHandler, chdir, close, exit, getcwd, mkdir, mmap, open,
    sigaction, waitpid,
};

const CLONE_VM: usize = 0x0000_0100;
const CLONE_FS: usize = 0x0000_0200;
const CLONE_FILES: usize = 0x0000_0400;
const CLONE_SIGHAND: usize = 0x0000_0800;
const CLONE_VFORK: usize = 0x0000_4000;
const SIGCHLD: usize = 17;
const PAGE_SIZE: usize = 4096;
const PROT_READ: usize = 1;
const PROT_WRITE: usize = 2;
const MAP_PRIVATE: usize = 2;
const MAP_ANONYMOUS: usize = 0x20;
const MAGIC: usize = 0x434c_4f4e_4556_4d21;

#[repr(align(16))]
struct ChildStack([u8; 64 * 1024]);

static mut CHILD_STACK: ChildStack = ChildStack([0; 64 * 1024]);
static SHARED_FD: AtomicIsize = AtomicIsize::new(-1);
static NEW_MAPPING: AtomicUsize = AtomicUsize::new(0);

#[cfg(target_arch = "riscv64")]
global_asm!(
    r#"
    .globl clone_shared_context_spawn
clone_shared_context_spawn:
    mv t0, a2
    mv t1, a3
    li a2, 0
    li a3, 0
    li a4, 0
    li a7, 220
    ecall
    bnez a0, 1f
    mv a0, t1
    jalr t0
    li a0, 99
    li a7, 93
    ecall
1:
    ret
"#
);

#[cfg(target_arch = "loongarch64")]
global_asm!(
    r#"
    .globl clone_shared_context_spawn
clone_shared_context_spawn:
    move $t0, $a2
    move $t1, $a3
    move $a2, $zero
    move $a3, $zero
    move $a4, $zero
    li.d $a7, 220
    syscall 0
    bnez $a0, 1f
    move $a0, $t1
    jirl $ra, $t0, 0
    li.d $a0, 99
    li.d $a7, 93
    syscall 0
1:
    jirl $zero, $ra, 0
"#
);

unsafe extern "C" {
    fn clone_shared_context_spawn(
        flags: usize,
        child_stack: usize,
        child_fn: extern "C" fn(usize) -> !,
        arg: usize,
    ) -> isize;
}

extern "C" fn child_main(_: usize) -> ! {
    let mapped = mmap(
        0,
        PAGE_SIZE,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0,
    );
    if mapped < 0 {
        exit(10);
    }
    unsafe { write_volatile(mapped as *mut usize, MAGIC) };
    NEW_MAPPING.store(mapped as usize, Ordering::Release);

    let fd = SHARED_FD.load(Ordering::Acquire);
    if fd < 0 || close(fd as usize) < 0 {
        exit(11);
    }
    if chdir("/clone_shared_context_dir") < 0 {
        exit(12);
    }
    let ignore = SigAction::ignore();
    if sigaction(SIGUSR1, Some(&ignore), None) < 0 {
        exit(13);
    }
    exit(0)
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[clone_shared_context_test] start");
    let _ = mkdir("/clone_shared_context_dir", 0o755);
    let fd = open(
        -100,
        "/clone_shared_context_file",
        OpenFlags::O_CREAT | OpenFlags::RDWR,
        0o644,
    );
    if fd < 0 {
        println!("[clone_shared_context_test] FAIL: open={}", fd);
        return 1;
    }
    SHARED_FD.store(fd, Ordering::Release);

    let stack_top = unsafe {
        (addr_of_mut!(CHILD_STACK.0) as *mut u8 as usize) + core::mem::size_of::<ChildStack>()
    };
    let flags = CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_VFORK | SIGCHLD;
    let child = unsafe { clone_shared_context_spawn(flags, stack_top, child_main, 0) };
    if child <= 0 {
        println!("[clone_shared_context_test] FAIL: clone={}", child);
        return 1;
    }

    let mut status = -1;
    let waited = waitpid(child as usize, &mut status);
    let mapping = NEW_MAPPING.load(Ordering::Acquire);
    let vm_shared = mapping != 0 && unsafe { read_volatile(mapping as *const usize) } == MAGIC;
    let files_shared = close(fd as usize) == -9;

    let mut cwd = [0u8; 128];
    let cwd_ret = getcwd(&mut cwd, 128);
    let expected = b"/clone_shared_context_dir\0";
    let fs_shared = cwd_ret > 0 && cwd[..expected.len()] == expected[..];

    let mut action = SigAction::default();
    let sighand_ret = sigaction(SIGUSR1, None, Some(&mut action));
    let sighand_shared = sighand_ret == 0 && matches!(action.sa_handler, SigHandler::Ignore);

    println!(
        "[clone_shared_context_test] waited={} status={} vm={} files={} fs={} sighand={}",
        waited, status, vm_shared, files_shared, fs_shared, sighand_shared
    );
    if waited == child && status == 0 && vm_shared && files_shared && fs_shared && sighand_shared {
        println!("[clone_shared_context_test] PASS");
        0
    } else {
        println!("[clone_shared_context_test] FAIL");
        1
    }
}
