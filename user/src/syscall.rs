use crate::SignalSet;
use core::arch::asm;

const SYSCALL_GETCWD: usize = 17;
const SYSCALL_IOCTL: usize = 29;
const SYSCALL_MKDIR: usize = 34;
const SYSCALL_UNLINKAT: usize = 35;
const SYSCALL_SYMLINKAT: usize = 36;
const SYSCALL_LINKAT: usize = 37;
const SYSCALL_UMOUNT2: usize = 39;
const SYSCALL_MOUNT: usize = 40;
const SYSCALL_CHDIR: usize = 49;
const SYSCALL_OPENAT: usize = 56;
const SYSCALL_CLOSE: usize = 57;
const SYSCALL_GETDENTS: usize = 61;
const SYSCALL_READ: usize = 63;
const SYSCALL_WRITE: usize = 64;
const SYSCALL_FSTAT: usize = 80;
const SYSCALL_SYNC: usize = 81;
const SYSCALL_EXIT: usize = 93;
const SYSCALL_YIELD: usize = 124;
const SYSCALL_KILL: usize = 129;
const SYSCALL_RT_SIGACTION: usize = 134;
const SYSCALL_RT_SIGPROCMASK: usize = 135;
const SYSCALL_SETPGID: usize = 154;
// const SYSCALL_GETPGID: usize = 155;
const SYSCALL_UNAME: usize = 160;
const SYSCALL_GET_TIME: usize = 169;
const SYSCALL_GETPID: usize = 172;
const SYSCALL_READAHEAD: usize = 213;
const SYSCALL_FADVISE64: usize = 223;
const SYSCALL_MUNMAP: usize = 215;
const SYSCALL_FORK: usize = 220;
const SYSCALL_EXECVE: usize = 221;
const SYSCALL_MMAP: usize = 222;
const SYSCALL_WAITPID: usize = 260;
const SYSCALL_OS_POWER_OFF: usize = 1001;

const SYSCALL_SOCKET: usize = 198;
const SYSCALL_LISTEN: usize = 201;
const SYSCALL_ACCEPT: usize = 202;
const SYSCALL_CONNECT: usize = 203;
const SYSCALL_BIND: usize = 200;
const SYSCALL_SENDTO: usize = 206;
const SYSCALL_RECVFROM: usize = 207;
const SYSCALL_SHUTDOWN: usize = 210;
const SYSCALL_SENDMSG: usize = 211;
const SYSCALL_RECVMSG: usize = 212;

#[repr(C)]
#[derive(Debug, Default)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

impl TimeVal {
    pub fn new() -> Self {
        Self::default()
    }
}
#[cfg(target_arch = "riscv64")]
fn syscall(id: usize, args: [usize; 6]) -> isize {
    let mut ret: isize;
    unsafe {
        asm!(
            "ecall",
            inlateout("x10") args[0] => ret,
            in("x11") args[1],
            in("x12") args[2],
            in("x13") args[3],
            in("x14") args[4],
            in("x15") args[5],
            in("x17") id,
        );
    }
    ret
}

#[cfg(target_arch = "loongarch64")]
fn syscall(id: usize, args: [usize; 6]) -> isize {
    let ret: isize;
    unsafe {
        asm!(
            "syscall 0",
            inlateout("$a0") args[0] => ret,
            in("$a1") args[1],
            in("$a2") args[2],
            in("$a3") args[3],
            in("$a4") args[4],
            in("$a5") args[5],
            in("$a7") id,
        );
    }
    ret
}

pub fn sys_getcwd(buf: *const u8, len: usize) -> isize {
    syscall(SYSCALL_GETCWD, [buf as usize, len, 0, 0, 0, 0])
}
pub fn sys_mkdir(dirfd: isize, path: *const u8, mode: u32) -> isize {
    syscall(SYSCALL_MKDIR, [
        dirfd as usize,
        path as usize,
        mode as usize,
        0,
        0,
        0,
    ])
}

pub fn sys_unlinkat(dirfd: isize, path: *const u8, flags: u32) -> isize {
    syscall(SYSCALL_UNLINKAT, [
        dirfd as usize,
        path as usize,
        flags as usize,
        0,
        0,
        0,
    ])
}

pub fn sys_symlinkat(target: *const u8, newdirfd: isize, linkpath: *const u8) -> isize {
    syscall(SYSCALL_SYMLINKAT, [
        target as usize,
        newdirfd as usize,
        linkpath as usize,
        0,
        0,
        0,
    ])
}

