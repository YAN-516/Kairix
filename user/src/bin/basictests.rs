#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{execve, fork, waitpid};

static MUSL_BASIC_TESTS: &[&str] = &[
    "chdir",
    "clone",
    "close",
    "dup",
    "dup2",
    "execve",
    "exit",
    "fork",
    //"fstat",
    "getcwd",
    "getdents",
    "getpid",
    "getppid",
    "gettimeofday",
    "mkdir_",
    //"mmap",
    //"mount",
    //"munmap",
    "open",
    "openat",
    "pipe",
    "read",
    "sleep",
    "test_echo",
    "times",
    //"umount",
    //"uname",
    //"unlink",
    "wait",
    "waitpid",
    "write",
    "yield",
    //"brk",
];

fn run_musl_tests(tests: &[&str]) -> i32 {
    let mut pass_num = 0;

    for test_name in tests {
        println!("\x1b[34m[Basictest] Running {}...\x1b[0m", test_name);

        let pid = fork();
        if pid == 0 {
            // 参数列表：argv[0] = 程序名, envp = 空
            execve(*test_name, &[*test_name], &[]);

            println!("[Basictest] Error: Failed to execute {}", test_name);
            user_lib::exit(-1);
        } else {
            let mut exit_code: i32 = 0;
            let wait_pid = waitpid(pid as usize, &mut exit_code);

            assert_eq!(pid, wait_pid);

            if exit_code == 0 {
                pass_num += 1;
                println!(
                    "\x1b[32m[Basictest] Test {} PASSED (exit code 0)\x1b[0m",
                    test_name
                );
            } else {
                println!(
                    "\x1b[31m[Basictest] Test {} FAILED (exit code {})\x1b[0m",
                    test_name, exit_code
                );
            }
        }
    }
    pass_num
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("--- Starting Musl Basic Tests ---");
    let total_tests = MUSL_BASIC_TESTS.len() as i32;
    let pass_num = run_musl_tests(MUSL_BASIC_TESTS);

    println!("\n--- Musl Basic Test Summary ---");
    println!(
        "Total: {}, Passed: {}, Failed: {}",
        total_tests,
        pass_num,
        total_tests - pass_num
    );

    if pass_num == total_tests {
        println!("\x1b[32mAll Musl basic tests passed!\x1b[0m");
        0
    } else {
        println!("\x1b[31mSome tests failed.\x1b[0m");
        -1
    }
}
