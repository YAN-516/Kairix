#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::arch::global_asm;
use core::ptr::addr_of_mut;
use user_lib::waitpid;

const ENV_COUNT: usize = 128;
const CHILD_OK: i32 = 42;
const CHILD_STACK_SIZE: usize = 16 * 1024;

static EXEC_PATH: &[u8] = b"/vfork_exec_env_target\0";
static ENV_VALUE: &[u8] =
    b"BUILDSTORM_VFORK_ENV=abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ\0";

#[repr(C, align(16))]
struct ChildStack([u8; CHILD_STACK_SIZE]);

static mut CHILD_STACK: ChildStack = ChildStack([0; CHILD_STACK_SIZE]);

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
    addi sp, sp, -32
    sd s0, 0(sp)
    sd s1, 8(sp)
    sd s2, 16(sp)
    sd ra, 24(sp)
    mv s0, a0
    mv s1, a1
    mv s2, a2
    mv a0, a3
    mv a1, a4
    li a7, 435
    ecall
    bnez a0, 1f

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
    ld ra, 24(sp)
    addi sp, sp, 32
    ret
"#
);

#[cfg(target_arch = "loongarch64")]
global_asm!(
    r#"
    .section .text
    .globl vfork_exec_raw
vfork_exec_raw:
    addi.d $sp, $sp, -32
    st.d $s0, $sp, 0
    st.d $s1, $sp, 8
    st.d $s2, $sp, 16
    st.d $ra, $sp, 24
    move $s0, $a0
    move $s1, $a1
    move $s2, $a2
    move $a0, $a3
    move $a1, $a4
    li.w $a7, 435
    syscall 0
    bnez $a0, 1f

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
    ld.d $ra, $sp, 24
    addi.d $sp, $sp, 32
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
    ) -> isize;
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[vfork_exec_env_test] start env_count={}", ENV_COUNT);

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
        )
    };
    if child < 0 {
        println!("[vfork_exec_env_test] FAIL: clone returned {}", child);
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
