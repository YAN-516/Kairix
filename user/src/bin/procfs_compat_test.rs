#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{OpenFlags, close, open, read};

fn read_proc(path: &str, buffer: &mut [u8]) -> Result<usize, isize> {
    let fd = open(-100, path, OpenFlags::RDONLY, 0);
    if fd < 0 {
        return Err(fd);
    }
    let result = read(fd as usize, buffer);
    close(fd as usize);
    if result < 0 {
        Err(result)
    } else {
        Ok(result as usize)
    }
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[procfs_compat_test] start");
    let mut passed = true;

    let mut filesystems_buf = [0u8; 1024];
    match read_proc("/proc/filesystems", &mut filesystems_buf) {
        Ok(len) => {
            let text = core::str::from_utf8(&filesystems_buf[..len]).unwrap_or("");
            let valid = text.lines().any(|line| line.ends_with("\tproc"));
            println!(
                "[procfs_compat_test] filesystems bytes={} has_proc={}",
                len, valid
            );
            passed &= valid;
        }
        Err(error) => {
            println!("[procfs_compat_test] filesystems error={}", error);
            passed = false;
        }
    }

    let mut cgroup_buf = [0u8; 128];
    match read_proc("/proc/self/cgroup", &mut cgroup_buf) {
        Ok(len) => println!("[procfs_compat_test] cgroup bytes={}", len),
        Err(error) => {
            println!("[procfs_compat_test] cgroup error={}", error);
            passed = false;
        }
    }

    let mut statm_buf = [0u8; 128];
    match read_proc("/proc/self/statm", &mut statm_buf) {
        Ok(len) => {
            let text = core::str::from_utf8(&statm_buf[..len]).unwrap_or("");
            let fields = text.split_ascii_whitespace().count();
            let numeric = text
                .split_ascii_whitespace()
                .all(|field| field.parse::<usize>().is_ok());
            println!(
                "[procfs_compat_test] statm bytes={} fields={} numeric={}",
                len, fields, numeric
            );
            passed &= fields == 7 && numeric;
        }
        Err(error) => {
            println!("[procfs_compat_test] statm error={}", error);
            passed = false;
        }
    }

    if passed {
        println!("[procfs_compat_test] PASS");
        0
    } else {
        println!("[procfs_compat_test] FAIL");
        1
    }
}
