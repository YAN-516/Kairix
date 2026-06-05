#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use user_lib::{
    AT_FDCWD, OpenFlags, chdir, close, execve, fork, kill, mkdir, open, poweroff, setpgid,
    symlinkat, unlinkat, wait, waitpid, waitpid_options, yield_,
};

const ENV: &[&str] = &[
    "PATH=/bin:/sbin:/musl:/glibc:/usr/bin:/musl/ltp/testcases/bin:/glibc/ltp/testcases/bin",
    "LTPROOT=/musl/ltp",
    "HOME=/",
    "TERM=vt100",
];

const GLIBC_ENV: &[&str] = &[
    "PATH=/bin:/sbin:/glibc:/musl:/usr/bin:/glibc/ltp/testcases/bin:/musl/ltp/testcases/bin",
    "LTPROOT=/glibc/ltp",
    "HOME=/",
    "TERM=vt100",
];

/// 自动测试脚本白名单。只会按这里的顺序执行列出的脚本，不再扫描目录。
///
/// 例如：
/// "/musl/libctest_testcode.sh",
/// "/glibc/ltp_testcode.sh",
const TEST_SCRIPTS: &[&str] = &[
    "/musl/ltp_testcode.sh",
    "/glibc/ltp_testcode.sh",
    "/musl/basic_testcode.sh",
    "/musl/busybox_testcode.sh",
    "/musl/cyclictest_testcode.sh",
    "/musl/iperf_testcode.sh",
    "/musl/iozone_testcode.sh",
    "/musl/libcbench_testcode.sh",
    "/musl/libctest_testcode.sh",
    "/musl/lua_testcode.sh",
    // "/musl/lmbench_testcode.sh",
    // "/musl/ltp_testcode.sh",
    "/musl/netperf_testcode.sh",
    // "/musl/unixbench_testcode.sh",

    "/glibc/basic_testcode.sh",
    "/glibc/busybox_testcode.sh",
    "/glibc/cyclictest_testcode.sh",
    "/glibc/iperf_testcode.sh",
    "/glibc/iozone_testcode.sh",
    "/glibc/libcbench_testcode.sh",
    "/glibc/libctest_testcode.sh",
    "/glibc/lua_testcode.sh",
    // "/glibc/lmbench_testcode.sh",
    // "/glibc/ltp_testcode.sh",
    "/glibc/netperf_testcode.sh",
    // "/glibc/unixbench_testcode.sh",
];
const AUTO_TEST_DISABLE_FLAG: &str = "/.initproc-no-autotest";
const SIGKILL: usize = 9;
const WNOHANG: i32 = 1;

/// Busybox 常用命令列表。比赛测试（lmbench/libctest 等）通常需要这些。
const BUSYBOX_CMDS: &[&str] = &[
    // 文件操作
    "ls", "cp", "mv", "rm", "cat", "mkdir", "rmdir", "touch", "ln", "readlink", "realpath",
    "chmod", "chown", "chgrp", "df", "du", "sync",
    // 文本处理
    "echo", "printf", "head", "tail", "grep", "sed", "awk", "cut", "sort", "uniq", "wc", "tr",
    "tee", "basename", "dirname", "seq", "hexdump",
    // shell / 流程控制
    "sh", "test", "[", "expr", "true", "false", "yes", "env", "exit",
    // 进程 / 系统
    "ps", "kill", "pidof", "pgrep", "pkill", "top", "uptime", "free", "mount", "umount",
    "dmesg", "insmod", "rmmod", "lsmod",
    // 网络
    "ifconfig", "ping", "wget", "nc", "netstat", "route", "traceroute",
    // 其他常用
    "sleep", "usleep", "date", "id", "whoami", "hostname", "clear", "reset", "pwd", "mknod",
    "mktemp", "stat", "watch", "xargs", "find", "which",
    "mkfs.vfat",
    //busybox里面不存在d
    // "mkfs.xfs","mkfs.bcachefs","mkfs.btrfs","mkfs.ext3","mkfs.ext4",
];

fn setup_busybox_links() {
    // 1. 确保 /bin 目录存在（已存在时忽略 EEXIST）
    let _ = mkdir("/bin", 0o755);

    // 2. 探测 busybox 位置，并关闭探测用的 fd
    let bb_path = {
        let fd = open(AT_FDCWD, "/musl/busybox", OpenFlags::RDONLY, 0);
        if fd >= 0 {
            close(fd as usize);
            "/musl/busybox"
        } else {
            let fd = open(AT_FDCWD, "/bin/busybox", OpenFlags::RDONLY, 0);
            if fd >= 0 {
                close(fd as usize);
                "/bin/busybox"
            } else {
                println!("[initproc] busybox not found, skipping symlink setup");
                return;
            }
        }
    };

    // 3. 批量创建软链接（先删除旧链接，再创建新链接）
    let mut created = 0;
    let mut skipped = 0;
    for cmd in BUSYBOX_CMDS.iter() {
        let linkpath = alloc::format!("/bin/{}", cmd);
        let _ = unlinkat(AT_FDCWD, &linkpath, 0);
        let ret = symlinkat(bb_path, AT_FDCWD, &linkpath);
        if ret >= 0 {
            created += 1;
        } else {
            skipped += 1;
        }
    }

    println!(
        "[initproc] busybox={}, created {} symlinks, skipped {} (already exist or error)",
        bb_path, created, skipped
    );

    // mkfs.ext2/3/4 are installed as real e2fsprogs binaries at kernel boot.
    // Do not replace them with busybox symlinks; busybox in this image does
    // not provide the ext mkfs applets.
}

