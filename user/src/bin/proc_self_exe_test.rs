#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{mount, readlinkat};

const AT_FDCWD: isize = -100;

fn check_exe(label: &str) -> Result<(), i32> {
    let mut target = [0u8; 256];
    let ret = readlinkat(AT_FDCWD, "/proc/self/exe", &mut target);
    if ret <= 0 {
        println!("[proc_self_exe_test] FAIL {} readlinkat={}", label, ret);
        return Err(1);
    }
    let target = match core::str::from_utf8(&target[..ret as usize]) {
        Ok(target) => target,
        Err(_) => {
            println!("[proc_self_exe_test] FAIL {} non-UTF8 target", label);
            return Err(2);
        }
    };
    println!("[proc_self_exe_test] {} target={}", label, target);
    if !target.ends_with("/proc_self_exe_test") {
        println!("[proc_self_exe_test] FAIL {} unexpected target", label);
        return Err(3);
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[proc_self_exe_test] start");

    if let Err(code) = check_exe("before-remount") {
        return code;
    }

    let mut source = *b"proc\0";
    let mut target = *b"/proc\0";
    let mut fstype = *b"proc\0";
    let mut data = [0u8; 1];
    let mount_ret = mount(&mut source, &mut target, &mut fstype, 0, &mut data);
    if mount_ret != 0 {
        println!("[proc_self_exe_test] FAIL remount={}", mount_ret);
        return 4;
    }
    if let Err(code) = check_exe("after-remount") {
        return code + 4;
    }

    println!("[proc_self_exe_test] PASS");
    0
}
