#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{AT_FDCWD, OpenFlags, close, fstat, open, unlinkat, utimensat, write};

const PATH: &str = "/utime_realtime_test.tmp";

#[repr(C)]
#[derive(Clone, Copy)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

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

fn stat_bytes(stat: &mut LinuxStat) -> &mut [u8] {
    unsafe {
        core::slice::from_raw_parts_mut(
            stat as *mut LinuxStat as *mut u8,
            core::mem::size_of::<LinuxStat>(),
        )
    }
}

fn read_stat(fd: usize) -> Option<LinuxStat> {
    let mut stat = LinuxStat::default();
    (fstat(fd, stat_bytes(&mut stat)) == 0).then_some(stat)
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    let _ = unlinkat(AT_FDCWD, PATH, 0);
    let fd = open(
        AT_FDCWD,
        PATH,
        OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::RDWR,
        0o600,
    );
    if fd < 0 {
        println!("[utime_realtime_test] FAIL: open={}", fd);
        return 1;
    }
    let fd = fd as usize;
    if write(fd, b"x") != 1 {
        let _ = close(fd);
        let _ = unlinkat(AT_FDCWD, PATH, 0);
        return 2;
    }

    let now_result = utimensat(AT_FDCWD, PATH, core::ptr::null(), 0);
    let now_stat = read_stat(fd);
    let epoch_ok = now_result == 0
        && now_stat.is_some_and(|stat| {
            stat.st_atime_sec >= 1_500_000_000 && stat.st_mtime_sec >= 1_500_000_000
        });

    let explicit = [
        Timespec {
            tv_sec: 1_700_000_123,
            tv_nsec: 123_456_789,
        },
        Timespec {
            tv_sec: 1_700_000_124,
            tv_nsec: 987_654_321,
        },
    ];
    let explicit_result = utimensat(AT_FDCWD, PATH, explicit.as_ptr() as *const u8, 0);
    let explicit_stat = read_stat(fd);
    let explicit_ok = explicit_result == 0
        && explicit_stat.is_some_and(|stat| {
            stat.st_atime_sec == explicit[0].tv_sec
                && stat.st_atime_nsec == explicit[0].tv_nsec
                && stat.st_mtime_sec == explicit[1].tv_sec
                && stat.st_mtime_nsec == explicit[1].tv_nsec
        });

    let _ = close(fd);
    let _ = unlinkat(AT_FDCWD, PATH, 0);
    println!(
        "[utime_realtime_test] now={} explicit={} result={}",
        now_result,
        explicit_result,
        if epoch_ok && explicit_ok {
            "PASS"
        } else {
            "FAIL"
        }
    );
    if epoch_ok && explicit_ok { 0 } else { 3 }
}
