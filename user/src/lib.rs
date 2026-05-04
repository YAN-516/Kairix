#![no_std]
#![feature(linkage)]
#![feature(alloc_error_handler)]

#[macro_use]
pub mod console;
mod lang_items;
mod syscall;

extern crate alloc;
#[macro_use]
extern crate bitflags;

use alloc::{ffi::CString, vec::Vec};

use buddy_system_allocator::LockedHeap;
use core::ptr::addr_of_mut;
use syscall::*;

const USER_HEAP_SIZE: usize = 32768;

static mut HEAP_SPACE: [u8; USER_HEAP_SIZE] = [0; USER_HEAP_SIZE];

#[global_allocator]
static HEAP: LockedHeap<32> = LockedHeap::empty();

#[alloc_error_handler]
pub fn handle_alloc_error(layout: core::alloc::Layout) -> ! {
    panic!("Heap allocation error, layout = {:?}", layout);
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _start(argc: usize, argv: usize) -> ! {
    unsafe {
        HEAP.lock()
            .init(addr_of_mut!(HEAP_SPACE) as usize, USER_HEAP_SIZE);
    }
    exit(main_with_args(argc, argv as *const usize));
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
}

impl SigAction {
    pub const fn default() -> Self {
        Self {
            sa_handler: SigHandler::Default,
            sa_mask: SignalSet::empty(),
            sa_flags: 0,
        }
    }

    pub const fn ignore() -> Self {
        Self {
            sa_handler: SigHandler::Ignore,
            sa_mask: SignalSet::empty(),
            sa_flags: 0,
        }
    }

    pub const fn custom(handler: unsafe extern "C" fn(i32)) -> Self {
        Self {
            sa_handler: SigHandler::Custom(handler),
            sa_mask: SignalSet::empty(),
            sa_flags: 0,
        }
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
    let path = CString::new(path).unwrap();
    sys_mkdir(-100, path.as_ptr() as *const u8, _mode)
}

pub fn unlinkat(dirfd: isize, path: &str, flags: u32) -> isize {
    let path = CString::new(path).unwrap();
    sys_unlinkat(dirfd, path.as_ptr() as *const u8, flags)
}

pub const AT_FDCWD: isize = -100;

pub fn symlinkat(target: &str, newdirfd: isize, linkpath: &str) -> isize {
    let target = CString::new(target).unwrap();
    let linkpath = CString::new(linkpath).unwrap();
    sys_symlinkat(
        target.as_ptr() as *const u8,
        newdirfd,
        linkpath.as_ptr() as *const u8,
    )
}

pub fn linkat(
    olddirfd: isize,
    oldpath: &str,
    newdirfd: isize,
    newpath: &str,
    _flags: u32,
) -> isize {
    let oldpath = CString::new(oldpath).unwrap();
    let newpath = CString::new(newpath).unwrap();
    sys_linkat(
        olddirfd,
        oldpath.as_ptr() as *const u8,
        newdirfd,
        newpath.as_ptr() as *const u8,
        _flags,
    )
}

pub fn umount2(target: &str, flags: u32) -> isize {
    let target = CString::new(target).unwrap();
    sys_umount2(target.as_ptr() as *const u8, flags)
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
    let path = CString::new(path).unwrap();
    sys_chdir(path.as_ptr() as *const u8)
}

pub fn open(dirfd: isize, path: &str, flags: OpenFlags, _mode: u32) -> isize {
    let path = CString::new(path).unwrap();
    sys_openat(dirfd, path.as_ptr() as *const u8, flags.bits())
}
pub fn close(fd: usize) -> isize {
    sys_close(fd)
}
pub fn getdents64(fd: usize, buf: &mut [u8]) -> isize {
    sys_getdents64(fd, buf.as_mut_ptr(), buf.len())
}
pub fn read(fd: usize, buf: &mut [u8]) -> isize {
    sys_read(fd, buf)
}
pub fn write(fd: usize, buf: &[u8]) -> isize {
    sys_write(fd, buf)
}
pub fn fstat(fd: usize, stat_buf: &mut [u8]) -> isize {
    sys_fstat(fd, stat_buf.as_mut_ptr())
}
pub fn exit(exit_code: i32) -> ! {
    sys_exit(exit_code);
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

pub fn kill(pid: isize, sig: usize) -> isize {
    sys_kill(pid, sig)
}

pub fn sigaction(signum: i32, act: Option<&SigAction>, oldact: Option<&mut SigAction>) -> isize {
    let act_ptr = act.map_or(core::ptr::null(), |a| a as *const SigAction);
    let old_ptr = oldact.map_or(core::ptr::null_mut(), |a| a as *mut SigAction);
    sys_rt_sigaction(signum, act_ptr, old_ptr, core::mem::size_of::<SignalSet>())
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
            -2 => {
                yield_();
            }
            // -1 or a real pid
            exit_pid => return exit_pid,
        }
    }
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

pub fn bind(fd: usize, addr_ptr: *const u8, addr_len: usize) -> isize {
    sys_bind(fd, addr_ptr, addr_len)
}


pub fn setpgid(pid: i32, pgid: i32) -> isize {
    sys_setpgid(pid as usize, pgid as usize)
}

pub fn ioctl(fd: usize, request: usize, argp: usize) -> isize {
    sys_ioctl(fd, request, argp) 
}