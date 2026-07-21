#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::arch::asm;
use user_lib::{OpenFlags, close, fadvise64, fstat, open, pipe, read, readahead, unlinkat};

const AT_FDCWD: isize = -100;
const AT_EMPTY_PATH: usize = 0x1000;
const SYS_DUP: usize = 23;
const SYS_FLOCK: usize = 32;
const SYS_FACCESSAT2: usize = 439;
const LOCK_SH: usize = 1;
const LOCK_EX: usize = 2;
const LOCK_NB: usize = 4;
const LOCK_UN: usize = 8;
const EAGAIN: isize = -11;
const EBADF: isize = -9;
const EINVAL: isize = -22;
const ESPIPE: isize = -29;

#[cfg(target_arch = "riscv64")]
unsafe fn raw_syscall(id: usize, args: [usize; 6]) -> isize {
    let ret: isize;
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
unsafe fn raw_syscall(id: usize, args: [usize; 6]) -> isize {
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

fn flock(fd: usize, operation: usize) -> isize {
    unsafe { raw_syscall(SYS_FLOCK, [fd, operation, 0, 0, 0, 0]) }
}

fn test_faccessat2(fd: usize) -> bool {
    let path = b"/tmp/buildstorm_compat_lock\0";
    let empty = b"\0";
    let existing = unsafe {
        raw_syscall(SYS_FACCESSAT2, [
            AT_FDCWD as usize,
            path.as_ptr() as usize,
            0,
            0,
            0,
            0,
        ])
    };
    let empty_path = unsafe {
        raw_syscall(SYS_FACCESSAT2, [
            fd,
            empty.as_ptr() as usize,
            0,
            AT_EMPTY_PATH,
            0,
            0,
        ])
    };
    let invalid = unsafe {
        raw_syscall(SYS_FACCESSAT2, [
            AT_FDCWD as usize,
            path.as_ptr() as usize,
            0,
            0x8000_0000,
            0,
            0,
        ])
    };
    println!(
        "[buildstorm_compat_test] faccessat2 existing={} empty={} invalid={}",
        existing, empty_path, invalid
    );
    existing == 0 && empty_path == 0 && invalid == EINVAL
}

fn test_flock(first: usize, second: usize) -> bool {
    let duplicate = unsafe { raw_syscall(SYS_DUP, [first, 0, 0, 0, 0, 0]) };
    if duplicate < 0 {
        println!("[buildstorm_compat_test] dup={}", duplicate);
        return false;
    }
    let first_exclusive = flock(first, LOCK_EX);
    let first_close = close(first);
    let conflict = flock(second, LOCK_EX | LOCK_NB);
    let first_unlock = flock(duplicate as usize, LOCK_UN);
    let second_shared = flock(second, LOCK_SH);
    let first_shared = flock(duplicate as usize, LOCK_SH | LOCK_NB);
    let second_unlock = flock(second, LOCK_UN);
    let final_unlock = flock(duplicate as usize, LOCK_UN);
    let duplicate_close = close(duplicate as usize);
    println!(
        "[buildstorm_compat_test] flock dup={} ex={} close={} conflict={} unlock={} sh2={} sh1={} unlock2={} final={} dup_close={}",
        duplicate,
        first_exclusive,
        first_close,
        conflict,
        first_unlock,
        second_shared,
        first_shared,
        second_unlock,
        final_unlock,
        duplicate_close
    );
    duplicate >= 0
        && first_exclusive == 0
        && first_close == 0
        && conflict == EAGAIN
        && first_unlock == 0
        && second_shared == 0
        && first_shared == 0
        && second_unlock == 0
        && final_unlock == 0
        && duplicate_close == 0
}

fn test_overcommit_memory() -> bool {
    let fd = open(
        AT_FDCWD,
        "/proc/sys/vm/overcommit_memory",
        OpenFlags::RDONLY,
        0,
    );
    if fd < 0 {
        println!("[buildstorm_compat_test] overcommit open={}", fd);
        return false;
    }
    let mut buffer = [0u8; 16];
    let count = read(fd as usize, &mut buffer);
    close(fd as usize);
    if count <= 0 {
        println!("[buildstorm_compat_test] overcommit read={}", count);
        return false;
    }
    let value = core::str::from_utf8(&buffer[..count as usize])
        .ok()
        .and_then(|text| text.trim().parse::<usize>().ok());
    println!("[buildstorm_compat_test] overcommit value={:?}", value);
    value.is_some_and(|value| value <= 2)
}

fn test_fadvise64(fd: usize) -> bool {
    let mut valid = true;
    for advice in 0..=5 {
        valid &= fadvise64(fd, 0, 0, advice) == 0;
    }
    let invalid_advice = fadvise64(fd, 0, 0, 6);
    let invalid_offset = fadvise64(fd, usize::MAX, 0, 0);
    let invalid_fd = fadvise64(usize::MAX, 0, 0, 0);
    println!(
        "[buildstorm_compat_test] fadvise valid={} advice={} offset={} fd={}",
        valid, invalid_advice, invalid_offset, invalid_fd
    );
    valid && invalid_advice == EINVAL && invalid_offset == EINVAL && invalid_fd == EBADF
}

fn test_pipe_file_hints() -> bool {
    let mut fds = [-1i32; 2];
    let pipe_result = pipe(&mut fds);
    if pipe_result != 0 {
        println!("[buildstorm_compat_test] pipe create={}", pipe_result);
        return false;
    }
    let mut stat = [0u8; 128];
    let fstat_result = fstat(fds[1] as usize, &mut stat);
    let readahead_result = readahead(fds[0] as usize, 0, 4096);
    let fadvise_result = fadvise64(fds[1] as usize, 0, 0, 0);
    let read_close = close(fds[0] as usize);
    let write_close = close(fds[1] as usize);
    println!(
        "[buildstorm_compat_test] pipe hints fstat={} readahead={} fadvise={} close=({}, {})",
        fstat_result, readahead_result, fadvise_result, read_close, write_close
    );
    fstat_result == 0
        && readahead_result == EINVAL
        && fadvise_result == ESPIPE
        && read_close == 0
        && write_close == 0
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[buildstorm_compat_test] start");
    let path = "/tmp/buildstorm_compat_lock";
    let first = open(AT_FDCWD, path, OpenFlags::O_CREAT | OpenFlags::RDWR, 0o600);
    let second = open(AT_FDCWD, path, OpenFlags::RDWR, 0);
    if first < 0 || second < 0 {
        println!(
            "[buildstorm_compat_test] lockfile open first={} second={}",
            first, second
        );
        if first >= 0 {
            close(first as usize);
        }
        if second >= 0 {
            close(second as usize);
        }
        unlinkat(AT_FDCWD, path, 0);
        return 1;
    }

    let faccessat2_ok = test_faccessat2(first as usize);
    let fadvise_ok = test_fadvise64(first as usize);
    let pipe_hints_ok = test_pipe_file_hints();
    let flock_ok = test_flock(first as usize, second as usize);
    let overcommit_ok = test_overcommit_memory();
    close(second as usize);
    unlinkat(AT_FDCWD, path, 0);

    if faccessat2_ok && fadvise_ok && pipe_hints_ok && flock_ok && overcommit_ok {
        println!("[buildstorm_compat_test] PASS");
        0
    } else {
        println!("[buildstorm_compat_test] FAIL");
        1
    }
}
