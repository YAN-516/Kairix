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
const SYSCALL_GETCWD: usize = 17;
const SYSCALL_DUP: usize = 23;
const SYSCALL_DUP2: usize = 24;
const SYSCALL_FCNTL: usize = 25;
const SYSCALL_IOCTL: usize = 29;
const SYSCALL_MKDIR: usize = 34;
const SYSCALL_UNLINKAT: usize = 35;
const SYSCALL_LINKAT: usize = 37;
const SYSCALL_UMOUNT2: usize = 39;
const SYSCALL_MOUNT: usize = 40;
const SYSCALL_STATFS: usize = 43;
const SYSCALL_FACCESSAT: usize = 48;

const SYSCALL_CHDIR: usize = 49;
const SYSCALL_OPENAT: usize = 56;
const SYSCALL_CLOSE: usize = 57;
const SYSCALL_PIPE: usize = 59;
const SYSCALL_GETDENTS: usize = 61;
const SYSCALL_READ: usize = 63;
const SYSCALL_WRITE: usize = 64;
const SYSCALL_WRITEV: usize = 66;
const SYSCALL_SENDFILE: usize = 71;
const SYSCALL_PPOLL: usize = 73;
const SYSCALL_FSTATAT: usize = 79;
const SYSCALL_FSTAT: usize = 80;
const SYSCALL_FSYNC: usize = 82;
const SYSCALL_UTIMENSAT: usize = 88;
const SYSCALL_EXIT: usize = 93;
const SYSCALL_EXIT_GROUP: usize = 94;
const SYSCALL_SET_TID_ADDRESS: usize = 96;
const SYSCALL_SLEEP: usize = 101;
const SYSCALL_CLOCK_GETTIME: usize = 113;
const SYSCALL_SYSLOG: usize = 116;
const SYSCALL_YIELD: usize = 124;
const SYSCALL_KILL: usize = 129;
const SYSCALL_RT_SIGACTION: usize = 134;
const SYSCALL_RT_SIGPROCMASK: usize = 135;
const SYS_TIMES: usize = 153;
const SYSCALL_SETPGID: usize = 154;
const SYSCALL_GETPGID: usize = 155;
const SYSCALL_GETPGRP: usize = 158;
const SYSCALL_UNAME: usize = 160;
const SYSCALL_GET_TIME: usize = 169;
const SYSCALL_GETPID: usize = 172;
const SYSCALL_GETPPID: usize = 173;
const SYSCALL_GETUID: usize = 174;
const SYSCALL_GETEUID: usize = 175;
const SYSCALL_SETPGRP: usize = 176;
const SYSCALL_GETTID: usize = 178;
const SYSCALL_SYSINFO: usize = 179;
const SYSCALL_MUNMAP: usize = 215;
const SYSCALL_FORK: usize = 220;
const SYSCALL_EXECVE: usize = 221;
const SYSCALL_MMAP: usize = 222;
const SYSCALL_MPROTECT: usize = 226;
const SYSCALL_WAITPID: usize = 260;
const SYSCALL_THREAD_CREATE: usize = 1000;
const SYSCALL_WAITTID: usize = 1002;
const SYSCALL_BRK: usize = 214;
const SYSCALL_MADVICE: usize = 233;
const SYSCALL_SOCKET: usize = 198;
const SYSCALL_BIND: usize = 200;
const SYSCALL_SENDTO: usize = 206;
const SYSCALL_RECVFROM: usize = 207;
mod fs;
mod info;
mod mm;
///
pub mod net;
mod pipe;
mod process;
mod signal;
mod thread;
mod time;
mod misc;
use crate::{
    syscall::thread::{sys_thread_create, sys_waittid},
    task::Tms,
};
use fs::*;
use info::*;
use log::info;
use mm::*;
use net::*;
use pipe::*;
use process::*;
use signal::*;
use thread::*;
use time::*;
use misc::*;
//const SIGCHLD: usize = 17;

