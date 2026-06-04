#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use user_lib::{
    AT_FDCWD, OpenFlags, chdir, close, execve, fork, getdents64, mkdir, open, poweroff, symlinkat,
    unlinkat, wait, waitpid, yield_,
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

const TEST_SCRIPT_DIRS: &[&str] = &["/", "/musl", "/glibc"];
const AUTO_TEST_DISABLE_FLAG: &str = "/.initproc-no-autotest";

/// Busybox 常用命令列表。比赛测试（lmbench/libctest 等）通常需要这些。
const BUSYBOX_CMDS: &[&str] = &[
    // 文件操作
    "ls", "cp", "mv", "rm", "cat", "mkdir", "rmdir", "touch", "ln", "readlink", "realpath",
    "chmod", "chown", "chgrp", "df", "du", "sync",
    // 文本处理
    "echo", "printf", "head", "tail", "grep", "sed", "awk", "cut", "sort", "uniq", "wc",
    "tr", "tee", "basename", "dirname", "seq", "hexdump",
    // shell / 流程控制
    "sh", "test", "[", "expr", "true", "false", "yes", "env", "exit",
    // 进程 / 系统
    "ps", "kill", "pidof", "pgrep", "pkill", "top", "uptime", "free",
    "mount", "umount", "dmesg", "insmod", "rmmod", "lsmod",
    // 网络
    "ifconfig", "ping", "wget", "nc", "netstat", "route", "traceroute",
    // 其他常用
    "sleep", "usleep", "date", "id", "whoami", "hostname", "clear", "reset",
    "pwd", "mknod", "mktemp", "stat", "watch", "xargs", "find", "which",
    
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

fn push_test_script_path(dir: &str, name: &str, out: &mut Vec<String>) {
    if dir == "/" {
        out.push(alloc::format!("/{}", name));
    } else {
        out.push(alloc::format!("{}/{}", dir, name));
    }
}

fn parse_dirents_collect(dir: &str, buf: &[u8], out: &mut Vec<String>) {
    let mut offset = 0usize;
    while offset + 19 <= buf.len() {
        let reclen = u16::from_ne_bytes([buf[offset + 16], buf[offset + 17]]) as usize;
        if reclen == 0 || offset + reclen > buf.len() {
            break;
        }

        let name_start = offset + 19;
        let mut name_end = name_start;
        while name_end < offset + reclen && buf[name_end] != 0 {
            name_end += 1;
        }

        if let Ok(name) = core::str::from_utf8(&buf[name_start..name_end]) {
            if name.ends_with("_testcode.sh") {
                push_test_script_path(dir, name, out);
            }
        }

        offset += reclen;
    }
}

fn collect_test_scripts_in_dir(dir: &str, scripts: &mut Vec<String>) -> bool {
    let fd = open(AT_FDCWD, dir, OpenFlags::RDONLY | OpenFlags::O_DIRECTORY, 0);
    if fd < 0 {
        if dir == "/" {
            println!("[initproc] cannot open root directory for test scan");
        }
        return false;
    }

    let mut buf = [0u8; 4096];
    loop {
        let nread = getdents64(fd as usize, &mut buf);
        if nread <= 0 {
            break;
        }
        parse_dirents_collect(dir, &buf[..nread as usize], scripts);
    }
    close(fd as usize);
    true
}

fn find_test_scripts() -> Vec<String> {
    let mut scripts = Vec::new();
    for dir in TEST_SCRIPT_DIRS.iter() {
        collect_test_scripts_in_dir(dir, &mut scripts);
    }
    scripts.sort();
    scripts
}

fn script_workdir_and_name(path: &str) -> (&str, &str) {
    match path.rsplit_once('/') {
        Some(("", name)) => ("/", name),
        Some((dir, name)) => (dir, name),
        None => (".", path),
    }
}

fn env_for_script(path: &str) -> &'static [&'static str] {
    if path.starts_with("/glibc/") {
        GLIBC_ENV
    } else {
        ENV
    }
}

fn run_test_script(path: &str) -> i32 {
    let pid = fork();
    if pid == 0 {
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

    let mut exit_code = 0;
    let waited = waitpid(pid as usize, &mut exit_code);
    if waited != pid {
        println!(
            "[initproc] waitpid failed for {}, pid={}, waited={}",
            path, pid, waited
        );
        return 127;
    }
    exit_code
}

fn run_official_tests_if_present() -> bool {
    let scripts = find_test_scripts();
    if scripts.is_empty() {
        return false;
    }

    println!("[initproc] found {} official test script(s)", scripts.len());
    let mut last_exit = 0;
    for script in scripts.iter() {
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
        if chdir("/musl") < 0 {
            println!("[initproc] failed to chdir /musl, keeping current directory");
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
