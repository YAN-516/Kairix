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
use crate::current_task;
const SYSCALL_GETCWD: usize = 17;
const SYSCALL_EVENTFD2: usize = 19;
const SYSCALL_EPOLL_CREATE1: usize = 20;
const SYSCALL_DUP: usize = 23;
const SYSCALL_DUP2: usize = 24;
const SYSCALL_FCNTL: usize = 25;
const SYSCALL_INOTIFY_INIT1: usize = 26;
const SYSCALL_INOTIFY_ADD_WATCH: usize = 27;
const SYSCALL_INOTIFY_RM_WATCH: usize = 28;
const SYSCALL_IOCTL: usize = 29;
const SYSCALL_MKNODAT: usize = 33;
const SYSCALL_MKDIR: usize = 34;
const SYSCALL_UNLINKAT: usize = 35;
const SYSCALL_SYMLINKAT: usize = 36;
const SYSCALL_LINKAT: usize = 37;
const SYSCALL_UMOUNT2: usize = 39;
const SYSCALL_MOUNT: usize = 40;
const SYSCALL_STATFS: usize = 43;
const SYSCALL_TRUNCATE: usize = 45;
const SYSCALL_FTRUNCATE: usize = 46;
const SYSCALL_FALLOCATE: usize = 47;
const SYSCALL_FACCESSAT: usize = 48;
const SYSCALL_CHDIR: usize = 49;
const SYSCALL_FCHMODAT: usize = 53;
const SYSCALL_FCHOWNAT: usize = 54;
const SYSCALL_OPENAT: usize = 56;
const SYSCALL_CLOSE: usize = 57;
const SYSCALL_PIPE: usize = 59;
const SYSCALL_GETDENTS: usize = 61;
const SYSCALL_LSEEK: usize = 62;
const SYSCALL_READ: usize = 63;
const SYSCALL_WRITE: usize = 64;
const SYSCALL_READV: usize = 65;
const SYSCALL_WRITEV: usize = 66;
const SYSCALL_PREAD64: usize = 67;
const SYSCALL_PWRITE64: usize = 68;
const SYSCALL_SENDFILE: usize = 71;
const SYSCALL_PSELECT6: usize = 72;
const SYSCALL_PPOLL: usize = 73;
const SYSCALL_SIGNALFD4: usize = 74;
const SYSCALL_SPLICE: usize = 76;
const SYSCALL_READLINKAT: usize = 78;
const SYSCALL_FSTATAT: usize = 79;
const SYSCALL_FSTAT: usize = 80;
const SYSCALL_SYNC: usize = 81;
const SYSCALL_FSYNC: usize = 82;
const SYSCALL_FDATASYNC: usize = 83;
const SYSCALL_SYNC_FILE_RANGE: usize = 84;
const SYSCALL_UTIMENSAT: usize = 88;
const SYSCALL_CAPGET: usize = 90;
const SYSCALL_CAPSET: usize = 91;
const SYSCALL_EXIT: usize = 93;
const SYSCALL_EXIT_GROUP: usize = 94;
const SYSCALL_WAITID: usize = 95;
const SYSCALL_SET_TID_ADDRESS: usize = 96;
const SYSCALL_FUTEX: usize = 98;
const SYSCALL_SET_ROBUST_LIST: usize = 99;
const SYSCALL_GET_ROBUST_LIST: usize = 100;
const SYSCALL_SLEEP: usize = 101;
const SYSCALL_GETITIMER: usize = 102;
const SYSCALL_SETITIMER: usize = 103;
const SYSCALL_CLOCK_GETTIME: usize = 113;
const SYSCALL_CLOCK_NANOSLEEP: usize = 115;
const SYSCALL_SYSLOG: usize = 116;
const SYSCALL_YIELD: usize = 124;
const SYSCALL_KILL: usize = 129;
const SYSCALL_TKILL: usize = 130;
const SYSCALL_TGKILL: usize = 131;
const SYSCALL_RT_SIGSUSPEND: usize = 133;
const SYSCALL_RT_SIGACTION: usize = 134;
const SYSCALL_RT_SIGPROCMASK: usize = 135;
const SYSCALL_RT_SIGTIMEDWAIT: usize = 137;
const SYSCALL_RT_SIGRETURN: usize = 139;
const SYS_TIMES: usize = 153;
const SYSCALL_SETPGID: usize = 154;
const SYSCALL_GETPGID: usize = 155;
//const SYSCALL_SETSID: usize = 157;
const SYSCALL_GETPGRP: usize = 158;
const SYSCALL_UNAME: usize = 160;
const SYSCALL_GETRUSAGE: usize = 165;
const SYSCALL_UMASK: usize = 166;
const SYSCALL_GET_TIME: usize = 169;
const SYSCALL_GETPID: usize = 172;
const SYSCALL_GETPPID: usize = 173;
const SYSCALL_SETGID: usize = 144;
const SYSCALL_SETUID: usize = 146;
const SYSCALL_SETREUID: usize = 145;
const SYSCALL_SETRESUID: usize = 147;
const SYSCALL_SETRESGID: usize = 149;
const SYSCALL_GETUID: usize = 174;
const SYSCALL_GETEUID: usize = 175;
const SYSCALL_GETGID: usize = 176;
const SYSCALL_GETEGID: usize = 177;
const SYSCALL_GETTID: usize = 178;
const SYSCALL_SYSINFO: usize = 179;
const SYSCALL_SHMGET: usize = 194;
const SYSCALL_SHMCTL: usize = 195;
const SYSCALL_SHMAT: usize = 196;
const SYSCALL_SHMDT: usize = 197;
const SYSCALL_SOCKET: usize = 198;
const SYSCALL_BIND: usize = 200;
const SYSCALL_LISTEN: usize = 201;
const SYSCALL_ACCEPT: usize = 202;
const SYSCALL_CONNECT: usize = 203;
const SYSCALL_GETSOCKNAME: usize = 204;
const SYSCALL_GETPEERNAME: usize = 205;
const SYSCALL_SENDTO: usize = 206;
const SYSCALL_RECVFROM: usize = 207;
const SYSCALL_SETSOCKOPT: usize = 208;
const SYSCALL_GETSOCKOPT: usize = 209;
const SYSCALL_SHUTDOWN: usize = 210;
const SYSCALL_BRK: usize = 214;
const SYSCALL_MUNMAP: usize = 215;
const SYSCALL_FORK: usize = 220;
const SYSCALL_EXECVE: usize = 221;
const SYSCALL_MMAP: usize = 222;
const SYSCALL_MPROTECT: usize = 226;
const SYSCALL_MSYNC: usize = 227;
const SYSCALL_MADVICE: usize = 233;
const SYSCALL_PERF_EVENT_OPEN: usize = 241;
const SYSCALL_WAITPID: usize = 260;
const SYSCALL_PRLIMIT64: usize = 261;
const SYSCALL_FANOTIFY_INIT: usize = 262;
const SYSCALL_FANOTIFY_MARK: usize = 263;
const SYSCALL_NAME_TO_HANDLE_AT: usize = 264;
const SYSCALL_OPEN_BY_HANDLE_AT: usize = 265;
const SYSCALL_SYNCFS: usize = 267;
const SYSCALL_PRCTL: usize = 167;
const SYSCALL_RENAMEAT2: usize = 276;
const SYSCALL_GETRANDOM: usize = 278;
const SYSCALL_MEMFD_CREATE: usize = 279;
const SYSCALL_BPF: usize = 280;
const SYSCALL_USERFAULTFD: usize = 282;
const SYSCALL_COPY_FILE_RANGE: usize = 285;
const SYSCALL_MEMBARRIER: usize = 283;
const SYSCALL_STATX: usize = 291;
const SYSCALL_PIDFD_SEND_SIGNAL: usize = 424;
const SYSCALL_CLONE3: usize = 435;
const SYSCALL_IO_URING_SETUP: usize = 425;
const SYSCALL_OPEN_TREE: usize = 428;
const SYSCALL_MOVE_MOUNT: usize = 429;
const SYSCALL_FSOPEN: usize = 430;
const SYSCALL_FSCONFIG: usize = 431;
const SYSCALL_FSMOUNT: usize = 432;
const SYSCALL_FSPICK: usize = 433;
const SYSCALL_PIDFD_OPEN: usize = 434;
const SYSCALL_SETXATTR: usize = 5;
const SYSCALL_LSETXATTR: usize = 6;
const SYSCALL_FSETXATTR: usize = 7;
const SYSCALL_GETXATTR: usize = 8;
const SYSCALL_LGETXATTR: usize = 9;
const SYSCALL_FGETXATTR: usize = 10;
const SYSCALL_LISTXATTR: usize = 11;
const SYSCALL_LLISTXATTR: usize = 12;
const SYSCALL_FLISTXATTR: usize = 13;
const SYSCALL_REMOVEXATTR: usize = 14;
const SYSCALL_LREMOVEXATTR: usize = 15;
const SYSCALL_FREMOVEXATTR: usize = 16;
const SYSCALL_CLOSE_RANGE: usize = 436;
const SYSCALL_MOUNT_SETATTR: usize = 442;
const SYSCALL_THREAD_CREATE: usize = 1000;
const SYSCALL_WAITTID: usize = 1002;
const SYSCALL_GETRESUID: usize = 148;

