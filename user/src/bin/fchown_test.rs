#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{AT_FDCWD, OpenFlags, close, fchown, fstat, open, unlinkat};

const TEST_PATH: &str = "/tmp/kairix-fchown-test";
const ID_UNCHANGED: u32 = u32::MAX;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxStat {
    st_dev: u64,
    st_ino: u64,
    st_mode: u32,
    st_nlink: u32,
    st_uid: u32,
    st_gid: u32,
    st_rdev: u64,
    __pad1: u64,
    st_size: i64,
    st_blksize: i32,
    __pad2: i32,
    st_blocks: u64,
    st_atime_sec: i64,
    st_atime_nsec: i64,
    st_mtime_sec: i64,
    st_mtime_nsec: i64,
    st_ctime_sec: i64,
    st_ctime_nsec: i64,
    __glibc_reserved: [i32; 2],
}

const _: [(); 128] = [(); core::mem::size_of::<LinuxStat>()];

fn stat_fd(fd: usize) -> Result<LinuxStat, isize> {
    let mut stat = LinuxStat::default();
    let bytes = unsafe {
        core::slice::from_raw_parts_mut(
            &mut stat as *mut LinuxStat as *mut u8,
            core::mem::size_of::<LinuxStat>(),
        )
    };
    let ret = fstat(fd, bytes);
    if ret < 0 { Err(ret) } else { Ok(stat) }
}

fn cleanup() {
    let _ = unlinkat(AT_FDCWD, TEST_PATH, 0);
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[fchown_test] start");
    cleanup();

    let fd = open(
        AT_FDCWD,
        TEST_PATH,
        OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::RDWR,
        0o600,
    );
    if fd < 0 {
        println!("[fchown_test] FAIL: open ret={}", fd);
        return 1;
    }
    let fd = fd as usize;

    let ret = fchown(fd, 123, 456);
    let first = stat_fd(fd);
    if ret != 0 || !matches!(first, Ok(stat) if stat.st_uid == 123 && stat.st_gid == 456) {
        println!(
            "[fchown_test] FAIL: initial ret={} stat_ok={}",
            ret,
            first.is_ok()
        );
        let _ = close(fd);
        cleanup();
        return 2;
    }

    let ret = fchown(fd, ID_UNCHANGED, 789);
    let second = stat_fd(fd);
    if ret != 0 || !matches!(second, Ok(stat) if stat.st_uid == 123 && stat.st_gid == 789) {
        println!(
            "[fchown_test] FAIL: sentinel ret={} stat_ok={}",
            ret,
            second.is_ok()
        );
        let _ = close(fd);
        cleanup();
        return 3;
    }

    let bad_fd_ret = fchown(usize::MAX, 0, 0);
    if bad_fd_ret != -9 {
        println!("[fchown_test] FAIL: bad fd ret={}", bad_fd_ret);
        let _ = close(fd);
        cleanup();
        return 4;
    }

    let close_ret = close(fd);
    cleanup();
    if close_ret == 0 {
        println!("[fchown_test] PASS");
        0
    } else {
        println!("[fchown_test] FAIL: close ret={}", close_ret);
        5
    }
}
