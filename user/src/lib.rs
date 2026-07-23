#![no_std]
#![feature(linkage)]
#![feature(alloc_error_handler)]

#[macro_use]
pub mod console;
pub mod git;
mod lang_items;
mod syscall;

extern crate alloc;
#[macro_use]
extern crate bitflags;

use alloc::{ffi::CString, vec::Vec};

use buddy_system_allocator::LockedHeap;
use core::arch::global_asm;
use core::ptr::addr_of_mut;
use syscall::*;

const USER_PATH_MAX: usize = 4096;
const USER_HEAP_SIZE: usize = 4 * 1024 * 1024;

static mut HEAP_SPACE: [u8; USER_HEAP_SIZE] = [0; USER_HEAP_SIZE];

#[global_allocator]
static HEAP: LockedHeap<32> = LockedHeap::empty();

fn copy_path_to_stack(path: &str, buf: &mut [u8; USER_PATH_MAX]) -> Result<*const u8, isize> {
    let bytes = path.as_bytes();
    if bytes.iter().any(|byte| *byte == 0) {
        return Err(-22);
    }
    if bytes.len() >= USER_PATH_MAX {
        return Err(-36);
    }
    buf[..bytes.len()].copy_from_slice(bytes);
    buf[bytes.len()] = 0;
    Ok(buf.as_ptr())
}

#[alloc_error_handler]
pub fn handle_alloc_error(layout: core::alloc::Layout) -> ! {
    panic!("Heap allocation error, layout = {:?}", layout);
}

#[cfg(target_arch = "riscv64")]
global_asm!(
    r#"
    .section .text.entry,"ax"
    .globl _start
_start:
    mv a0, sp
    call rust_start
"#
);

#[cfg(target_arch = "loongarch64")]
global_asm!(
    r#"
    .section .text.entry,"ax"
    .globl _start
_start:
    move $a0, $sp
    bl rust_start
"#
);

#[unsafe(no_mangle)]
pub extern "C" fn rust_start(stack_top: usize) -> ! {
    unsafe {
        HEAP.lock()
            .init(addr_of_mut!(HEAP_SPACE) as usize, USER_HEAP_SIZE);
    }
    let argc = unsafe { *(stack_top as *const usize) };
    let argv = (stack_top + core::mem::size_of::<usize>()) as *const usize;
    exit(main_with_args(argc, argv));
}

#[linkage = "weak"]
#[unsafe(no_mangle)]
fn main() -> i32 {
    panic!("Cannot find main!");
}