fn file_exists(path: &str) -> bool {
    let fd = open(AT_FDCWD, path, OpenFlags::RDONLY, 0);
    if fd >= 0 {
        close(fd as usize);
        true
    } else {
        false
    }
}

fn executable_exists(path: &str) -> bool {
    file_exists(path)
}

fn auto_test_disabled() -> bool {
    file_exists(AUTO_TEST_DISABLE_FLAG)
}

fn env_for_script(path: &str) -> &'static [&'static str] {
    if path.starts_with("/glibc/") {
        GLIBC_ENV
    } else {
        ENV
    }
}

fn script_workdir_and_name(path: &str) -> (&str, &str) {
    if let Some(script_name) = path.strip_prefix("/musl/") {
        ("/musl", script_name)
    } else if let Some(script_name) = path.strip_prefix("/glibc/") {
        ("/glibc", script_name)
    } else {
        ("/", path.strip_prefix('/').unwrap_or(path))
    }
}

fn run_test_script(path: &str) -> i32 {
    let pid = fork();
    if pid == 0 {
        let _ = setpgid(0, 0);
        let (workdir, script_name) = script_workdir_and_name(path);
        if chdir(workdir) < 0 {
            println!("[initproc] failed to chdir {} for {}", workdir, path);
            user_lib::exit(127);
        }

        let env = env_for_script(path);
        if executable_exists("/bin/sh") {
            execve("/bin/sh", &["sh", script_name], env);
        }
        if executable_exists("/musl/busybox") {
            execve("/musl/busybox", &["busybox", "sh", script_name], env);
        }
        if executable_exists("/bin/busybox") {
            execve("/bin/busybox", &["busybox", "sh", script_name], env);
        }
        execve(script_name, &[script_name], env);
        println!("[initproc] failed to execute {}", path);
        user_lib::exit(127);
    }

    if pid < 0 {
        println!("[initproc] fork failed for {}", path);
        return 127;
    }

    let _ = setpgid(pid as i32, pid as i32);

    let mut exit_code = 0;
    let waited = waitpid(pid as usize, &mut exit_code);
    if waited != pid {
        println!(
            "[initproc] waitpid failed for {}, pid={}, waited={}",
            path, pid, waited
        );
        cleanup_script_process_group(path, pid);
        return 127;
    }
    cleanup_script_process_group(path, pid);
    exit_code
}

fn reap_script_process_group(pgid: isize) -> usize {
    let mut total = 0;
    loop {
        let mut exit_code = 0;
        let waited = waitpid_options(-pgid, &mut exit_code, WNOHANG);
        if waited <= 0 {
            break;
        }
        total += 1;
    }
    total
}

fn cleanup_script_process_group(script: &str, pgid: isize) {
    if pgid <= 1 {
        return;
    }

    let mut reaped = reap_script_process_group(pgid);
    let ret = kill(-pgid, SIGKILL);
    if ret >= 0 {
        for _ in 0..16 {
            let n = reap_script_process_group(pgid);
            reaped += n;
            if n == 0 {
                yield_();
            }
        }
    }

    println!(
        "[initproc] cleaned {} process_group={} reaped={} kill_ret={}",
        script, pgid, reaped, ret
    );
}

fn run_official_tests_if_present() -> bool {
    if TEST_SCRIPTS.is_empty() {
        return false;
    }

    println!(
        "[initproc] selected {} official test script(s)",
        TEST_SCRIPTS.len()
    );
    let mut last_exit = 0;
    for script in TEST_SCRIPTS.iter() {
        println!("[initproc] running {}", script);
        last_exit = run_test_script(script);
        println!("[initproc] finished {} exit_code={}", script, last_exit);
    }

    loop {
        let mut exit_code = 0;
        if wait(&mut exit_code) < 0 {
            break;
        }
    }

    println!("[initproc] all official test scripts finished, poweroff");
    poweroff(last_exit);
}

fn run_interactive_shell() {
    if fork() == 0 {
        println!("this is child");
        if chdir("/") < 0 {
            println!("[initproc] failed to chdir /, keeping current directory");
        }
        execve("/bin/sh", &["sh"], ENV);
        execve("/musl/busybox", &["busybox", "sh"], ENV);
        execve("/bin/busybox", &["busybox", "sh"], ENV);
        println!("[initproc] failed to start shell");
        user_lib::exit(127);
    }
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("exec init_proc");

    setup_busybox_links();

    if auto_test_disabled() {
        println!(
            "[initproc] auto test disabled by {}, starting shell",
            AUTO_TEST_DISABLE_FLAG
        );
    } else if run_official_tests_if_present() {
        return 0;
    }

    run_interactive_shell();
    println!("this is parent");
    loop {
        let mut exit_code: i32 = 0;

        let pid = wait(&mut exit_code);
        if pid == -1 {
            yield_();
            continue;
        }
    }
}
