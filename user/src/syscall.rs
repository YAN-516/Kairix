use crate::SignalSet;
use core::arch::asm;

const SYSCALL_GETCWD: usize = 17;
const SYSCALL_EVENTFD2: usize = 19;
const SYSCALL_EPOLL_CREATE1: usize = 20;
const SYSCALL_EPOLL_CTL: usize = 21;
const SYSCALL_EPOLL_PWAIT: usize = 22;
const SYSCALL_FCNTL: usize = 25;
const SYSCALL_IOCTL: usize = 29;
const SYSCALL_MKDIR: usize = 34;
const SYSCALL_UNLINKAT: usize = 35;
const SYSCALL_SYMLINKAT: usize = 36;
const SYSCALL_LINKAT: usize = 37;
const SYSCALL_RENAMEAT: usize = 38;
const SYSCALL_UMOUNT2: usize = 39;
const SYSCALL_MOUNT: usize = 40;
const SYSCALL_FTRUNCATE: usize = 46;
const SYSCALL_CHDIR: usize = 49;
const SYSCALL_FCHOWN: usize = 55;
const SYSCALL_OPENAT: usize = 56;
const SYSCALL_CLOSE: usize = 57;
const SYSCALL_PIPE: usize = 59;
const SYSCALL_GETDENTS: usize = 61;
const SYSCALL_LSEEK: usize = 62;
const SYSCALL_READ: usize = 63;
const SYSCALL_WRITE: usize = 64;
const SYSCALL_READV: usize = 65;
const SYSCALL_WRITEV: usize = 66;
const SYSCALL_SIGNALFD4: usize = 74;
const SYSCALL_PREAD64: usize = 67;
const SYSCALL_READLINKAT: usize = 78;
const SYSCALL_FSTATAT: usize = 79;
const SYSCALL_FSTAT: usize = 80;
const SYSCALL_SYNC: usize = 81;
const SYSCALL_UTIMENSAT: usize = 88;
const SYSCALL_EXIT: usize = 93;
const SYSCALL_FUTEX: usize = 98;
const SYSCALL_SCHED_SETSCHEDULER: usize = 119;
const SYSCALL_SCHED_GETSCHEDULER: usize = 120;
const SYSCALL_SCHED_SETAFFINITY: usize = 122;
const SYSCALL_SCHED_GETAFFINITY: usize = 123;
const SYSCALL_YIELD: usize = 124;
const SYSCALL_KILL: usize = 129;
const SYSCALL_TKILL: usize = 130;
const SYSCALL_TGKILL: usize = 131;
const SYSCALL_SIGALTSTACK: usize = 132;
const SYSCALL_RT_SIGSUSPEND: usize = 133;
const SYSCALL_RT_SIGACTION: usize = 134;
const SYSCALL_RT_SIGPROCMASK: usize = 135;
const SYSCALL_RT_SIGQUEUEINFO: usize = 138;
const SYSCALL_SETPGID: usize = 154;
// const SYSCALL_GETPGID: usize = 155;
const SYSCALL_UNAME: usize = 160;
const SYSCALL_GET_TIME: usize = 169;
const SYSCALL_GETPID: usize = 172;
const SYSCALL_GETTID: usize = 178;
const SYSCALL_SHMGET: usize = 194;
const SYSCALL_SHMCTL: usize = 195;
const SYSCALL_SHMAT: usize = 196;
const SYSCALL_RT_TGSIGQUEUEINFO: usize = 240;
const SYSCALL_MEMBARRIER: usize = 283;
const SYSCALL_CLONE3: usize = 435;
const SYSCALL_OPENAT2: usize = 437;
const SYSCALL_FUTEX_WAITV: usize = 449;
const SYSCALL_READAHEAD: usize = 213;
const SYSCALL_FADVISE64: usize = 223;
const SYSCALL_MUNMAP: usize = 215;
const SYSCALL_MREMAP: usize = 216;
const SYSCALL_FORK: usize = 220;
const SYSCALL_EXECVE: usize = 221;
const SYSCALL_MMAP: usize = 222;
const SYSCALL_MPROTECT: usize = 226;
const SYSCALL_MSYNC: usize = 227;
const SYSCALL_WAITPID: usize = 260;
const SYSCALL_PRLIMIT64: usize = 261;
const SYSCALL_GETRANDOM: usize = 278;
const SYSCALL_OS_POWER_OFF: usize = 1001;
const SYSCALL_THREAD_CREATE: usize = 1000;
const SYSCALL_WAITTID: usize = 1002;