const SYSCALL_SCHED_GETAFFINITY: usize = 123;
const SYSCALL_SCHED_SETAFFINITY: usize = 122;

const SYSCALL_SCHED_GETSCHEDULER: usize = 120;
const SYSCALL_SCHED_SETSCHEDULER: usize = 119;
const SYSCALL_SCHED_GETPARAM: usize = 121;
const SYSCALL_TIMERFD_CREATE: usize = 85;
const SYSCALL_TIMERFD_SETTIME: usize = 86;
const SYSCALL_TIMERFD_GETTIME: usize = 87;
const SYSCALL_CLOCK_GETRES: usize = 114;
const SYSCALL_SOCKETPAIR: usize = 199;
const SYSCALL_MLOCK: usize = 228;
const SYSCALL_MUNLOCK: usize = 229;
const SYSCALL_MEMFD_SECRET: usize = usize::MAX;
const SYSCALL_FCHMOD: usize = 52;

mod fs;
pub(crate) mod fanotify;
pub mod futex;
mod info;
pub(crate) mod inotify;
mod misc;
mod mm;
///
pub mod net;
mod pipe;
mod process;
pub mod shm;
/// Signal-related syscalls (sigaction, kill, sigprocmask, sigtimedwait, sigreturn, setitimer)
pub mod signal;
mod thread;
mod time;

