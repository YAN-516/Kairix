#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::arch::global_asm;
use core::hint::black_box;
use core::ptr::{read_volatile, write_volatile};
use user_lib::{close, pipe, read, waitpid};

const ROUNDS: usize = 128;
const CHILD_EXIT_CODE: i32 = 127;
const ENOENT: isize = -2;
const CHILD_STACK_SIZE: usize = 32 * 1024;
const MISSING_PATH: &[u8] = b"/__kairix_vfork_exec_missing__/emcc\0";
const CLONE_VM: u64 = 0x0000_0100;
const CLONE_VFORK: u64 = 0x0000_4000;
const CLONE_CLEAR_SIGHAND: u64 = 0x1_0000_0000;

#[repr(C, align(16))]
struct ChildStack([u8; CHILD_STACK_SIZE]);

#[repr(C)]
#[derive(Default)]
struct VforkReport {
    sp_before: usize,
    sp_after: usize,
    registers_preserved: usize,
    exec_result: isize,
    pipe_write_result: isize,
}

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

static mut CHILD_STACK: ChildStack = ChildStack([0; CHILD_STACK_SIZE]);

#[cfg(target_arch = "riscv64")]
global_asm!(
    r#"
    .section .text
    .globl vfork_exec_failure_raw
vfork_exec_failure_raw:
    addi sp, sp, -112
    sd s0, 0(sp)
    sd s1, 8(sp)
    sd s2, 16(sp)
    sd s3, 24(sp)
    sd s4, 32(sp)
    sd ra, 40(sp)
    sd a0, 56(sp)
    sd a1, 64(sp)
    sd a2, 72(sp)
    sd a5, 80(sp)
    sd a6, 88(sp)

    addi t0, sp, 112
    sd t0, 0(a5)
    mv s0, a0
    mv s1, a1
    mv s2, a2
    mv s3, a5
    mv s4, a6
    mv a0, a3
    mv a1, a4
    li a7, 435
    ecall
    beqz a0, 2f

    addi t0, sp, 112
    sd t0, 8(s3)
    ld t2, 56(sp)
    bne s0, t2, 3f
    ld t2, 64(sp)
    bne s1, t2, 3f
    ld t2, 72(sp)
    bne s2, t2, 3f
    ld t2, 80(sp)
    bne s3, t2, 3f
    ld t2, 88(sp)
    bne s4, t2, 3f
    li t1, 1
    j 4f
3:
    li t1, 0
4:
    sd t1, 16(s3)
    ld s0, 0(sp)
    ld s1, 8(sp)
    ld s2, 16(sp)
    ld s3, 24(sp)
    ld s4, 32(sp)
    ld ra, 40(sp)
    addi sp, sp, 112
    ret

2:
    mv a0, s0
    mv a1, s1
    mv a2, s2
    li a7, 221
    ecall
    sd a0, 24(s3)
    mv a0, s4
    addi a1, s3, 24
    li a2, 8
    li a7, 64
    ecall
    sd a0, 32(s3)
    li a0, 127
    li a7, 94
    ecall
5:
    j 5b
"#
);

#[cfg(target_arch = "loongarch64")]
global_asm!(
    r#"
    .section .text
    .globl vfork_exec_failure_raw
vfork_exec_failure_raw:
    addi.d $sp, $sp, -112
    st.d $s0, $sp, 0
    st.d $s1, $sp, 8
    st.d $s2, $sp, 16
    st.d $s3, $sp, 24
    st.d $s4, $sp, 32
    st.d $ra, $sp, 40
    st.d $a0, $sp, 56
    st.d $a1, $sp, 64
    st.d $a2, $sp, 72
    st.d $a5, $sp, 80
    st.d $a6, $sp, 88

    addi.d $t0, $sp, 112
    st.d $t0, $a5, 0
    move $s0, $a0
    move $s1, $a1
    move $s2, $a2
    move $s3, $a5
    move $s4, $a6
    move $a0, $a3
    move $a1, $a4
    li.d $a7, 435
    syscall 0
    beqz $a0, 2f

    addi.d $t0, $sp, 112
    st.d $t0, $s3, 8
    ld.d $t2, $sp, 56
    bne $s0, $t2, 3f
    ld.d $t2, $sp, 64
    bne $s1, $t2, 3f
    ld.d $t2, $sp, 72
    bne $s2, $t2, 3f
    ld.d $t2, $sp, 80
    bne $s3, $t2, 3f
    ld.d $t2, $sp, 88
    bne $s4, $t2, 3f
    li.d $t1, 1
    b 4f
3:
    move $t1, $zero
4:
    st.d $t1, $s3, 16
    ld.d $s0, $sp, 0
    ld.d $s1, $sp, 8
    ld.d $s2, $sp, 16
    ld.d $s3, $sp, 24
    ld.d $s4, $sp, 32
    ld.d $ra, $sp, 40
    addi.d $sp, $sp, 112
    jr $ra

2:
    move $a0, $s0
    move $a1, $s1
    move $a2, $s2
    li.d $a7, 221
    syscall 0
    st.d $a0, $s3, 24
    move $a0, $s4
    addi.d $a1, $s3, 24
    li.d $a2, 8
    li.d $a7, 64
    syscall 0
    st.d $a0, $s3, 32
    li.d $a0, 127
    li.d $a7, 94
    syscall 0
5:
    b 5b
"#
);

