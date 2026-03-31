use core::arch::asm;

const SYSCALL_GETCWD: usize = 17;
const SYSCALL_MKDIR: usize = 34;
const SYSCALL_CHDIR: usize = 49;
const SYSCALL_OPENAT: usize = 56;
const SYSCALL_CLOSE: usize = 57;
const SYSCALL_GETDENTS: usize = 61;
const SYSCALL_READ: usize = 63;
const SYSCALL_WRITE: usize = 64;
const SYSCALL_EXIT: usize = 93;
const SYSCALL_YIELD: usize = 124;
const SYSCALL_GET_TIME: usize = 169;
const SYSCALL_GETPID: usize = 172;
const SYSCALL_FORK: usize = 220;
const SYSCALL_WAITPID: usize = 260;
const SYSCALL_EXECVE: usize = 221;
const SYSCALL_SOCKET: usize = 198;
const SYSCALL_BIND: usize = 200;
const SYSCALL_SENDTO: usize = 206;
const SYSCALL_RECVFROM: usize = 207;

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

fn syscall(id: usize, args: [usize; 3]) -> isize {
    let mut ret: isize;
    unsafe {
        asm!(
            "ecall",
            inlateout("x10") args[0] => ret,
            in("x11") args[1],
            in("x12") args[2],
            in("x17") id
        );
    }
    ret
}

fn syscall6(id: usize, args: [usize; 6]) -> isize {
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
            in("x17") id
        );
    }
    ret
}

pub fn sys_getcwd(buf: *const u8, len: usize) -> isize {
    syscall(SYSCALL_GETCWD, [buf as usize, len, 0])
}
pub fn sys_mkdir(dirfd: isize, path: *const u8, mode: u32) -> isize {
    syscall(SYSCALL_MKDIR, [
        dirfd as usize,
        path as usize,
        mode as usize,
    ])
}
pub fn sys_chdir(path: *const u8) -> isize {
    syscall(SYSCALL_CHDIR, [path as usize, 0, 0])
}
pub fn sys_openat(dirfd: isize, path: *const u8, flags: u32) -> isize {
    syscall(SYSCALL_OPENAT, [
        dirfd as usize,
        path as usize,
        flags as usize,
    ])
}

pub fn sys_close(fd: usize) -> isize {
    syscall(SYSCALL_CLOSE, [fd, 0, 0])
}

pub fn sys_getdents64(fd: usize, buf: *mut u8, len: usize) -> isize {
    syscall(SYSCALL_GETDENTS, [fd, buf as usize, len])
}

pub fn sys_read(fd: usize, buffer: &mut [u8]) -> isize {
    syscall(SYSCALL_READ, [
        fd,
        buffer.as_mut_ptr() as usize,
        buffer.len(),
    ])
}

pub fn sys_write(fd: usize, buffer: &[u8]) -> isize {
    syscall(SYSCALL_WRITE, [fd, buffer.as_ptr() as usize, buffer.len()])
}

pub fn sys_exit(exit_code: i32) -> ! {
    syscall(SYSCALL_EXIT, [exit_code as usize, 0, 0]);
    panic!("sys_exit never returns!");
}

pub fn sys_yield() -> isize {
    syscall(SYSCALL_YIELD, [0, 0, 0])
}

pub fn sys_get_time(time: &TimeVal, tz: usize) -> isize {
    syscall(SYSCALL_GET_TIME, [time as *const _ as usize, tz, 0])
}

pub fn sys_getpid() -> isize {
    syscall(SYSCALL_GETPID, [0, 0, 0])
}

pub fn sys_fork() -> isize {
    syscall(SYSCALL_FORK, [0, 0, 0])
}

// pub fn sys_exec(path: *const u8) -> isize {
//     syscall(SYSCALL_EXEC, [path as usize, 0, 0])
// }
pub fn sys_execve(path: *const u8, argv: *const usize, envp: *const usize) -> isize {
    println!("enter user execve");
    syscall(SYSCALL_EXECVE, [
        path as usize,
        argv as usize,
        envp as usize,
    ])
}
pub fn sys_waitpid(pid: isize, exit_code: *mut i32) -> isize {
    syscall(SYSCALL_WAITPID, [pid as usize, exit_code as usize, 0])
}

pub fn sys_socket(domain: i32, type_: i32, protocol: i32) -> isize {
    syscall(SYSCALL_SOCKET, [
        domain as usize,
        type_ as usize,
        protocol as usize,
    ])
}

pub fn sys_sendto(
    fd: usize,
    buf_ptr: *const u8,
    len: usize,
    _flags: i32,
    addr_ptr: *const u8,
    addr_len: usize,
) -> isize {
    syscall6(SYSCALL_SENDTO, [
        fd as usize,
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
    syscall6(SYSCALL_RECVFROM, [
        fd as usize,
        buf_ptr as usize,
        len,
        _flags as usize,
        addr_ptr as usize,
        addr_len as usize,
    ])
}