pub(crate) use fs::maybe_update_atime;

use crate::{
    error::{SysError, SyscallResult},
    syscall::thread::{sys_thread_create, sys_waittid},
    task::Tms,
};
use fanotify::*;
use fs::*;
use futex::*;
use info::*;
use inotify::*;
use log::{error, info, trace};
use misc::*;
use mm::*;
use net::*;
use pipe::*;
use polyhal::println;
use process::*;
use shm::*;
use shm::*;
use signal::*;
use thread::*;
use time::*;
//const SIGCHLD: usize = 17;

/// handle syscall exception with `syscall_id` and other arguments
pub fn syscall(syscall_id: usize, args: [usize; 6]) -> SyscallResult {
    if syscall_id != 260 {
        info!("[SYSCALL] id: {}, args: {:?}", syscall_id, args);
    }
    //let pro = current_task().unwrap().process.upgrade().unwrap().getpid();
    // if pro == 4 {
    //     println!("!!!SYSCALL!!! id: {}", syscall_id);
    // }
    if syscall_id == SYSCALL_WAITTID {
        loop {
            match sys_waittid(args[0]) {
                Err(SysError::EAGAIN) => {
                    let _ = sys_yield()?;
                }
                other => return other,
            }
        }
    }
    // info!("SYSCALL: id={}, args={:?}", syscall_id, args);
    match syscall_id {
        SYSCALL_GETCWD => sys_getcwd(args[0] as *const u8, args[1]),
        SYSCALL_EVENTFD2 => sys_eventfd2(args[0], args[1] as i32),
        SYSCALL_EPOLL_CREATE1 => sys_epoll_create1(args[0] as i32),
        SYSCALL_CHDIR => sys_chdir(args[0] as *const u8),
        SYSCALL_FCHMODAT => sys_fchmodat(
            args[0] as isize,
            args[1] as *const u8,
            args[2] as u32,
            args[3] as i32,
        ),
        SYSCALL_FCHOWNAT => sys_fchownat(
            args[0] as isize,
            args[1] as *const u8,
            args[2] as u32,
            args[3] as u32,
            args[4] as i32,
        ),
        SYSCALL_UNLINKAT => sys_unlinkat(args[0] as isize, args[1] as *const u8, args[2] as u32),
        SYSCALL_MKNODAT => sys_mknodat(
            args[0] as isize,
            args[1] as *const u8,
            args[2] as u32,
            args[3] as u32,
        ),
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
        SYSCALL_FACCESSAT => sys_faccessat(
            args[0] as isize,
            args[1] as *const u8,
            args[2] as u32,
            args[3] as u32,
        ),
        SYSCALL_OPENAT => sys_openat(
            args[0] as isize,
            args[1] as *const u8,
            args[2] as u32,
            args[3] as u32,
        ),
        SYSCALL_CLOSE => sys_close(args[0]),
        SYSCALL_GETDENTS => sys_getdents64(args[0], args[1] as *mut u8, args[2]),
        SYSCALL_LSEEK => sys_lseek(args[0], args[1] as isize, args[2] as i32),
        SYSCALL_READ => sys_read(args[0], args[1] as *const u8, args[2]),
        SYSCALL_WRITE => sys_write(args[0], args[1] as *const u8, args[2]),
        SYSCALL_PREAD64 => sys_pread64(args[0], args[1] as *const u8, args[2], args[3]),
        SYSCALL_PWRITE64 => sys_pwrite64(args[0], args[1] as *const u8, args[2], args[3]),
        SYSCALL_SPLICE => sys_splice(args[0], args[1], args[2], args[3], args[4], args[5] as u32),
        SYSCALL_FSTATAT => sys_fstatat(
            args[0] as isize,
            args[1] as *const u8,
            args[2] as *mut u8,
            args[3] as u32,
        ),
        SYSCALL_FSTAT => sys_fstat(args[0], args[1] as *mut u8),
        SYSCALL_TRUNCATE => sys_truncate(args[0] as *const u8, args[1]),
        SYSCALL_FTRUNCATE => sys_ftruncate(args[0], args[1]),
        SYSCALL_FALLOCATE => sys_fallocate(args[0], args[1] as i32, args[2], args[3]),
        SYSCALL_SYNC => sys_sync(),
        SYSCALL_FDATASYNC => sys_fsync(args[0]),
        SYSCALL_SYNC_FILE_RANGE => {
            sys_sync_file_range(args[0], args[1] as i64, args[2] as i64, args[3] as u32)
        }
        SYSCALL_STATX => sys_statx(
            args[0] as isize,
            args[1] as *const u8,
            args[2] as u32,
            args[3],
            args[4] as *mut u8,
        ),
        SYSCALL_CLOSE_RANGE => sys_close_range(args[0], args[1], args[2] as u32),
        SYSCALL_FSYNC => sys_fsync(args[0]),
        SYSCALL_EXIT => {
            info!("sys_exit: code={}", args[0] as i32);
            sys_exit(args[0] as i32)
        }
        SYSCALL_YIELD => sys_yield(),
        SYSCALL_KILL => sys_kill(args[0] as isize, args[1]),
        SYSCALL_TKILL => sys_tkill(args[0] as isize, args[1]),
        SYSCALL_TGKILL => sys_tgkill(args[0] as isize, args[1] as isize, args[2]),
        SYSCALL_UNAME => sys_uname(args[0] as *mut u8),
        SYSCALL_GETRUSAGE => sys_getrusage(args[0] as i32, args[1] as *mut Rusage),
        SYSCALL_UMASK => sys_umask(args[0] as u32),
        SYSCALL_GET_TIME => sys_get_time(args[0] as *mut TimeVal, args[1]),
        SYSCALL_GETPID => sys_getpid(),
        SYSCALL_GETPPID => sys_getppid(),
        SYSCALL_GETPGID => sys_getpgid(args[0] as i32),
        SYSCALL_GETPGRP => sys_getpgrp(),
        SYSCALL_MUNMAP => sys_munmap(args[0], args[1]),
        SYSCALL_EXECVE => sys_execve(args[0], args[1], args[2]),
        SYSCALL_MMAP => sys_mmap(args[0], args[1], args[2], args[3], args[4], args[5]),
        SYSCALL_WAITPID => sys_wait4(args[0] as isize, args[1] as *mut i32, args[2] as i32, args[3] as *mut u8),
        SYSCALL_WAITID => sys_waitid(args[0] as i32, args[1] as u32, args[2] as *mut u8, args[3] as i32),
        SYSCALL_RT_SIGRETURN => {
            info!("SYSCALL_RT_SIGRETURN entered");
            sys_rt_sigreturn()
        }
        SYSCALL_SETITIMER => sys_setitimer(args[0], args[1], args[2]),
        SYSCALL_GETITIMER => sys_getitimer(args[0], args[1] as *mut Itimerval),

        SYSCALL_FORK => {
            // if args[1] == 0 {
            //     sys_fork()
            // } else {
            sys_clone(args[0] as u32, args[1] as usize, args[2], args[4], args[3])
            // }
        }
        SYSCALL_CLONE3 => sys_clone3(args[0] as *mut CloneArgs, args[1]),
        SYSCALL_PIDFD_SEND_SIGNAL => {
            sys_pidfd_send_signal(args[0] as i32, args[1] as i32, args[2], args[3] as u32)
        }
        SYS_TIMES => sys_times(args[0] as *mut Tms),
        SYSCALL_SLEEP => sys_sleep(args[0] as *mut NanoTimeVal, args[1] as *mut NanoTimeVal),
        SYSCALL_DUP => sys_dup(args[0]),
        SYSCALL_DUP2 => sys_dup3(args[0], args[1], args[2]),
        SYSCALL_PIPE => sys_pipe(args[0] as *mut i32),
        SYSCALL_THREAD_CREATE => sys_thread_create(args[0], args[1]),
        SYSCALL_BRK => sys_brk(args[0]),
        SYSCALL_SET_TID_ADDRESS => sys_set_tid_address(args[0]),
        SYSCALL_FUTEX => sys_futex(
            args[0] as *mut u32,
            args[1] as i32,
            args[2] as u32,
            args[3] as *const TimeSpec,
            args[4] as *mut u32,
            args[5] as u32,
        ),
        SYSCALL_SET_ROBUST_LIST => sys_set_robust_list(args[0], args[1]),
        SYSCALL_GET_ROBUST_LIST => {
            sys_get_robust_list(args[0], args[1] as *mut usize, args[2] as *mut usize)
        }
        SYSCALL_GETUID => sys_getuid(),
        SYSCALL_IOCTL => sys_ioctl(args[0], args[1], args[2]),
        SYSCALL_EXIT_GROUP => {
            info!("sys_exit_group: code={}", args[0] as i32);
            sys_exit_group(args[0] as i32)
        }
        SYSCALL_RT_SIGACTION => sys_sigaction(args[0], args[1], args[2], args[3]),
        SYSCALL_RT_SIGPROCMASK => sys_sigprocmask(args[0], args[1], args[2], args[3]),
        SYSCALL_RT_SIGTIMEDWAIT => sys_rt_sigtimedwait(args[0], args[1], args[2], args[3]),
        SYSCALL_FCNTL => sys_fcntl(args[0], args[1], args[2]),
        SYSCALL_READV => sys_readv(args[0], args[1], args[2]),
        SYSCALL_WRITEV => sys_writev(args[0], args[1], args[2]),
        SYSCALL_SETPGID => sys_setpgid(args[0] as i32, args[1] as i32),
        // SYSCALL_SETSID => sys_setsid(),
        SYSCALL_GETGID => sys_getgid(),
        SYSCALL_PSELECT6 => sys_pselect6(
            args[0],
            args[1] as *mut u64,
            args[2] as *mut u64,
            args[3] as *mut u64,
            args[4] as *mut Timespec,
            args[5] as *mut u8,
        ),
        SYSCALL_PPOLL => sys_ppoll(args[0], args[1], args[2], args[3]),
        SYSCALL_SIGNALFD4 => sys_signalfd4(args[0] as isize, args[1], args[2], args[3] as i32),
        SYSCALL_RT_SIGSUSPEND => sys_rt_sigsuspend(args[0], args[1]),
        SYSCALL_GETTID => sys_gettid(),
        SYSCALL_SYSINFO => sys_sysinfo(args[0] as *mut SysInfo),
        SYSCALL_SOCKET => sys_socket(args[0] as i32, args[1] as i32, args[2] as i32),
        SYSCALL_LISTEN => sys_listen(args[0], args[1]),
        SYSCALL_ACCEPT => sys_accept(args[0], args[1] as *mut u8, args[2] as *mut usize),
        SYSCALL_CONNECT => sys_connect(args[0], args[1] as *const u8, args[2]),
        SYSCALL_GETSOCKNAME => sys_getsockname(args[0], args[1] as *mut u8, args[2] as *mut usize),
        SYSCALL_GETPEERNAME => sys_getpeername(args[0], args[1] as *mut u8, args[2] as *mut usize),
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
        SYSCALL_SETSOCKOPT => sys_setsockopt(
            args[0],
            args[1] as i32,
            args[2] as i32,
            args[3] as *const u8,
            args[4],
        ),
        SYSCALL_GETSOCKOPT => sys_getsockopt(
            args[0],
            args[1] as i32,
            args[2] as i32,
            args[3] as *mut u8,
            args[4] as *mut usize,
        ),
        SYSCALL_BIND => sys_bind(args[0], args[1] as *const u8, args[2]),
        SYSCALL_SHUTDOWN => sys_shutdown(args[0], args[1] as i32),
        SYSCALL_CLOCK_GETTIME => sys_clock_gettime(args[0], args[1] as *mut NanoTimeVal),
        SYSCALL_CLOCK_NANOSLEEP => sys_clock_nanosleep(
            args[0],
            args[1],
            args[2] as *const TimeSpec,
            args[3] as *mut TimeSpec,
        ),
        SYSCALL_MADVICE => sys_madvice(args[0], args[1], args[2]),
        SYSCALL_MPROTECT => sys_mprotect(args[0], args[1], args[2]),
        SYSCALL_MSYNC => sys_msync(args[0], args[1], args[2]),
        SYSCALL_SETUID => sys_setuid(args[0] as u32),
        SYSCALL_SETREUID => sys_setreuid(args[0], args[1]),
        SYSCALL_SETGID => sys_setgid(args[0] as u32),
        SYSCALL_SETRESUID => sys_setresuid(args[0], args[1], args[2]),
        SYSCALL_SETRESGID => sys_setresgid(args[0], args[1], args[2]),
        SYSCALL_GETEUID => sys_geteuid(),
        SYSCALL_GETEGID => sys_getegid(),
        SYSCALL_SENDFILE => sys_sendfile(args[0], args[1], args[2], args[3]),
        SYSCALL_COPY_FILE_RANGE => {
            sys_copy_file_range(args[0], args[1], args[2], args[3], args[4], args[5])
        }
        SYSCALL_SYSLOG => sys_syslog(args[0], args[1], args[2]),
        SYSCALL_STATFS => sys_statfs(args[0] as *const u8, args[1] as *mut u8),
        SYSCALL_SYMLINKAT => {
            sys_symlinkat(args[0] as *const u8, args[1] as isize, args[2] as *const u8)
        }
        SYSCALL_READLINKAT => sys_readlinkat(
            args[0] as isize,
            args[1] as *const u8,
            args[2] as *mut u8,
            args[3],
        ),
        SYSCALL_UTIMENSAT => sys_utimensat(
            args[0] as isize,
            args[1] as *const u8,
            args[2] as *const Timespec,
            args[3] as i32,
        ),
        SYSCALL_CAPGET => sys_capget(args[0], args[1]),
        SYSCALL_CAPSET => sys_capset(args[0], args[1]),
        SYSCALL_PERF_EVENT_OPEN => sys_perf_event_open(
            args[0],
            args[1] as isize,
            args[2] as isize,
            args[3] as isize,
            args[4] as u32,
        ),
        SYSCALL_FANOTIFY_INIT => sys_fanotify_init(args[0] as u32, args[1] as u32),
        SYSCALL_FANOTIFY_MARK => sys_fanotify_mark(
            args[0],
            args[1] as u32,
            args[2] as u64,
            args[3] as isize,
            args[4] as *const u8,
        ),
        SYSCALL_NAME_TO_HANDLE_AT => sys_name_to_handle_at(
            args[0] as isize,
            args[1] as *const u8,
            args[2] as *mut FileHandleHeader,
            args[3] as *mut i32,
            args[4] as u32,
        ),
        SYSCALL_OPEN_BY_HANDLE_AT => sys_open_by_handle_at(
            args[0] as isize,
            args[1] as *const FileHandleHeader,
            args[2] as u32,
        ),
        SYSCALL_SYNCFS => sys_syncfs(args[0]),
        SYSCALL_RENAMEAT2 => sys_renameat2(
            args[0] as isize,
            args[1] as *const u8,
            args[2] as isize,
            args[3] as *const u8,
            args[4] as u32,
        ),
        SYSCALL_GETRANDOM => sys_getrandom(args[0] as *mut u8, args[1], args[2] as u32),
        SYSCALL_MEMFD_CREATE => sys_memfd_create(args[0] as *const u8, args[1] as u32),
        SYSCALL_BPF => sys_bpf(args[0] as u32, args[1], args[2] as u32),
        SYSCALL_USERFAULTFD => sys_userfaultfd(args[0] as i32),
        SYSCALL_IO_URING_SETUP => sys_io_uring_setup(args[0] as u32, args[1]),
        SYSCALL_OPEN_TREE => sys_open_tree(args[0] as isize, args[1] as *const u8, args[2] as u32),
        SYSCALL_MOVE_MOUNT => sys_move_mount(
            args[0] as isize,
            args[1] as *const u8,
            args[2] as isize,
            args[3] as *const u8,
            args[4] as u32,
        ),
        SYSCALL_FSOPEN => sys_fsopen(args[0] as *const u8, args[1] as u32),
        SYSCALL_FSCONFIG => sys_fsconfig(
            args[0],
            args[1] as u32,
            args[2] as *const u8,
            args[3] as *const u8,
            args[4] as i32,
        ),
        SYSCALL_FSMOUNT => sys_fsmount(args[0], args[1] as u32, args[2] as u32),
        SYSCALL_FSPICK => sys_fspick(args[0] as isize, args[1] as *const u8, args[2] as u32),
        SYSCALL_MOUNT_SETATTR => sys_mount_setattr(
            args[0] as isize,
            args[1] as *const u8,
            args[2] as u32,
            args[3] as *const MountAttr,
            args[4],
        ),
        SYSCALL_PIDFD_OPEN => sys_pidfd_open(args[0], args[1] as u32),
        SYSCALL_MEMFD_SECRET => sys_memfd_secret(args[0] as u32),
        SYSCALL_PRCTL => sys_prctl(args[0] as i32, args[1], args[2], args[3], args[4]),
        SYSCALL_PRLIMIT64 => sys_prlimit64(
            args[0],
            args[1] as i32,
            args[2] as *const u8,
            args[3] as *mut u8,
        ),
        SYSCALL_SHMGET => sys_shmget(args[0] as i32, args[1], args[2] as i32),
        SYSCALL_SHMCTL => sys_shmctl(args[0], args[1] as i32, args[2] as *mut u8),
        SYSCALL_SHMAT => sys_shmat(args[0], args[1] as *const u8, args[2] as i32),
        SYSCALL_SHMDT => sys_shmdt(args[0] as *const u8),
        SYSCALL_SCHED_GETAFFINITY => sys_sched_getaffinity(args[0], args[1], args[2]),
        SYSCALL_SCHED_GETSCHEDULER => sys_sched_getscheduler(args[0] as isize),
        SYSCALL_SCHED_SETSCHEDULER => sys_sched_setscheduler(
            args[0] as isize,
            args[1] as i32,
            args[2] as *const SchedParam,
        ),
        SYSCALL_SCHED_GETPARAM => sys_sched_getparam(args[0] as isize, args[1] as *mut SchedParam),
        SYSCALL_TIMERFD_CREATE => sys_timerfd_create(args[0], args[1] as i32),
        SYSCALL_TIMERFD_SETTIME => sys_timerfd_settime(
            args[0],
            args[1] as i32,
            args[2] as *const TimeSpec,
            args[3] as *mut TimeSpec,
        ),
        SYSCALL_TIMERFD_GETTIME => sys_timerfd_gettime(args[0], args[1] as *mut TimeSpec),
        SYSCALL_INOTIFY_INIT1 => sys_inotify_init1(args[0] as i32),
        SYSCALL_INOTIFY_ADD_WATCH => {
            sys_inotify_add_watch(args[0], args[1] as *const u8, args[2] as u32)
        }
        SYSCALL_INOTIFY_RM_WATCH => sys_inotify_rm_watch(args[0], args[1] as i32),
        SYSCALL_SCHED_SETAFFINITY => {
            sys_sched_setaffinity(args[0] as isize, args[1] as usize, args[2] as *const u64)
        }
        SYSCALL_SOCKETPAIR => sys_socketpair(
            args[0] as i32,
            args[1] as i32,
            args[2] as i32,
            args[3] as *mut i32,
        ),
        SYSCALL_CLOCK_GETRES => sys_clock_getres(args[0], args[1] as *mut NanoTimeVal),
        SYSCALL_MLOCK => sys_mlock(args[0], args[1]),
        SYSCALL_MUNLOCK => sys_munlock(args[0], args[1]),
        SYSCALL_GETRESUID => sys_getresuid(
            args[0] as *mut u32,
            args[1] as *mut u32,
            args[2] as *mut u32,
        ),
        SYSCALL_SETXATTR => sys_setxattr(
            args[0] as *const u8,
            args[1] as *const u8,
            args[2] as *const u8,
            args[3],
            args[4] as i32,
        ),
        SYSCALL_LSETXATTR => sys_lsetxattr(
            args[0] as *const u8,
            args[1] as *const u8,
            args[2] as *const u8,
            args[3],
            args[4] as i32,
        ),
        SYSCALL_FSETXATTR => sys_fsetxattr(
            args[0],
            args[1] as *const u8,
            args[2] as *const u8,
            args[3],
            args[4] as i32,
        ),
        SYSCALL_GETXATTR => sys_getxattr(
            args[0] as *const u8,
            args[1] as *const u8,
            args[2] as *mut u8,
            args[3],
        ),
        SYSCALL_LGETXATTR => sys_lgetxattr(
            args[0] as *const u8,
            args[1] as *const u8,
            args[2] as *mut u8,
            args[3],
        ),
        SYSCALL_FGETXATTR => {
            sys_fgetxattr(args[0], args[1] as *const u8, args[2] as *mut u8, args[3])
        }
        SYSCALL_LISTXATTR => sys_listxattr(args[0] as *const u8, args[1] as *mut u8, args[2]),
        SYSCALL_LLISTXATTR => sys_llistxattr(args[0] as *const u8, args[1] as *mut u8, args[2]),
        SYSCALL_FLISTXATTR => sys_flistxattr(args[0], args[1] as *mut u8, args[2]),
        SYSCALL_REMOVEXATTR => sys_removexattr(args[0] as *const u8, args[1] as *const u8),
        SYSCALL_LREMOVEXATTR => sys_lremovexattr(args[0] as *const u8, args[1] as *const u8),
        SYSCALL_FREMOVEXATTR => sys_fremovexattr(args[0], args[1] as *const u8),
        SYSCALL_FCHMOD => sys_fchmod(args[0] as usize, args[1] as u32),
        SYSCALL_MEMBARRIER => sys_membarrier(args[0] as i32, args[1] as i32, args[2] as *mut u64),

        _ => {
            error!("Unsupported syscall_id: {}", syscall_id);
            Err(SysError::ENOSYS)
        }
    }
}