#[linkage = "weak"]
#[unsafe(no_mangle)]
fn main_with_args(_argc: usize, _argv: *const usize) -> i32 {
    main()
}
bitflags! {
    ///Open file flags
    pub struct OpenFlags: u32 {
        ///Read only
        const RDONLY = 0;
        ///Write only
        const WRONLY = 1;
        ///Read & Write
        const RDWR = 2;

        ///Allow create
        const O_CREAT       = 0o100;
        const O_TRUNC       = 0o1000;
        const O_DIRECTORY   = 0o200000;
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct EpollEvent {
    pub events: u32,
    pub data: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignalSet {
    bits: u64,
}

impl SignalSet {
    pub const fn empty() -> Self {
        Self { bits: 0 }
    }

    pub const fn bits(&self) -> u64 {
        self.bits
    }

    pub fn add(&mut self, signum: i32) {
        if (1..=64).contains(&signum) {
            self.bits |= 1u64 << ((signum - 1) as usize);
        }
    }

    pub fn remove(&mut self, signum: i32) {
        if (1..=64).contains(&signum) {
            self.bits &= !(1u64 << ((signum - 1) as usize));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigHandler {
    Default,
    Ignore,
    Custom(unsafe extern "C" fn(i32)),
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SigAction {
    pub sa_handler: SigHandler,
    pub sa_mask: SignalSet,
    pub sa_flags: u32,
    pub sa_restorer: usize,
}

impl SigAction {
    pub const fn default() -> Self {
        Self {
            sa_handler: SigHandler::Default,
            sa_mask: SignalSet::empty(),
            sa_flags: 0,
            sa_restorer: 0,
        }
    }

    pub const fn ignore() -> Self {
        Self {
            sa_handler: SigHandler::Ignore,
            sa_mask: SignalSet::empty(),
            sa_flags: 0,
            sa_restorer: 0,
        }
    }

    pub const fn custom(handler: unsafe extern "C" fn(i32)) -> Self {
        Self {
            sa_handler: SigHandler::Custom(handler),
            sa_mask: SignalSet::empty(),
            sa_flags: 0,
            sa_restorer: 0,
        }
    }
}

impl SigHandler {
    fn as_ptr(self) -> usize {
        match self {
            SigHandler::Default => 0,
            SigHandler::Ignore => 1,
            SigHandler::Custom(f) => f as usize,
        }
    }

    unsafe fn from_ptr(ptr: usize) -> Self {
        match ptr {
            0 => SigHandler::Default,
            1 => SigHandler::Ignore,
            _ => SigHandler::Custom(unsafe { core::mem::transmute(ptr) }),
        }
    }
}

#[cfg(target_arch = "riscv64")]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct KernelSigAction {
    handler: usize,
    flags: usize,
    mask: usize,
}

#[cfg(target_arch = "loongarch64")]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct KernelSigAction {
    handler: usize,
    flags: usize,
    mask: usize,
}

#[cfg(target_arch = "riscv64")]
fn to_kernel_sigaction(action: &SigAction) -> KernelSigAction {
    KernelSigAction {
        handler: action.sa_handler.as_ptr(),
        flags: action.sa_flags as usize,
        mask: action.sa_mask.bits() as usize,
    }
}

#[cfg(target_arch = "loongarch64")]
fn to_kernel_sigaction(action: &SigAction) -> KernelSigAction {
    KernelSigAction {
        handler: action.sa_handler.as_ptr(),
        flags: action.sa_flags as usize,
        mask: action.sa_mask.bits() as usize,
    }
}

#[cfg(target_arch = "riscv64")]
fn from_kernel_sigaction(action: &KernelSigAction) -> SigAction {
    SigAction {
        sa_handler: unsafe { SigHandler::from_ptr(action.handler) },
        sa_mask: SignalSet {
            bits: action.mask as u64,
        },
        sa_flags: action.flags as u32,
        sa_restorer: 0,
    }
}

#[cfg(target_arch = "loongarch64")]
fn from_kernel_sigaction(action: &KernelSigAction) -> SigAction {
    SigAction {
        sa_handler: unsafe { SigHandler::from_ptr(action.handler) },
        sa_mask: SignalSet {
            bits: action.mask as u64,
        },
        sa_flags: action.flags as u32,
        sa_restorer: 0,
    }
}

pub const SIG_BLOCK: i32 = 0;
pub const SIG_UNBLOCK: i32 = 1;
pub const SIG_SETMASK: i32 = 2;

pub const SIGUSR1: i32 = 10;
pub const SIGTERM: i32 = 15;

pub fn getcwd(buf: &mut [u8], len: usize) -> isize {
    sys_getcwd(buf.as_mut_ptr() as *const u8, len)
}

///ignore the mode,dirfd is always AT_FDCWD
pub fn mkdir(path: &str, _mode: u32) -> isize {
    let mut path_buf = [0u8; USER_PATH_MAX];
    let path = match copy_path_to_stack(path, &mut path_buf) {
        Ok(path) => path,
        Err(err) => return err,
    };
    sys_mkdir(-100, path, _mode)
}

pub fn unlinkat(dirfd: isize, path: &str, flags: u32) -> isize {
    let mut path_buf = [0u8; USER_PATH_MAX];
    let path = match copy_path_to_stack(path, &mut path_buf) {
        Ok(path) => path,
        Err(err) => return err,
    };
    sys_unlinkat(dirfd, path, flags)
}

pub const AT_FDCWD: isize = -100;

pub fn symlinkat(target: &str, newdirfd: isize, linkpath: &str) -> isize {
    let mut target_buf = [0u8; USER_PATH_MAX];
    let mut linkpath_buf = [0u8; USER_PATH_MAX];
    let target = match copy_path_to_stack(target, &mut target_buf) {
        Ok(target) => target,
        Err(err) => return err,
    };
    let linkpath = match copy_path_to_stack(linkpath, &mut linkpath_buf) {
        Ok(linkpath) => linkpath,
        Err(err) => return err,
    };
    sys_symlinkat(target, newdirfd, linkpath)
}

pub fn linkat(
    olddirfd: isize,
    oldpath: &str,
    newdirfd: isize,
    newpath: &str,
    _flags: u32,
) -> isize {
    let mut oldpath_buf = [0u8; USER_PATH_MAX];
    let mut newpath_buf = [0u8; USER_PATH_MAX];
    let oldpath = match copy_path_to_stack(oldpath, &mut oldpath_buf) {
        Ok(oldpath) => oldpath,
        Err(err) => return err,
    };
    let newpath = match copy_path_to_stack(newpath, &mut newpath_buf) {
        Ok(newpath) => newpath,
        Err(err) => return err,
    };
    sys_linkat(olddirfd, oldpath, newdirfd, newpath, _flags)
}

pub fn renameat(olddirfd: isize, oldpath: &str, newdirfd: isize, newpath: &str) -> isize {
    let mut oldpath_buf = [0u8; USER_PATH_MAX];
    let oldpath = match copy_path_to_stack(oldpath, &mut oldpath_buf) {
        Ok(path) => path,
        Err(err) => return err,
    };
    let mut newpath_buf = [0u8; USER_PATH_MAX];
    let newpath = match copy_path_to_stack(newpath, &mut newpath_buf) {
        Ok(path) => path,
        Err(err) => return err,
    };
    sys_renameat(olddirfd, oldpath, newdirfd, newpath)
}

pub fn umount2(target: &str, flags: u32) -> isize {
    let mut target_buf = [0u8; USER_PATH_MAX];
    let target = match copy_path_to_stack(target, &mut target_buf) {
        Ok(target) => target,
        Err(err) => return err,
    };
    sys_umount2(target, flags)
}

pub fn mount(
    special: &mut [u8],
    dir: &mut [u8],
    fstype: &mut [u8],
    flags: isize,
    data: &mut [u8],
) -> isize {
    sys_mount(
        special.as_mut_ptr() as *const u8,
        dir.as_mut_ptr() as *const u8,
        fstype.as_mut_ptr() as *const u8,
        flags as isize,
        data.as_mut_ptr() as *const u8,
    )
}

pub fn chdir(path: &str) -> isize {
    let mut path_buf = [0u8; USER_PATH_MAX];
    let path = match copy_path_to_stack(path, &mut path_buf) {
        Ok(path) => path,
        Err(err) => return err,
    };
    sys_chdir(path)
}

pub fn open(dirfd: isize, path: &str, flags: OpenFlags, mode: u32) -> isize {
    let mut path_buf = [0u8; USER_PATH_MAX];
    let path = match copy_path_to_stack(path, &mut path_buf) {
        Ok(path) => path,
        Err(err) => return err,
    };
    sys_openat(dirfd, path, flags.bits(), mode)
}
pub fn close(fd: usize) -> isize {
    sys_close(fd)
}
pub fn eventfd2(initval: u32, flags: i32) -> isize {
    sys_eventfd2(initval, flags)
}
pub fn epoll_create1(flags: i32) -> isize {
    sys_epoll_create1(flags)
}
pub fn epoll_ctl(epfd: usize, op: i32, fd: usize, event: &EpollEvent) -> isize {
    sys_epoll_ctl(epfd, op, fd, event as *const EpollEvent as *const u8)
}
pub fn epoll_wait(epfd: usize, events: &mut [EpollEvent], timeout_ms: i32) -> isize {
    sys_epoll_pwait(
        epfd,
        events.as_mut_ptr() as *mut u8,
        events.len() as i32,
        timeout_ms,
    )
}
pub fn ftruncate(fd: usize, length: usize) -> isize {
    sys_ftruncate(fd, length)
}
pub fn pipe(fds: &mut [i32; 2]) -> isize {
    sys_pipe(fds.as_mut_ptr(), 0)
}
pub fn getdents64(fd: usize, buf: &mut [u8]) -> isize {
    sys_getdents64(fd, buf.as_mut_ptr(), buf.len())
}
pub fn lseek(fd: usize, offset: isize, whence: i32) -> isize {
    sys_lseek(fd, offset, whence)
}
pub fn read(fd: usize, buf: &mut [u8]) -> isize {
    sys_read(fd, buf)
}
pub fn pread64(fd: usize, buf: &mut [u8], offset: usize) -> isize {
    sys_pread64(fd, buf, offset)
}
pub fn readlinkat(dirfd: isize, path: &str, buf: &mut [u8]) -> isize {
    let mut path_buf = [0u8; USER_PATH_MAX];
    let path = match copy_path_to_stack(path, &mut path_buf) {
        Ok(path) => path,
        Err(err) => return err,
    };
    sys_readlinkat(dirfd, path, buf.as_mut_ptr(), buf.len())
}
pub fn write(fd: usize, buf: &[u8]) -> isize {
    sys_write(fd, buf)
}
pub fn fstat(fd: usize, stat_buf: &mut [u8]) -> isize {
    sys_fstat(fd, stat_buf.as_mut_ptr())
}
pub fn fstatat(dirfd: isize, path: &str, stat_buf: &mut [u8], flags: u32) -> isize {
    let mut path_buf = [0u8; USER_PATH_MAX];
    let path = match copy_path_to_stack(path, &mut path_buf) {
        Ok(path) => path,
        Err(err) => return err,
    };
    sys_fstatat(dirfd, path, stat_buf.as_mut_ptr(), flags)
}
pub fn sync() -> isize {
    sys_sync()
}
pub fn exit(exit_code: i32) -> ! {
    println!("user_lib: exit({})", exit_code);
    sys_exit(exit_code);
}
pub fn poweroff(exit_code: i32) -> ! {
    sys_poweroff(exit_code);
}
pub fn yield_() -> isize {
    sys_yield()
}
pub fn uname(buf: &mut [u8]) -> isize {
    sys_uname(buf.as_mut_ptr())
}

pub fn get_time() -> isize {
    let mut time = TimeVal::new();
    match sys_get_time(&mut time, 0) {
        0 => ((time.sec & 0xffff) * 1000 + time.usec / 1000) as isize,
        _ => -1,
    }
}
pub fn getpid() -> isize {
    sys_getpid()
}
pub fn gettid() -> isize {
    sys_gettid()
}
pub fn thread_create(entry: extern "C" fn(usize) -> !, arg: usize) -> isize {
    sys_thread_create(entry as usize, arg)
}

pub fn readahead(fd: usize, offset: usize, count: usize) -> isize {
    sys_readahead(fd, offset, count)
}

pub fn fadvise64(fd: usize, offset: usize, len: usize, advice: i32) -> isize {
    sys_fadvise64(fd, offset, len, advice)
}

pub fn kill(pid: isize, sig: usize) -> isize {
    sys_kill(pid, sig)
}

pub fn sigaction(signum: i32, act: Option<&SigAction>, oldact: Option<&mut SigAction>) -> isize {
    let kernel_act = act.map(to_kernel_sigaction);
    let mut kernel_old = KernelSigAction::default();
    let act_ptr = kernel_act.as_ref().map_or(core::ptr::null(), |a| {
        a as *const KernelSigAction as *const u8
    });
    let old_ptr = if oldact.is_some() {
        &mut kernel_old as *mut KernelSigAction as *mut u8
    } else {
        core::ptr::null_mut()
    };
    let ret = sys_rt_sigaction(signum, act_ptr, old_ptr, core::mem::size_of::<SignalSet>());
    if ret == 0 {
        if let Some(oldact) = oldact {
            *oldact = from_kernel_sigaction(&kernel_old);
        }
    }
    ret
}

pub fn sigprocmask(how: i32, set: Option<&SignalSet>, oldset: Option<&mut SignalSet>) -> isize {
    let set_ptr = set.map_or(core::ptr::null(), |s| s as *const SignalSet);
    let old_ptr = oldset.map_or(core::ptr::null_mut(), |s| s as *mut SignalSet);
    sys_rt_sigprocmask(how, set_ptr, old_ptr, core::mem::size_of::<SignalSet>())
}
pub fn fork() -> isize {
    sys_fork()
}

pub fn execve(path: &str, argv: &[&str], envp: &[&str]) -> isize {
    let path = CString::new(path).unwrap();
    let argv: Vec<_> = argv.iter().map(|s| CString::new(*s).unwrap()).collect();
    let envp: Vec<_> = envp.iter().map(|s| CString::new(*s).unwrap()).collect();
    let mut argv = argv.iter().map(|s| s.as_ptr() as usize).collect::<Vec<_>>();
    let mut envp = envp.iter().map(|s| s.as_ptr() as usize).collect::<Vec<_>>();
    argv.push(0);
    envp.push(0);
    sys_execve(path.as_ptr() as *const u8, argv.as_ptr(), envp.as_ptr())
}

pub fn wait(exit_code: &mut i32) -> isize {
    loop {
        match sys_waitpid(-1, exit_code as *mut _) {
            -4 => continue,
            -2 => {
                yield_();
            }
            // -1 or a real pid
            exit_pid => return exit_pid,
        }
    }
}

pub fn waitpid(pid: usize, exit_code: &mut i32) -> isize {
    loop {
        match sys_waitpid(pid as isize, exit_code as *mut _) {
            -4 => continue,
            -2 => {
                yield_();
            }
            // -1 or a real pid
            exit_pid => return exit_pid,
        }
    }
}

pub fn waitpid_options(pid: isize, exit_code: &mut i32, options: i32) -> isize {
    sys_waitpid_options(pid, exit_code as *mut _, options)
}

pub fn sleep(period_ms: usize) {
    let start = get_time();
    if start < 0 {
        return;
    }

    let deadline = start.saturating_add(period_ms as isize);
    loop {
        let now = get_time();
        if now < 0 || now >= deadline {
            break;
        }
        sys_yield();
    }
}

pub fn mmap(
    start: usize,
    len: usize,
    prot: usize,
    flags: usize,
    fd: isize,
    offset: usize,
) -> isize {
    sys_mmap(start, len, prot, flags, fd, offset)
}

pub fn munmap(start: usize, len: usize) -> isize {
    sys_munmap(start, len)
}
pub fn socket(domain: i32, type_: i32, protocol: i32) -> isize {
    sys_socket(domain, type_, protocol)
}

pub fn listen(fd: usize, backlog: usize) -> isize {
    sys_listen(fd, backlog)
}

pub fn accept(fd: usize, addr_ptr: *mut u8, addr_len: *mut usize) -> isize {
    sys_accept(fd, addr_ptr, addr_len)
}

pub fn connect(fd: usize, addr_ptr: *const u8, addr_len: usize) -> isize {
    sys_connect(fd, addr_ptr, addr_len)
}

pub fn shutdown(fd: usize, how: i32) -> isize {
    sys_shutdown(fd, how)
}

pub fn ssh_connect(fd: usize, client_ident: &str) -> isize {
    sys_ssh_connect(fd, client_ident.as_ptr(), client_ident.len())
}

pub fn ssh_write(ssh_id: usize, buf: &[u8]) -> isize {
    sys_ssh_write(ssh_id, buf.as_ptr(), buf.len())
}

pub fn ssh_read(ssh_id: usize, buf: &mut [u8]) -> isize {
    sys_ssh_read(ssh_id, buf.as_mut_ptr(), buf.len())
}

pub fn ssh_close(ssh_id: usize) -> isize {
    sys_ssh_close(ssh_id)
}

pub fn ssh_peer_ident(ssh_id: usize, buf: &mut [u8]) -> isize {
    sys_ssh_peer_ident(ssh_id, buf.as_mut_ptr(), buf.len())
}

pub fn ssh_auth_password(ssh_id: usize, username: &str, password: &str) -> isize {
    sys_ssh_auth_password(
        ssh_id,
        username.as_ptr(),
        username.len(),
        password.as_ptr(),
        password.len(),
    )
}

pub fn ssh_auth_publickey(ssh_id: usize, username: &str, private_key: &[u8]) -> isize {
    sys_ssh_auth_publickey(
        ssh_id,
        username.as_ptr(),
        username.len(),
        private_key.as_ptr(),
        private_key.len(),
    )
}

pub fn ssh_exec(ssh_id: usize, command: &str) -> isize {
    sys_ssh_exec(ssh_id, command.as_ptr(), command.len())
}

pub fn ssh_shell(ssh_id: usize) -> isize {
    sys_ssh_shell(ssh_id)
}

pub fn ssh_channel_read(ssh_id: usize, channel_id: usize, buf: &mut [u8]) -> isize {
    sys_ssh_channel_read(ssh_id, channel_id, buf.as_mut_ptr(), buf.len())
}

pub fn ssh_channel_try_read(ssh_id: usize, channel_id: usize, buf: &mut [u8]) -> isize {
    sys_ssh_channel_try_read(ssh_id, channel_id, buf.as_mut_ptr(), buf.len())
}

pub fn ssh_channel_write(ssh_id: usize, channel_id: usize, buf: &[u8]) -> isize {
    sys_ssh_channel_write(ssh_id, channel_id, buf.as_ptr(), buf.len())
}

pub fn ssh_channel_close(ssh_id: usize, channel_id: usize) -> isize {
    sys_ssh_channel_close(ssh_id, channel_id)
}

pub fn ssh_channel_status(ssh_id: usize, channel_id: usize) -> isize {
    sys_ssh_channel_status(ssh_id, channel_id)
}

pub fn ssh_connect_raw(fd: usize, ident: *const u8, ident_len: usize) -> isize {
    sys_ssh_connect(fd, ident, ident_len)
}

pub fn ssh_write_raw(ssh_id: usize, buf: *const u8, len: usize) -> isize {
    sys_ssh_write(ssh_id, buf, len)
}

pub fn ssh_read_raw(ssh_id: usize, buf: *mut u8, len: usize) -> isize {
    sys_ssh_read(ssh_id, buf, len)
}

pub fn ssh_peer_ident_raw(ssh_id: usize, buf: *mut u8, len: usize) -> isize {
    sys_ssh_peer_ident(ssh_id, buf, len)
}

pub fn sendto(
    fd: usize,
    buf_ptr: *const u8,
    len: usize,
    _flags: i32,
    addr_ptr: *const u8,
    addr_len: usize,
) -> isize {
    sys_sendto(fd, buf_ptr, len, _flags, addr_ptr, addr_len)
}

pub fn recvfrom(
    fd: usize,
    buf_ptr: *mut u8,
    len: usize,
    _flags: i32,
    addr_ptr: *mut u8,
    addr_len: *mut usize,
) -> isize {
    sys_recvfrom(fd, buf_ptr, len, _flags, addr_ptr, addr_len)
}

pub fn sendmsg(fd: usize, msg_ptr: usize, flags: i32) -> isize {
    sys_sendmsg(fd, msg_ptr, flags)
}

pub fn recvmsg(fd: usize, msg_ptr: usize, flags: i32) -> isize {
    sys_recvmsg(fd, msg_ptr, flags)
}

pub fn bind(fd: usize, addr_ptr: *const u8, addr_len: usize) -> isize {
    sys_bind(fd, addr_ptr, addr_len)
}

pub fn setpgid(pid: i32, pgid: i32) -> isize {
    sys_setpgid(pid as usize, pgid as usize)
}

pub fn fcntl(fd: usize, cmd: usize, arg: usize) -> isize {
    sys_fcntl(fd, cmd, arg)
}

pub fn ioctl(fd: usize, request: usize, argp: usize) -> isize {
    sys_ioctl(fd, request, argp)
}

pub fn setsockopt(fd: usize, level: i32, optname: i32, optval: *const u8, optlen: usize) -> isize {
    sys_setsockopt(fd, level, optname, optval, optlen)
}

pub fn getsockopt(fd: usize, level: i32, optname: i32, optval: *mut u8, optlen: *mut u32) -> isize {
    sys_getsockopt(fd, level, optname, optval, optlen)
}
