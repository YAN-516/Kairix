#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::arch::global_asm;
use core::ptr::addr_of_mut;
use core::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use user_lib::{close, exit, fork, pipe, read, waitpid, write, yield_};

const ENV_COUNT: usize = 128;
const CHILD_OK: i32 = 42;
const CHILD_STACK_SIZE: usize = 16 * 1024;

static EXEC_PATH: &[u8] = b"/vfork_exec_env_target\0";
static ENV_VALUE: &[u8] =
    b"BUILDSTORM_VFORK_ENV=abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ\0";

#[repr(C, align(16))]
struct ChildStack([u8; CHILD_STACK_SIZE]);

static mut CHILD_STACK: ChildStack = ChildStack([0; CHILD_STACK_SIZE]);
static HELPER_WRITE_FD: AtomicI32 = AtomicI32::new(-1);
static BEFORE_EXEC_DONE: AtomicBool = AtomicBool::new(false);

#[repr(C)]
struct CloneArgs {
    flags: u64,
    pidfd: u64,
    child_tid: u64,
    parent_tid: u64,
    exit_signal: u64,
    stack: u64,
    stack_size: u64,
    tls: u64,
    set_tid: u64,
    set_tid_size: u64,
    cgroup: u64,
}

#[cfg(target_arch = "riscv64")]
global_asm!(
    r#"
    .section .text
    .globl vfork_exec_raw
vfork_exec_raw:
    addi sp, sp, -48
    sd s0, 0(sp)
    sd s1, 8(sp)
    sd s2, 16(sp)
    sd s3, 24(sp)
    sd ra, 32(sp)
    mv s0, a0
    mv s1, a1
    mv s2, a2
    mv s3, a5
    mv a0, a3
    mv a1, a4
    li a7, 435
    ecall
    bnez a0, 1f

    jalr s3
    mv a0, s0
    mv a1, s1
    mv a2, s2
    li a7, 221
    ecall
    li a0, 127
    li a7, 94
    ecall
0:
    j 0b

1:
    ld s0, 0(sp)
    ld s1, 8(sp)
    ld s2, 16(sp)
    ld s3, 24(sp)
    ld ra, 32(sp)
    addi sp, sp, 48
    ret
"#
);

#[cfg(target_arch = "loongarch64")]
global_asm!(
    r#"
    .section .text
    .globl vfork_exec_raw
vfork_exec_raw:
    addi.d $sp, $sp, -48
    st.d $s0, $sp, 0
    st.d $s1, $sp, 8
    st.d $s2, $sp, 16
    st.d $s3, $sp, 24
    st.d $ra, $sp, 32
    move $s0, $a0
    move $s1, $a1
    move $s2, $a2
    move $s3, $a5
    move $a0, $a3
    move $a1, $a4
    li.w $a7, 435
    syscall 0
    bnez $a0, 1f

    jirl $ra, $s3, 0
    move $a0, $s0
    move $a1, $s1
    move $a2, $s2
    li.w $a7, 221
    syscall 0
    li.w $a0, 127
    li.w $a7, 94
    syscall 0
0:
    b 0b

1:
    ld.d $s0, $sp, 0
    ld.d $s1, $sp, 8
    ld.d $s2, $sp, 16
    ld.d $s3, $sp, 24
    ld.d $ra, $sp, 32
    addi.d $sp, $sp, 48
    jr $ra
"#
);

unsafe extern "C" {
    fn vfork_exec_raw(
        path: *const u8,
        argv: *const usize,
        envp: *const usize,
        clone_args: *const CloneArgs,
        clone_args_size: usize,
        before_exec: extern "C" fn(),
    ) -> isize;
}

extern "C" fn before_exec() {
    let fd = HELPER_WRITE_FD.load(Ordering::Acquire);
    let byte = [0x5au8];
    if fd >= 0 {
        let _ = write(fd as usize, &byte);
    }
    // Give the helper enough opportunities to exit and deliver an unrelated
    // SIGCHLD while this vfork child has not reached execve yet.
    for _ in 0..4096 {
        let _ = yield_();
    }
    BEFORE_EXEC_DONE.store(true, Ordering::Release);
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[vfork_exec_env_test] start env_count={}", ENV_COUNT);

    let mut helper_pipe = [-1i32; 2];
    if pipe(&mut helper_pipe) != 0 {
        println!("[vfork_exec_env_test] FAIL: helper pipe");
        return 1;
    }
    HELPER_WRITE_FD.store(helper_pipe[1], Ordering::Release);
    BEFORE_EXEC_DONE.store(false, Ordering::Release);
    let helper = fork();
    if helper == 0 {
        let _ = close(helper_pipe[1] as usize);
        let mut byte = [0u8; 1];
        let received = read(helper_pipe[0] as usize, &mut byte);
        let _ = close(helper_pipe[0] as usize);
        exit(if received == 1 && byte[0] == 0x5a {
            0
        } else {
            2
        });
    }
    if helper < 0 {
        println!("[vfork_exec_env_test] FAIL: helper fork={}", helper);
        return 1;
    }
    let _ = close(helper_pipe[0] as usize);

    let argv = [EXEC_PATH.as_ptr() as usize, 0];
    let mut envp = [ENV_VALUE.as_ptr() as usize; ENV_COUNT + 1];
    envp[ENV_COUNT] = 0;
    let child_stack = unsafe { addr_of_mut!(CHILD_STACK.0).cast::<u8>() as usize };
    let clone_args = CloneArgs {
        flags: 0x4100,
        pidfd: 0,
        child_tid: 0,
        parent_tid: 0,
        exit_signal: 17,
        stack: child_stack as u64,
        stack_size: CHILD_STACK_SIZE as u64,
        tls: 0,
        set_tid: 0,
        set_tid_size: 0,
        cgroup: 0,
    };

    let child = unsafe {
        vfork_exec_raw(
            EXEC_PATH.as_ptr(),
            argv.as_ptr(),
            envp.as_ptr(),
            &clone_args,
            core::mem::size_of::<CloneArgs>(),
            before_exec,
        )
    };
    if child < 0 {
        println!("[vfork_exec_env_test] FAIL: clone returned {}", child);
        return 1;
    }
    let _ = close(helper_pipe[1] as usize);
    HELPER_WRITE_FD.store(-1, Ordering::Release);
    if !BEFORE_EXEC_DONE.load(Ordering::Acquire) {
        println!(
            "[vfork_exec_env_test] FAIL: parent resumed before child exec child={}",
            child
        );
        return 1;
    }

    let mut helper_status = 0i32;
    let helper_waited = waitpid(helper as usize, &mut helper_status);
    if helper_waited != helper || helper_status != 0 {
        println!(
            "[vfork_exec_env_test] FAIL: helper={} waited={} status={}",
            helper, helper_waited, helper_status
        );
        return 1;
    }

    let mut status = 0i32;
    let waited = waitpid(child as usize, &mut status);
    let exit_code = (status >> 8) & 0xff;
    if waited == child && status & 0x7f == 0 && exit_code == CHILD_OK {
        println!("[vfork_exec_env_test] PASS child={}", child);
        0
    } else {
        println!(
            "[vfork_exec_env_test] FAIL: child={} waited={} status={} exit={}",
            child, waited, status, exit_code
        );
        1
    }
}
