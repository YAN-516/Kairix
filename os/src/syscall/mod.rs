//! Implementation of syscalls
//!
//! The single entry point to all system calls, [`syscall()`], is called
//! whenever userspace wishes to perform a system call using the `ecall`
//! instruction. In this case, the processor raises an 'Environment call from
//! U-mode' exception, which is handled as one of the cases in
//! [`crate::trap::trap_handler`].
//!
//! For clarity, each single syscall is implemented as its own function, named
//! `sys_` then the name of the syscall. You can find functions like this in
//! submodules, and you should also implement syscalls this way.
const SYSCALL_DUP: usize = 23;
const SYSCALL_DUP2: usize = 24;
const SYSCALL_OPEN: usize = 56;
const SYSCALL_CLOSE: usize = 57;
const SYSCALL_PIPE: usize = 59;
const SYSCALL_READ: usize = 63;
const SYSCALL_WRITE: usize = 64;
const SYSCALL_EXIT: usize = 93;
const SYSCALL_SLEEP: usize = 101;
const SYSCALL_YIELD: usize = 124;
//const SYSCALL_KILL: usize = 129;
const SYS_TIMES: usize = 153;
const SYSCALL_GET_TIME: usize = 169;
const SYSCALL_GETPID: usize = 172;
const SYSCALL_GETPPID: usize = 173;
const SYSCALL_FORK: usize = 220;
const SYSCALL_EXEC: usize = 221;
const SYSCALL_WAITPID: usize = 260;

// const SYSCALL_WAITTID: usize = 1002;
// const SYSCALL_THREAD_CREATE: usize = 1000;

mod fs;
mod pipe;
mod process;
mod time;

use crate::task::Tms;
use fs::*;
use pipe::*;
use process::*;
use time::*;

/// handle syscall exception with `syscall_id` and other arguments
pub fn syscall(syscall_id: usize, args: [usize; 3]) -> isize {
    if syscall_id == SYSCALL_WAITPID {
        loop {
            match sys_waitpid(args[0] as isize, args[1] as *mut i32) {
                -2 => {
                    sys_yield();
                }
                exit_pid => {
                    return exit_pid;
                }
            }
        }
    }
    match syscall_id {
        SYSCALL_OPEN => sys_open(args[0] as *const u8, args[1] as u32),
        SYSCALL_CLOSE => sys_close(args[0]),
        SYSCALL_READ => sys_read(args[0], args[1] as *const u8, args[2]),
        SYSCALL_WRITE => sys_write(args[0], args[1] as *const u8, args[2]),
        SYSCALL_EXIT => sys_exit(args[0] as i32),
        SYSCALL_YIELD => sys_yield(),
        SYSCALL_GET_TIME => sys_get_time(args[0] as *mut TimeVal, args[1]),
        SYSCALL_GETPID => sys_getpid(),
        SYSCALL_GETPPID => sys_getppid(),
        SYSCALL_FORK => sys_fork(),
        SYSCALL_EXEC => sys_exec(args[0] as *const u8),
        SYS_TIMES => sys_times(args[0] as *mut Tms),
        SYSCALL_SLEEP => sys_sleep(args[0] as *mut TimeVal, args[1] as *mut TimeVal),
        SYSCALL_DUP => sys_dup(args[0]),
        SYSCALL_DUP2 => sys_dup2(args[0], args[1]),
        SYSCALL_PIPE => sys_pipe(args[0] as *mut i32),
        _ => panic!("Unsupported syscall_id: {}", syscall_id),
    }
}