pub fn sys_linkat(
    olddirfd: isize,
    oldpath: *const u8,
    newdirfd: isize,
    newpath: *const u8,
    flags: u32,
) -> isize {
    syscall(SYSCALL_LINKAT, [
        olddirfd as usize,
        oldpath as usize,
        newdirfd as usize,
        newpath as usize,
        flags as usize,
        0,
    ])
}

pub fn sys_umount2(target: *const u8, flags: u32) -> isize {
    syscall(SYSCALL_UMOUNT2, [
        target as usize,
        flags as usize,
        0,
        0,
        0,
        0,
    ])
}

pub fn sys_mount(
    source: *const u8,
    mount_point: *const u8,
    fstype: *const u8,
    flags: isize,
    data: *const u8,
) -> isize {
    syscall(SYSCALL_MOUNT, [
        source as usize,
        mount_point as usize,
        fstype as usize,
        flags as usize,
        data as usize,
        0,
    ])
}

pub fn sys_chdir(path: *const u8) -> isize {
    syscall(SYSCALL_CHDIR, [path as usize, 0, 0, 0, 0, 0])
}
pub fn sys_openat(dirfd: isize, path: *const u8, flags: u32, mode: u32) -> isize {
    syscall(SYSCALL_OPENAT, [
        dirfd as usize,
        path as usize,
        flags as usize,
        mode as usize,
        0,
        0,
    ])
}

pub fn sys_close(fd: usize) -> isize {
    syscall(SYSCALL_CLOSE, [fd, 0, 0, 0, 0, 0])
}

pub fn sys_getdents64(fd: usize, buf: *mut u8, len: usize) -> isize {
    syscall(SYSCALL_GETDENTS, [fd, buf as usize, len, 0, 0, 0])
}

pub fn sys_read(fd: usize, buffer: &mut [u8]) -> isize {
    syscall(SYSCALL_READ, [
        fd,
        buffer.as_mut_ptr() as usize,
        buffer.len(),
        0,
        0,
        0,
    ])
}

pub fn sys_write(fd: usize, buffer: &[u8]) -> isize {
    syscall(SYSCALL_WRITE, [
        fd,
        buffer.as_ptr() as usize,
        buffer.len(),
        0,
        0,
        0,
    ])
}

pub fn sys_fstat(fd: usize, stat_buf: *mut u8) -> isize {
    syscall(SYSCALL_FSTAT, [fd, stat_buf as usize, 0, 0, 0, 0])
}

pub fn sys_sync() -> isize {
    syscall(SYSCALL_SYNC, [0, 0, 0, 0, 0, 0])
}

pub fn sys_exit(exit_code: i32) -> ! {
    syscall(SYSCALL_EXIT, [exit_code as usize, 0, 0, 0, 0, 0]);
    panic!("sys_exit never returns!");
}

pub fn sys_yield() -> isize {
    syscall(SYSCALL_YIELD, [0, 0, 0, 0, 0, 0])
}

pub fn sys_uname(buf: *mut u8) -> isize {
    syscall(SYSCALL_UNAME, [buf as usize, 0, 0, 0, 0, 0])
}

pub fn sys_get_time(time: &mut TimeVal, tz: usize) -> isize {
    syscall(SYSCALL_GET_TIME, [time as *mut _ as usize, tz, 0, 0, 0, 0])
}

pub fn sys_getpid() -> isize {
    syscall(SYSCALL_GETPID, [0, 0, 0, 0, 0, 0])
}

pub fn sys_readahead(fd: usize, offset: usize, count: usize) -> isize {
    syscall(SYSCALL_READAHEAD, [fd, offset, count, 0, 0, 0])
}

pub fn sys_fadvise64(fd: usize, offset: usize, len: usize, advice: i32) -> isize {
    syscall(SYSCALL_FADVISE64, [fd, offset, len, advice as usize, 0, 0])
}

pub fn sys_kill(pid: isize, sig: usize) -> isize {
    syscall(SYSCALL_KILL, [pid as usize, sig, 0, 0, 0, 0])
}

pub fn sys_rt_sigaction(signum: i32, act: *const u8, oldact: *mut u8, sigsetsize: usize) -> isize {
    syscall(SYSCALL_RT_SIGACTION, [
        signum as usize,
        act as usize,
        oldact as usize,
        sigsetsize,
        0,
        0,
    ])
}

pub fn sys_rt_sigprocmask(
    how: i32,
    set: *const SignalSet,
    oldset: *mut SignalSet,
    sigsetsize: usize,
) -> isize {
    syscall(SYSCALL_RT_SIGPROCMASK, [
        how as usize,
        set as usize,
        oldset as usize,
        sigsetsize,
        0,
        0,
    ])
}