/// handle syscall exception with `syscall_id` and other arguments
pub fn syscall(syscall_id: usize, args: [usize; 6]) -> isize {

    // info!("[SYSCALL] id: {}, args: {:?}", syscall_id, args);
    if syscall_id == SYSCALL_WAITPID {
        loop {
            match sys_waitpid(args[0] as isize, args[1] as *mut i32) {
                -2 => {
                    //println!("wait and yield");
                    sys_yield();
                }
                exit_pid => {
                    return exit_pid;
                }
            }
        }
    }

    if syscall_id == SYSCALL_WAITTID {
        loop {
            match sys_waittid(args[0]) {
                -2 => {
                    sys_yield();
                }
                exit_pid => {
                    return exit_pid as isize;
                }
            }
        }
    }

    match syscall_id {
        SYSCALL_GETCWD => sys_getcwd(args[0] as *const u8, args[1]),
        SYSCALL_CHDIR => sys_chdir(args[0] as *const u8),
        SYSCALL_UNLINKAT => sys_unlinkat(args[0] as isize, args[1] as *const u8, args[2] as u32),
        SYSCALL_MKDIR => sys_mkdirat(args[0] as isize, args[1] as *const u8, args[2] as u32),
        SYSCALL_LINKAT => sys_linkat(
            args[0] as isize,
            args[1] as *const u8,
            args[2] as isize,
            args[3] as *const u8,
            args[4] as u32,
        ),
        SYSCALL_UMOUNT2 => sys_umount2(args[0] as *const u8, args[1] as u32),
        SYSCALL_MOUNT => sys_mount(
            args[0] as *const u8,
            args[1] as *const u8,
            args[2] as *const u8,
            args[3],
            args[4] as *const u8,
        ),
        SYSCALL_FACCESSAT => sys_faccessat(args[0] as isize, args[1] as *const u8, args[2] as u32, args[3] as u32),
        SYSCALL_OPENAT => sys_openat(args[0] as isize, args[1] as *const u8, args[2] as u32),
        SYSCALL_CLOSE => sys_close(args[0]),
        SYSCALL_GETDENTS => sys_getdents64(args[0], args[1] as *mut u8, args[2]),
        SYSCALL_READ => sys_read(args[0], args[1] as *const u8, args[2]),
        SYSCALL_WRITE => sys_write(args[0], args[1] as *const u8, args[2]),
        SYSCALL_FSTATAT => sys_fstatat(
            args[0] as isize,
            args[1] as *const u8,
            args[2] as *mut u8,
            args[3] as u32,
        ),
        SYSCALL_FSTAT => sys_fstat(args[0], args[1] as *mut u8),
        SYSCALL_FSYNC => sys_fsync(args[0]),
        SYSCALL_EXIT => sys_exit(args[0] as i32),
        SYSCALL_YIELD => sys_yield(),
        SYSCALL_KILL => sys_kill(args[0] as isize, args[1]),
        SYSCALL_UNAME => sys_uname(args[0] as *mut u8),
        SYSCALL_GET_TIME => sys_get_time(args[0] as *mut TimeVal, args[1]),
        SYSCALL_GETPID => sys_getpid(),
        SYSCALL_GETPPID => sys_getppid(),
        SYSCALL_GETPGID => sys_getpgid(args[0] as i32),
        SYSCALL_GETPGRP => sys_getpgrp(),
        SYSCALL_MUNMAP => sys_munmap(args[0], args[1]),
        SYSCALL_EXECVE => sys_execve(args[0], args[1], args[2]),
        SYSCALL_MMAP => sys_mmap(args[0], args[1], args[2], args[3], args[4], args[5]),
        SYSCALL_WAITPID => sys_waitpid(args[0] as isize, args[1] as *mut i32),
        SYSCALL_FORK => {
            if args[1] == 0 {
                sys_fork()
            } else {
                sys_clone(args[0] as u32, args[1] as usize)
            }
        }
        SYS_TIMES => sys_times(args[0] as *mut Tms),
        SYSCALL_SLEEP => sys_sleep(args[0] as *mut TimeVal, args[1] as *mut TimeVal),
        SYSCALL_DUP => sys_dup(args[0]),
        SYSCALL_DUP2 => sys_dup2(args[0], args[1]),
        SYSCALL_PIPE => sys_pipe(args[0] as *mut i32),
        SYSCALL_THREAD_CREATE => sys_thread_create(args[0], args[1]),
        SYSCALL_BRK => sys_brk(args[0] as *const i32),
        SYSCALL_SET_TID_ADDRESS => sys_set_tid_address(args[0]),
        SYSCALL_GETUID => sys_getuid(),
        SYSCALL_IOCTL => sys_ioctl(args[0], args[1], args[2]),
        SYSCALL_EXIT_GROUP => sys_exit_group(args[0] as i32),
        SYSCALL_RT_SIGACTION => sys_sigaction(args[0], args[1], args[2], args[3]),
        SYSCALL_RT_SIGPROCMASK => sys_sigprocmask(args[0], args[1], args[2], args[3]),
        SYSCALL_FCNTL => sys_fcntl(args[0], args[1], args[2]),
        SYSCALL_WRITEV => sys_writev(args[0], args[1], args[2]),
        SYSCALL_SETPGID => sys_setpgid(args[0] as i32, args[1] as i32),
        SYSCALL_SETPGRP => sys_setpgrp(),
        SYSCALL_PPOLL => sys_ppoll(args[0], args[1], args[2], args[3]),
        SYSCALL_GETTID => sys_gettid(),
        SYSCALL_SYSINFO => sys_sysinfo(args[0] as *mut SysInfo),
        SYSCALL_SOCKET => sys_socket(args[0] as i32, args[1] as i32, args[2] as i32),
        SYSCALL_SENDTO => sys_sendto(
            args[0],
            args[1] as *const u8,
            args[2],
            args[3] as i32,
            args[4] as *const u8,
            args[5],
        ),
        SYSCALL_RECVFROM => sys_recvfrom(
            args[0],
            args[1] as *mut u8,
            args[2],
            args[3] as i32,
            args[4] as *mut u8,
            args[5] as *mut usize,
        ),
        SYSCALL_BIND => sys_bind(args[0], args[1] as *const u8, args[2]),
        SYSCALL_CLOCK_GETTIME => sys_clock_gettime(args[0], args[1] as *mut NanoTimeVal),
        SYSCALL_MADVICE => sys_madvice(args[0]),
        SYSCALL_MPROTECT => sys_mprotect(args[0], args[1], args[2]),
        SYSCALL_GETEUID => sys_geteuid(),
        SYSCALL_SENDFILE => sys_sendfile(args[0], args[1], args[2], args[3]),
        SYSCALL_SYSLOG => sys_syslog(args[0], args[1] , args[2]),
        SYSCALL_STATFS => sys_statfs(args[0] as *const u8, args[1] as *mut u8),
        SYSCALL_UTIMENSAT => sys_utimensat(args[0] as isize, args[1] as *const u8,args[2] as *const Timespec,args[3] as i32,),
        _ => panic!("Unsupported syscall_id: {}", syscall_id),
    }
}