const SYSCALL_SOCKET: usize = 198;
const SYSCALL_LISTEN: usize = 201;
const SYSCALL_ACCEPT: usize = 202;
const SYSCALL_CONNECT: usize = 203;
const SYSCALL_BIND: usize = 200;
const SYSCALL_SENDTO: usize = 206;
const SYSCALL_RECVFROM: usize = 207;
const SYSCALL_SETSOCKOPT: usize = 208;
const SYSCALL_GETSOCKOPT: usize = 209;
const SYSCALL_SHUTDOWN: usize = 210;
const SYSCALL_SENDMSG: usize = 211;
const SYSCALL_RECVMSG: usize = 212;
const SYSCALL_SSH_CONNECT: usize = 1110;
const SYSCALL_SSH_WRITE: usize = 1111;
const SYSCALL_SSH_READ: usize = 1112;
const SYSCALL_SSH_CLOSE: usize = 1113;
const SYSCALL_SSH_PEER_IDENT: usize = 1114;
const SYSCALL_SSH_AUTH_PASSWORD: usize = 1115;
const SYSCALL_SSH_EXEC: usize = 1116;
const SYSCALL_SSH_CHANNEL_READ: usize = 1117;
const SYSCALL_SSH_CHANNEL_CLOSE: usize = 1118;
const SYSCALL_SSH_CHANNEL_STATUS: usize = 1119;
const SYSCALL_SSH_CHANNEL_WRITE: usize = 1120;
const SYSCALL_SSH_SHELL: usize = 1121;
const SYSCALL_SSH_CHANNEL_TRY_READ: usize = 1122;
const SYSCALL_SSH_AUTH_PUBLICKEY: usize = 1123;

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
pub fn sys_eventfd2(initval: u32, flags: i32) -> isize {
    syscall(SYSCALL_EVENTFD2, [
        initval as usize,
        flags as usize,
        0,
        0,
        0,
        0,
    ])
}
pub fn sys_readv(fd: usize, iov: *const u8, iovcnt: usize) -> isize {
    syscall(SYSCALL_READV, [fd, iov as usize, iovcnt, 0, 0, 0])
}
pub fn sys_writev(fd: usize, iov: *const u8, iovcnt: usize) -> isize {
    syscall(SYSCALL_WRITEV, [fd, iov as usize, iovcnt, 0, 0, 0])
}
pub fn sys_signalfd4(fd: isize, mask: *const u8, sizemask: usize, flags: i32) -> isize {
    syscall(SYSCALL_SIGNALFD4, [
        fd as usize,
        mask as usize,
        sizemask,
        flags as usize,
        0,
        0,
    ])
}
pub fn sys_prlimit64(pid: usize, resource: i32, new_limit: *const u8, old_limit: *mut u8) -> isize {
    syscall(SYSCALL_PRLIMIT64, [
        pid,
        resource as usize,
        new_limit as usize,
        old_limit as usize,
        0,
        0,
    ])
}
pub fn sys_getrandom(buf: *mut u8, len: usize, flags: u32) -> isize {
    syscall(SYSCALL_GETRANDOM, [
        buf as usize,
        len,
        flags as usize,
        0,
        0,
        0,
    ])
}
pub fn sys_openat2(dirfd: isize, path: *const u8, how: *const u8, size: usize) -> isize {
    syscall(SYSCALL_OPENAT2, [
        dirfd as usize,
        path as usize,
        how as usize,
        size,
        0,
        0,
    ])
}
pub fn sys_epoll_create1(flags: i32) -> isize {
    syscall(SYSCALL_EPOLL_CREATE1, [flags as usize, 0, 0, 0, 0, 0])
}
pub fn sys_epoll_ctl(epfd: usize, op: i32, fd: usize, event: *const u8) -> isize {
    syscall(SYSCALL_EPOLL_CTL, [
        epfd,
        op as usize,
        fd,
        event as usize,
        0,
        0,
    ])
}
pub fn sys_epoll_pwait(epfd: usize, events: *mut u8, maxevents: i32, timeout_ms: i32) -> isize {
    syscall(SYSCALL_EPOLL_PWAIT, [
        epfd,
        events as usize,
        maxevents as usize,
        timeout_ms as usize,
        0,
        0,
    ])
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

pub fn sys_renameat(
    olddirfd: isize,
    oldpath: *const u8,
    newdirfd: isize,
    newpath: *const u8,
) -> isize {
    syscall(SYSCALL_RENAMEAT, [
        olddirfd as usize,
        oldpath as usize,
        newdirfd as usize,
        newpath as usize,
        0,
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

pub fn sys_ftruncate(fd: usize, length: usize) -> isize {
    syscall(SYSCALL_FTRUNCATE, [fd, length, 0, 0, 0, 0])
}

pub fn sys_pipe(pipe: *mut i32, flags: u32) -> isize {
    syscall(SYSCALL_PIPE, [pipe as usize, flags as usize, 0, 0, 0, 0])
}

pub fn sys_getdents64(fd: usize, buf: *mut u8, len: usize) -> isize {
    syscall(SYSCALL_GETDENTS, [fd, buf as usize, len, 0, 0, 0])
}

pub fn sys_lseek(fd: usize, offset: isize, whence: i32) -> isize {
    syscall(SYSCALL_LSEEK, [
        fd,
        offset as usize,
        whence as usize,
        0,
        0,
        0,
    ])
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

pub fn sys_pread64(fd: usize, buffer: &mut [u8], offset: usize) -> isize {
    syscall(SYSCALL_PREAD64, [
        fd,
        buffer.as_mut_ptr() as usize,
        buffer.len(),
        offset,
        0,
        0,
    ])
}

pub fn sys_readlinkat(dirfd: isize, path: *const u8, buf: *mut u8, len: usize) -> isize {
    syscall(SYSCALL_READLINKAT, [
        dirfd as usize,
        path as usize,
        buf as usize,
        len,
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

pub fn sys_fchown(fd: usize, owner: u32, group: u32) -> isize {
    syscall(SYSCALL_FCHOWN, [
        fd,
        owner as usize,
        group as usize,
        0,
        0,
        0,
    ])
}

pub fn sys_fstatat(dirfd: isize, path: *const u8, stat_buf: *mut u8, flags: u32) -> isize {
    syscall(SYSCALL_FSTATAT, [
        dirfd as usize,
        path as usize,
        stat_buf as usize,
        flags as usize,
        0,
        0,
    ])
}

pub fn sys_sync() -> isize {
    syscall(SYSCALL_SYNC, [0, 0, 0, 0, 0, 0])
}

pub fn sys_exit(exit_code: i32) -> ! {
    syscall(SYSCALL_EXIT, [exit_code as usize, 0, 0, 0, 0, 0]);
    loop { core::hint::spin_loop(); }
}

pub fn sys_futex(
    uaddr: *mut u32,
    op: i32,
    val: u32,
    timeout: usize,
    uaddr2: *mut u32,
    val3: u32,
) -> isize {
    syscall(SYSCALL_FUTEX, [
        uaddr as usize,
        op as usize,
        val as usize,
        timeout,
        uaddr2 as usize,
        val3 as usize,
    ])
}

pub fn sys_futex_waitv(
    waiters: *const u8,
    count: usize,
    flags: u32,
    timeout: usize,
    clockid: i32,
) -> isize {
    syscall(SYSCALL_FUTEX_WAITV, [
        waiters as usize,
        count,
        flags as usize,
        timeout,
        clockid as usize,
        0,
    ])
}

pub fn sys_membarrier(cmd: i32, flags: i32) -> isize {
    syscall(SYSCALL_MEMBARRIER, [
        cmd as usize,
        flags as usize,
        0,
        0,
        0,
        0,
    ])
}

pub fn sys_sched_setaffinity(pid: isize, mask: *const u64) -> isize {
    syscall(SYSCALL_SCHED_SETAFFINITY, [
        pid as usize,
        core::mem::size_of::<u64>(),
        mask as usize,
        0,
        0,
        0,
    ])
}

pub fn sys_sched_getaffinity(pid: isize, mask: *mut u64) -> isize {
    syscall(SYSCALL_SCHED_GETAFFINITY, [
        pid as usize,
        core::mem::size_of::<u64>(),
        mask as usize,
        0,
        0,
        0,
    ])
}

pub fn sys_sched_setscheduler(pid: isize, policy: i32, param: *const i32) -> isize {
    syscall(SYSCALL_SCHED_SETSCHEDULER, [
        pid as usize,
        policy as usize,
        param as usize,
        0,
        0,
        0,
    ])
}

pub fn sys_sched_getscheduler(pid: isize) -> isize {
    syscall(SYSCALL_SCHED_GETSCHEDULER, [pid as usize, 0, 0, 0, 0, 0])
}

pub fn sys_tkill(tid: isize, sig: i32) -> isize {
    syscall(SYSCALL_TKILL, [tid as usize, sig as usize, 0, 0, 0, 0])
}

pub fn sys_tgkill(tgid: isize, tid: isize, sig: i32) -> isize {
    syscall(SYSCALL_TGKILL, [
        tgid as usize,
        tid as usize,
        sig as usize,
        0,
        0,
        0,
    ])
}

pub fn sys_rt_sigqueueinfo(pid: isize, sig: i32, info: *const u8) -> isize {
    syscall(SYSCALL_RT_SIGQUEUEINFO, [
        pid as usize,
        sig as usize,
        info as usize,
        0,
        0,
        0,
    ])
}

pub fn sys_rt_tgsigqueueinfo(tgid: isize, tid: isize, sig: i32, info: *const u8) -> isize {
    syscall(SYSCALL_RT_TGSIGQUEUEINFO, [
        tgid as usize,
        tid as usize,
        sig as usize,
        info as usize,
        0,
        0,
    ])
}

pub fn sys_waittid(tid: usize) -> isize {
    syscall(SYSCALL_WAITTID, [tid, 0, 0, 0, 0, 0])
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

pub fn sys_gettid() -> isize {
    syscall(SYSCALL_GETTID, [0, 0, 0, 0, 0, 0])
}

pub fn sys_thread_create(entry: usize, arg: usize) -> isize {
    syscall(SYSCALL_THREAD_CREATE, [entry, arg, 0, 0, 0, 0])
}

pub fn sys_readahead(fd: usize, offset: usize, count: usize) -> isize {
    syscall(SYSCALL_READAHEAD, [fd, offset, count, 0, 0, 0])
}

pub fn sys_fadvise64(fd: usize, offset: usize, len: usize, advice: i32) -> isize {
    syscall(SYSCALL_FADVISE64, [fd, offset, len, advice as usize, 0, 0])
}

pub fn sys_utimensat(dirfd: isize, path: *const u8, times: *const u8, flags: i32) -> isize {
    syscall(SYSCALL_UTIMENSAT, [
        dirfd as usize,
        path as usize,
        times as usize,
        flags as usize,
        0,
        0,
    ])
}

pub fn sys_kill(pid: isize, sig: usize) -> isize {
    syscall(SYSCALL_KILL, [pid as usize, sig, 0, 0, 0, 0])
}

pub fn sys_sigaltstack(ss: *const u8, old_ss: *mut u8) -> isize {
    syscall(SYSCALL_SIGALTSTACK, [
        ss as usize,
        old_ss as usize,
        0,
        0,
        0,
        0,
    ])
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

pub fn sys_rt_sigsuspend(mask: *const SignalSet, sigsetsize: usize) -> isize {
    syscall(SYSCALL_RT_SIGSUSPEND, [
        mask as usize,
        sigsetsize,
        0,
        0,
        0,
        0,
    ])
}

pub fn sys_munmap(start: usize, len: usize) -> isize {
    syscall(SYSCALL_MUNMAP, [start, len, 0, 0, 0, 0])
}

pub fn sys_mremap(
    old_address: usize,
    old_size: usize,
    new_size: usize,
    flags: usize,
    new_address: usize,
) -> isize {
    syscall(SYSCALL_MREMAP, [
        old_address,
        old_size,
        new_size,
        flags,
        new_address,
        0,
    ])
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

pub fn sys_mprotect(start: usize, len: usize, prot: usize) -> isize {
    syscall(SYSCALL_MPROTECT, [start, len, prot, 0, 0, 0])
}

pub fn sys_msync(start: usize, len: usize, flags: usize) -> isize {
    syscall(SYSCALL_MSYNC, [start, len, flags, 0, 0, 0])
}

pub fn sys_shmget(key: i32, size: usize, flags: i32) -> isize {
    syscall(SYSCALL_SHMGET, [
        key as usize,
        size,
        flags as usize,
        0,
        0,
        0,
    ])
}

pub fn sys_shmat(shmid: usize, address: usize, flags: i32) -> isize {
    syscall(SYSCALL_SHMAT, [shmid, address, flags as usize, 0, 0, 0])
}

pub fn sys_shmctl(shmid: usize, command: i32, buffer: *mut u8) -> isize {
    syscall(SYSCALL_SHMCTL, [
        shmid,
        command as usize,
        buffer as usize,
        0,
        0,
        0,
    ])
}

pub fn sys_fork() -> isize {
    syscall(SYSCALL_FORK, [0, 0, 0, 0, 0, 0])
}

pub fn sys_clone_raw(
    flags: u64,
    stack: usize,
    parent_tid: *mut i32,
    child_tid: *mut i32,
    tls: usize,
) -> isize {
    #[cfg(target_arch = "riscv64")]
    let args = [
        flags as usize,
        stack,
        parent_tid as usize,
        tls,
        child_tid as usize,
        0,
    ];
    #[cfg(target_arch = "loongarch64")]
    let args = [
        flags as usize,
        stack,
        parent_tid as usize,
        child_tid as usize,
        tls,
        0,
    ];
    syscall(SYSCALL_FORK, args)
}

pub fn sys_clone3_raw(args: *mut u8, size: usize) -> isize {
    syscall(SYSCALL_CLONE3, [args as usize, size, 0, 0, 0, 0])
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

pub fn sys_ssh_connect(fd: usize, ident: *const u8, ident_len: usize) -> isize {
    syscall(SYSCALL_SSH_CONNECT, [
        fd,
        ident as usize,
        ident_len,
        0,
        0,
        0,
    ])
}

pub fn sys_ssh_write(ssh_id: usize, buf: *const u8, len: usize) -> isize {
    syscall(SYSCALL_SSH_WRITE, [ssh_id, buf as usize, len, 0, 0, 0])
}

pub fn sys_ssh_read(ssh_id: usize, buf: *mut u8, len: usize) -> isize {
    syscall(SYSCALL_SSH_READ, [ssh_id, buf as usize, len, 0, 0, 0])
}

pub fn sys_ssh_close(ssh_id: usize) -> isize {
    syscall(SYSCALL_SSH_CLOSE, [ssh_id, 0, 0, 0, 0, 0])
}

pub fn sys_ssh_peer_ident(ssh_id: usize, buf: *mut u8, len: usize) -> isize {
    syscall(SYSCALL_SSH_PEER_IDENT, [ssh_id, buf as usize, len, 0, 0, 0])
}

pub fn sys_ssh_auth_password(
    ssh_id: usize,
    username: *const u8,
    username_len: usize,
    password: *const u8,
    password_len: usize,
) -> isize {
    syscall(SYSCALL_SSH_AUTH_PASSWORD, [
        ssh_id,
        username as usize,
        username_len,
        password as usize,
        password_len,
        0,
    ])
}

pub fn sys_ssh_auth_publickey(
    ssh_id: usize,
    username: *const u8,
    username_len: usize,
    key: *const u8,
    key_len: usize,
) -> isize {
    syscall(SYSCALL_SSH_AUTH_PUBLICKEY, [
        ssh_id,
        username as usize,
        username_len,
        key as usize,
        key_len,
        0,
    ])
}

pub fn sys_ssh_exec(ssh_id: usize, command: *const u8, command_len: usize) -> isize {
    syscall(SYSCALL_SSH_EXEC, [
        ssh_id,
        command as usize,
        command_len,
        0,
        0,
        0,
    ])
}

pub fn sys_ssh_shell(ssh_id: usize) -> isize {
    syscall(SYSCALL_SSH_SHELL, [ssh_id, 0, 0, 0, 0, 0])
}

pub fn sys_ssh_channel_read(ssh_id: usize, channel_id: usize, buf: *mut u8, len: usize) -> isize {
    syscall(SYSCALL_SSH_CHANNEL_READ, [
        ssh_id,
        channel_id,
        buf as usize,
        len,
        0,
        0,
    ])
}

pub fn sys_ssh_channel_try_read(
    ssh_id: usize,
    channel_id: usize,
    buf: *mut u8,
    len: usize,
) -> isize {
    syscall(SYSCALL_SSH_CHANNEL_TRY_READ, [
        ssh_id,
        channel_id,
        buf as usize,
        len,
        0,
        0,
    ])
}

pub fn sys_ssh_channel_write(
    ssh_id: usize,
    channel_id: usize,
    buf: *const u8,
    len: usize,
) -> isize {
    syscall(SYSCALL_SSH_CHANNEL_WRITE, [
        ssh_id,
        channel_id,
        buf as usize,
        len,
        0,
        0,
    ])
}

pub fn sys_ssh_channel_close(ssh_id: usize, channel_id: usize) -> isize {
    syscall(SYSCALL_SSH_CHANNEL_CLOSE, [ssh_id, channel_id, 0, 0, 0, 0])
}

pub fn sys_ssh_channel_status(ssh_id: usize, channel_id: usize) -> isize {
    syscall(SYSCALL_SSH_CHANNEL_STATUS, [ssh_id, channel_id, 0, 0, 0, 0])
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

pub fn sys_setsockopt(
    fd: usize,
    level: i32,
    optname: i32,
    optval: *const u8,
    optlen: usize,
) -> isize {
    syscall(SYSCALL_SETSOCKOPT, [
        fd,
        level as usize,
        optname as usize,
        optval as usize,
        optlen,
        0,
    ])
}

pub fn sys_getsockopt(
    fd: usize,
    level: i32,
    optname: i32,
    optval: *mut u8,
    optlen: *mut u32,
) -> isize {
    syscall(SYSCALL_GETSOCKOPT, [
        fd,
        level as usize,
        optname as usize,
        optval as usize,
        optlen as usize,
        0,
    ])
}

pub fn sys_setpgid(pid: usize, pgid: usize) -> isize {
    syscall(SYSCALL_SETPGID, [pid, pgid, 0, 0, 0, 0])
}

pub fn sys_fcntl(fd: usize, cmd: usize, arg: usize) -> isize {
    syscall(SYSCALL_FCNTL, [fd, cmd, arg, 0, 0, 0])
}

pub fn sys_ioctl(fd: usize, request: usize, argp: usize) -> isize {
    syscall(SYSCALL_IOCTL, [fd, request, argp, 0, 0, 0])
}