pub fn sys_munmap(start: usize, len: usize) -> isize {
    syscall(SYSCALL_MUNMAP, [start, len, 0, 0, 0, 0])
}

pub fn sys_mmap(
    start: usize,
    len: usize,
    prot: usize,
    flags: usize,
    fd: isize,
    offset: usize,
) -> isize {
    syscall(SYSCALL_MMAP, [start, len, prot, flags, fd as usize, offset])
}

pub fn sys_fork() -> isize {
    syscall(SYSCALL_FORK, [0, 0, 0, 0, 0, 0])
}

// pub fn sys_exec(path: *const u8) -> isize {
//     syscall(SYSCALL_EXEC, [path as usize, 0, 0])
// }
pub fn sys_execve(path: *const u8, argv: *const usize, envp: *const usize) -> isize {
    syscall(SYSCALL_EXECVE, [
        path as usize,
        argv as usize,
        envp as usize,
        0,
        0,
        0,
    ])
}
pub fn sys_waitpid(pid: isize, exit_code: *mut i32) -> isize {
    sys_waitpid_options(pid, exit_code, 0)
}

pub fn sys_waitpid_options(pid: isize, exit_code: *mut i32, options: i32) -> isize {
    syscall(SYSCALL_WAITPID, [
        pid as usize,
        exit_code as usize,
        options as usize,
        0,
        0,
        0,
    ])
}

pub fn sys_poweroff(exit_code: i32) -> ! {
    syscall(SYSCALL_OS_POWER_OFF, [exit_code as usize, 0, 0, 0, 0, 0]);
    panic!("sys_poweroff never returns!");
}

pub fn sys_socket(domain: i32, type_: i32, protocol: i32) -> isize {
    syscall(SYSCALL_SOCKET, [
        domain as usize,
        type_ as usize,
        protocol as usize,
        0,
        0,
        0,
    ])
}

pub fn sys_listen(fd: usize, backlog: usize) -> isize {
    syscall(SYSCALL_LISTEN, [fd, backlog, 0, 0, 0, 0])
}

pub fn sys_accept(fd: usize, addr_ptr: *mut u8, addr_len: *mut usize) -> isize {
    syscall(SYSCALL_ACCEPT, [
        fd,
        addr_ptr as usize,
        addr_len as usize,
        0,
        0,
        0,
    ])
}

pub fn sys_connect(fd: usize, addr_ptr: *const u8, addr_len: usize) -> isize {
    syscall(SYSCALL_CONNECT, [fd, addr_ptr as usize, addr_len, 0, 0, 0])
}

pub fn sys_shutdown(fd: usize, how: i32) -> isize {
    syscall(SYSCALL_SHUTDOWN, [fd, how as usize, 0, 0, 0, 0])
}

pub fn sys_sendto(
    fd: usize,
    buf_ptr: *const u8,
    len: usize,
    _flags: i32,
    addr_ptr: *const u8,
    addr_len: usize,
) -> isize {
    syscall(SYSCALL_SENDTO, [
        fd,
        buf_ptr as usize,
        len,
        _flags as usize,
        addr_ptr as usize,
        addr_len,
    ])
}

pub fn sys_recvfrom(
    fd: usize,
    buf_ptr: *mut u8,
    len: usize,
    _flags: i32,
    addr_ptr: *mut u8,
    addr_len: *mut usize,
) -> isize {
    syscall(SYSCALL_RECVFROM, [
        fd,
        buf_ptr as usize,
        len,
        _flags as usize,
        addr_ptr as usize,
        addr_len as usize,
    ])
}

pub fn sys_sendmsg(fd: usize, msg_ptr: usize, flags: i32) -> isize {
    syscall(SYSCALL_SENDMSG, [fd, msg_ptr, flags as usize, 0, 0, 0])
}

pub fn sys_recvmsg(fd: usize, msg_ptr: usize, flags: i32) -> isize {
    syscall(SYSCALL_RECVMSG, [fd, msg_ptr, flags as usize, 0, 0, 0])
}

pub fn sys_bind(fd: usize, addr_ptr: *const u8, addr_len: usize) -> isize {
    syscall(SYSCALL_BIND, [fd, addr_ptr as usize, addr_len, 0, 0, 0])
}

pub fn sys_setpgid(pid: usize, pgid: usize) -> isize {
    syscall(SYSCALL_SETPGID, [pid, pgid, 0, 0, 0, 0])
}

pub fn sys_ioctl(fd: usize, request: usize, argp: usize) -> isize {
    syscall(SYSCALL_IOCTL, [fd, request, argp, 0, 0, 0])
}