unsafe extern "C" {
    fn vfork_exec_failure_raw(
        path: *const u8,
        argv: *const usize,
        envp: *const usize,
        clone_args: *const CloneArgs,
        clone_args_size: usize,
        report: *mut VforkReport,
        error_pipe_fd: usize,
    ) -> isize;
}

fn read_exact(fd: usize, output: &mut [u8]) -> bool {
    let mut done = 0usize;
    while done < output.len() {
        let count = read(fd, &mut output[done..]);
        if count <= 0 {
            return false;
        }
        done += count as usize;
    }
    true
}

fn guard_value(round: usize, index: usize) -> usize {
    0x5646_4f52_4b45_4e4fu64
        .wrapping_add((round as u64) << 16)
        .wrapping_add(index as u64) as usize
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[vfork_exec_failure_test] start rounds={}", ROUNDS);
    let argv = [MISSING_PATH.as_ptr() as usize, 0];
    let envp = [0usize];
    let child_stack = unsafe { core::ptr::addr_of_mut!(CHILD_STACK.0).cast::<u8>() as usize };
    let clone_args = CloneArgs {
        // This matches the modern glibc posix_spawn clone3 path used by
        // std::process::Command, including the clone3-only CLEAR_SIGHAND bit.
        flags: CLONE_VM | CLONE_VFORK | CLONE_CLEAR_SIGHAND,
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

    for round in 0..ROUNDS {
        let mut guards = [0usize; 8];
        for (index, guard) in guards.iter_mut().enumerate() {
            unsafe { write_volatile(guard, guard_value(round, index)) };
        }
        black_box(&mut guards);

        let mut pipe_fds = [-1i32; 2];
        if pipe(&mut pipe_fds) != 0 {
            println!("[vfork_exec_failure_test] FAIL stage=pipe round={}", round);
            return 1;
        }
        let mut report = VforkReport::default();
        let child = unsafe {
            vfork_exec_failure_raw(
                MISSING_PATH.as_ptr(),
                argv.as_ptr(),
                envp.as_ptr(),
                &clone_args,
                core::mem::size_of::<CloneArgs>(),
                &mut report,
                pipe_fds[1] as usize,
            )
        };
        let close_write = close(pipe_fds[1] as usize);
        if child <= 0 {
            let _ = close(pipe_fds[0] as usize);
            println!(
                "[vfork_exec_failure_test] FAIL stage=clone round={} child={} close_write={}",
                round, child, close_write
            );
            return 2;
        }

        let mut error_bytes = [0u8; core::mem::size_of::<isize>()];
        let pipe_ok = read_exact(pipe_fds[0] as usize, &mut error_bytes);
        let close_read = close(pipe_fds[0] as usize);
        let pipe_exec_result = isize::from_ne_bytes(error_bytes);
        let mut status = -1i32;
        let waited = waitpid(child as usize, &mut status);
        let guard_ok = guards
            .iter()
            .enumerate()
            .all(|(index, guard)| unsafe { read_volatile(guard) == guard_value(round, index) });
        let status_ok =
            waited == child && status & 0x7f == 0 && ((status >> 8) & 0xff) == CHILD_EXIT_CODE;
        let context_ok = report.sp_before == report.sp_after && report.registers_preserved == 1;
        let exec_ok = report.exec_result == ENOENT
            && report.pipe_write_result == core::mem::size_of::<isize>() as isize
            && pipe_exec_result == ENOENT;
        if close_write != 0
            || close_read != 0
            || !pipe_ok
            || !status_ok
            || !context_ok
            || !exec_ok
            || !guard_ok
        {
            println!(
                "[vfork_exec_failure_test] FAIL round={} child={} waited={} status={:#x} close_write={} close_read={} pipe_ok={} pipe_exec={} exec_result={} pipe_write={} sp_before={:#x} sp_after={:#x} regs={} guards={}",
                round,
                child,
                waited,
                status,
                close_write,
                close_read,
                pipe_ok,
                pipe_exec_result,
                report.exec_result,
                report.pipe_write_result,
                report.sp_before,
                report.sp_after,
                report.registers_preserved,
                guard_ok,
            );
            return 3;
        }
        if (round + 1) % 8 == 0 {
            println!(
                "[vfork_exec_failure_test] progress={}/{}",
                round + 1,
                ROUNDS
            );
        }
    }

    println!("[vfork_exec_failure_test] PASS rounds={}", ROUNDS);
    0
}
