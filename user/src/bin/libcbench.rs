#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{execve, fork, waitpid};

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("--- Starting libc-bench ---");

    let pid = fork();
    if pid == 0 {
        execve("/libc-bench", &["libc-bench"], &[]);
        println!("[libc-bench] Error: Failed to execute /libc-bench");
        user_lib::exit(-1);
    } else {
        let mut exit_code: i32 = 0;
        let wait_pid = waitpid(pid as usize, &mut exit_code);
        assert_eq!(pid, wait_pid);

        if exit_code == 0 {
            println!("\x1b[32m[libc-bench] PASSED (exit code 0)\x1b[0m");
        } else {
            println!(
                "\x1b[31m[libc-bench] FAILED (exit code {})\x1b[0m",
                exit_code
            );
        }
        // 等待所有子进程结束，然后休眠
        loop {
            let mut code: i32 = 0;
            let wp = user_lib::wait(&mut code);
            if wp == -1 {
                // 没有更多子进程，进入长休眠
                user_lib::sleep(3600);
                continue;
            }
            // println!(
            //     "[libc-bench] Released a zombie process, pid={}, exit_code={}",
            //     wp, code
            // );
        }
    }
}
