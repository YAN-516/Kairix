#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::mem::size_of;
use user_lib::{OpenFlags, close, exit, fcntl, fork, getpid, open, pipe, read, waitpid, write};

const AT_FDCWD: isize = -100;
const F_GETLK: usize = 5;
const F_SETLK: usize = 6;
const F_SETLKW: usize = 7;
const F_WRLCK: i16 = 1;
const F_UNLCK: i16 = 2;
const SEEK_SET: i16 = 0;

#[repr(C)]
#[derive(Clone, Copy)]
struct Flock {
    lock_type: i16,
    whence: i16,
    start: i64,
    len: i64,
    pid: i32,
    _padding: i32,
}

const _: [(); 32] = [(); size_of::<Flock>()];

impl Flock {
    const fn new(lock_type: i16) -> Self {
        Self {
            lock_type,
            whence: SEEK_SET,
            start: 0,
            len: 0,
            pid: 0,
            _padding: 0,
        }
    }
}

fn set_lock(fd: usize, cmd: usize, lock_type: i16) -> isize {
    let lock = Flock::new(lock_type);
    fcntl(fd, cmd, &lock as *const Flock as usize)
}

fn read_byte(fd: usize) -> bool {
    let mut byte = [0u8; 1];
    loop {
        match read(fd, &mut byte) {
            1 => return true,
            -4 => continue,
            _ => return false,
        }
    }
}

fn write_byte(fd: usize) -> bool {
    let byte = [1u8; 1];
    loop {
        match write(fd, &byte) {
            1 => return true,
            -4 => continue,
            _ => return false,
        }
    }
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[fcntl_lock_test] start");
    let fd = open(
        AT_FDCWD,
        "/tmp/fcntl_lock_test.db",
        OpenFlags::RDWR | OpenFlags::O_CREAT | OpenFlags::O_TRUNC,
        0o600,
    );
    if fd < 0 || set_lock(fd as usize, F_SETLK, F_WRLCK) != 0 {
        println!("[fcntl_lock_test] FAIL: parent lock fd={}", fd);
        return 1;
    }

    let parent_pid = getpid() as i32;
    let mut events = [-1i32; 2];
    if pipe(&mut events) != 0 {
        println!("[fcntl_lock_test] FAIL: pipe");
        return 1;
    }

    let child = fork();
    if child == 0 {
        let _ = close(events[0] as usize);
        let mut query = Flock::new(F_WRLCK);
        let getlk = fcntl(fd as usize, F_GETLK, &mut query as *mut Flock as usize);
        if getlk != 0 || query.lock_type != F_WRLCK || query.pid != parent_pid {
            println!(
                "[fcntl_lock_test] child F_GETLK fail: ret={} type={} pid={}",
                getlk, query.lock_type, query.pid
            );
            exit(2);
        }
        let nonblock = set_lock(fd as usize, F_SETLK, F_WRLCK);
        if nonblock != -11 && nonblock != -13 {
            println!(
                "[fcntl_lock_test] child F_SETLK expected conflict, ret={}",
                nonblock
            );
            exit(3);
        }
        if !write_byte(events[1] as usize) {
            exit(4);
        }
        if set_lock(fd as usize, F_SETLKW, F_WRLCK) != 0 {
            exit(5);
        }
        if !write_byte(events[1] as usize) {
            exit(6);
        }
        let _ = set_lock(fd as usize, F_SETLK, F_UNLCK);
        exit(0);
    }
    if child < 0 {
        println!("[fcntl_lock_test] FAIL: fork ret={}", child);
        return 1;
    }

    let _ = close(events[1] as usize);
    if !read_byte(events[0] as usize) {
        println!("[fcntl_lock_test] FAIL: child readiness");
        return 1;
    }
    if set_lock(fd as usize, F_SETLK, F_UNLCK) != 0 {
        println!("[fcntl_lock_test] FAIL: parent unlock");
        return 1;
    }
    if !read_byte(events[0] as usize) {
        println!("[fcntl_lock_test] FAIL: blocked child did not wake");
        return 1;
    }

    let mut status = 0;
    let waited = waitpid(child as usize, &mut status);
    let _ = close(events[0] as usize);
    let _ = close(fd as usize);
    let _ = user_lib::unlinkat(AT_FDCWD, "/tmp/fcntl_lock_test.db", 0);
    if waited == child && status == 0 {
        println!("[fcntl_lock_test] PASS");
        0
    } else {
        println!(
            "[fcntl_lock_test] FAIL: child={} waited={} status={}",
            child, waited, status
        );
        1
    }
}
