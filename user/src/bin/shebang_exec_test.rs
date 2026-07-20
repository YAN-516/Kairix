#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{AT_FDCWD, OpenFlags, close, execve, fork, open, unlinkat, waitpid, write};

const SCRIPT_PATH: &str = "/tmp/kairix-shebang-test.sh";
const SCRIPT: &[u8] = b"#!/bin/bash --noprofile\n\
if [ -n \"$BASH_VERSION\" ] && [ \"$1\" = payload ]; then\n\
    exit 42\n\
fi\n\
exit 43\n";

fn write_script() -> bool {
    let fd = open(
        AT_FDCWD,
        SCRIPT_PATH,
        OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::WRONLY,
        0o755,
    );
    if fd < 0 {
        return false;
    }

    let mut offset = 0;
    while offset < SCRIPT.len() {
        let written = write(fd as usize, &SCRIPT[offset..]);
        if written <= 0 {
            close(fd as usize);
            return false;
        }
        offset += written as usize;
    }
    close(fd as usize) == 0
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[shebang_exec_test] start");
    if !write_script() {
        println!("[shebang_exec_test] FAIL: could not create script");
        return 1;
    }

    let child = fork();
    if child == 0 {
        let ret = execve(SCRIPT_PATH, &["custom-argv0", "payload"], &[
            "PATH=/usr/bin:/bin",
        ]);
        println!("[shebang_exec_test] FAIL: execve returned {}", ret);
        return 127;
    }
    if child < 0 {
        let _ = unlinkat(AT_FDCWD, SCRIPT_PATH, 0);
        println!("[shebang_exec_test] FAIL: fork returned {}", child);
        return 1;
    }

    let mut status = 0;
    let waited = waitpid(child as usize, &mut status);
    let _ = unlinkat(AT_FDCWD, SCRIPT_PATH, 0);
    let exit_code = (status >> 8) & 0xff;
    if waited == child && status & 0x7f == 0 && exit_code == 42 {
        println!("[shebang_exec_test] PASS");
        0
    } else {
        println!(
            "[shebang_exec_test] FAIL: child={} waited={} status={} exit_code={}",
            child, waited, status, exit_code
        );
        1
    }
}
