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
pub extern "C" fn _start() -> ! {
    unsafe {
        HEAP.lock()
            .init(addr_of_mut!(HEAP_SPACE) as usize, USER_HEAP_SIZE);
    }
    exit(main());
}

#[linkage = "weak"]
#[unsafe(no_mangle)]
fn main() -> i32 {
    panic!("Cannot find main!");
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

pub fn linkat(olddirfd: isize, oldpath: &str, newdirfd: isize, newpath: &str, _flags: u32) -> isize {
    let oldpath = CString::new(oldpath).unwrap();
    let newpath = CString::new(newpath).unwrap();
    sys_linkat(olddirfd, oldpath.as_ptr() as *const u8, newdirfd, newpath.as_ptr() as *const u8, _flags)
}

pub fn umount2(target: &str, flags: u32) -> isize {
    let target = CString::new(target).unwrap();
    sys_umount2(target.as_ptr() as *const u8, flags)
}

pub fn mount(special:&mut [u8],dir:&mut [u8],fstype:&mut [u8],flags:isize,data:&mut [u8])-> isize{
    sys_mount(special.as_mut_ptr() as *const u8, dir.as_mut_ptr() as *const u8, fstype.as_mut_ptr() as *const u8, flags as isize, data.as_mut_ptr() as *const u8)
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
    let time = TimeVal::new();
    match sys_get_time(&time, 0) {
        0 => ((time.sec & 0xffff) * 1000 + time.usec / 1000) as isize,
        _ => -1,
    }
}
pub fn getpid() -> isize {
    sys_getpid()
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
    while get_time() < start + period_ms as isize {
        sys_yield();
    }
}

pub fn mmap(start: usize, len: usize, prot: usize, flags: usize, fd: isize, offset: usize) -> isize {
    sys_mmap(start, len, prot, flags, fd, offset)
}

pub fn munmap(start: usize, len: usize) -> isize {
    sys_munmap(start, len)
}