#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use user_lib::{
    AT_FDCWD, OpenFlags, chdir, close, execve, fork, getdents64, kill, mkdir, open, poweroff,
    setpgid, symlinkat, unlinkat, wait, waitpid, waitpid_options, yield_,
};

const ENV: &[&str] = &[
    "PATH=.:/bin:/sbin:/musl:/glibc:/usr/bin:/musl/ltp/testcases/bin:/glibc/ltp/testcases/bin",
    "LTPROOT=/musl/ltp",
    "HOME=/",
    "TERM=vt100",
];

const SDCARD_MUSL_ENV: &[&str] = &[
    "PATH=.:/bin:/sbin:/sdcard/musl:/musl:/glibc:/usr/bin:/sdcard/musl/ltp/testcases/bin:/musl/ltp/testcases/bin:/glibc/ltp/testcases/bin",
    "LTPROOT=/sdcard/musl/ltp",
    "HOME=/",
    "TERM=vt100",
];

const GLIBC_ENV: &[&str] = &[
    "PATH=.:/bin:/sbin:/glibc:/musl:/usr/bin:/glibc/ltp/testcases/bin:/musl/ltp/testcases/bin",
    "LD_LIBRARY_PATH=/lib64:/lib:/glibc/lib:/sdcard/glibc/lib",
    "LTPROOT=/glibc/ltp",
    "HOME=/",
    "TERM=vt100",
];

const SDCARD_GLIBC_ENV: &[&str] = &[
    "PATH=.:/bin:/sbin:/sdcard/glibc:/glibc:/musl:/usr/bin:/sdcard/glibc/ltp/testcases/bin:/glibc/ltp/testcases/bin:/musl/ltp/testcases/bin",
    "LD_LIBRARY_PATH=/lib64:/lib:/sdcard/glibc/lib:/glibc/lib",
    "LTPROOT=/sdcard/glibc/ltp",
    "HOME=/",
    "TERM=vt100",
];

/// 自动测试脚本白名单。只会按这里的顺序执行列出的脚本，不再扫描目录。
///
/// 例如：
/// "/musl/libctest_testcode.sh",
/// "/glibc/ltp_testcode.sh",
const TEST_SCRIPTS: &[&str] = &[
    "/musl/basic_testcode.sh",
    "/musl/busybox_testcode.sh",
    "/musl/cyclictest_testcode.sh",
    "/musl/iperf_testcode.sh",
    "/musl/iozone_testcode.sh",
    "/musl/libctest_testcode.sh",
    "/musl/libcbench_testcode.sh",
    "/musl/lua_testcode.sh",
    "/musl/lmbench_testcode.sh",
    // "/musl/netperf_testcode.sh",
    "/glibc/basic_testcode.sh",
    "/glibc/busybox_testcode.sh",
    "/glibc/cyclictest_testcode.sh",
    "/glibc/iperf_testcode.sh",
    "/glibc/iozone_testcode.sh",
    "/glibc/libcbench_testcode.sh",
    "/glibc/lua_testcode.sh",
    "/glibc/lmbench_testcode.sh",
    // "/glibc/netperf_testcode.sh",
    "/musl/ltp_testcode.sh",
    "/glibc/ltp_testcode.sh",
];
const AUTO_TEST_DISABLE_FLAG: &str = "/.initproc-no-autotest";
const SIGKILL: usize = 9;
const WNOHANG: i32 = 1;
const LTP_EXEC_FILTER_SOURCE: &str = include_str!("../../../os/src/syscall/ltp_exec_filter.rs");

/// Busybox 常用命令列表。比赛测试（lmbench/libctest 等）通常需要这些。
const BUSYBOX_CMDS: &[&str] = &[
    // 文件操作
    "ls",
    "cp",
    "mv",
    "rm",
    "cat",
    "mkdir",
    "rmdir",
    "touch",
    "ln",
    "readlink",
    "realpath",
    "chmod",
    "chown",
    "chgrp",
    "df",
    "du",
    "sync",
    // 文本处理
    "echo",
    "printf",
    "head",
    "tail",
    "grep",
    "sed",
    "awk",
    "cut",
    "sort",
    "uniq",
    "wc",
    "tr",
    "tee",
    "basename",
    "dirname",
    "seq",
    "hexdump",
    // shell / 流程控制
    "sh",
    "test",
    "[",
    "expr",
    "true",
    "false",
    "yes",
    "env",
    "exit",
    // 进程 / 系统
    "ps",
    "kill",
    "pidof",
    "pgrep",
    "pkill",
    "top",
    "uptime",
    "free",
    "mount",
    "umount",
    "dmesg",
    "insmod",
    "rmmod",
    "lsmod",
    // 网络
    "ifconfig",
    "ping",
    "wget",
    "nc",
    "netstat",
    "route",
    "traceroute",
    // 其他常用
    "sleep",
    "usleep",
    "date",
    "id",
    "whoami",
    "hostname",
    "clear",
    "reset",
    "pwd",
    "mknod",
    "mktemp",
    "stat",
    "watch",
    "xargs",
    "find",
    "which",
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

fn for_each_dir_name(path: &str, mut f: impl FnMut(&str)) -> bool {
    let fd = open(AT_FDCWD, path, OpenFlags::RDONLY, 0);
    if fd < 0 {
        return false;
    }

    let mut buf = [0u8; 4096];
    loop {
        let read_bytes = getdents64(fd as usize, &mut buf);
        if read_bytes <= 0 {
            break;
        }

        let mut offset = 0usize;
        let buf = &buf[..read_bytes as usize];
        while offset < buf.len() {
            if offset + 19 > buf.len() {
                break;
            }
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
                if !name.is_empty() && name != "." && name != ".." {
                    f(name);
                }
            }
            offset += reclen;
        }
    }

    close(fd as usize);
    true
}

fn is_ltp_whitelisted(case_name: &str) -> bool {
    let mut in_whitelist = false;
    for raw_line in LTP_EXEC_FILTER_SOURCE.lines() {
        let line = if let Some(comment_pos) = raw_line.find("//") {
            &raw_line[..comment_pos]
        } else {
            raw_line
        };

        if !in_whitelist {
            if line.contains("pub const LTP_EXEC_WHITELIST") {
                in_whitelist = true;
            }
            continue;
        }

        if line.contains("];") {
            return false;
        }

        let mut rest = line;
        while let Some(start) = rest.find('"') {
            rest = &rest[start + 1..];
            let Some(end) = rest.find('"') else {
                break;
            };
            if &rest[..end] == case_name {
                return true;
            }
            rest = &rest[end + 1..];
        }
    }
    false
}

fn create_symlink(target: &str, linkpath: &str) -> bool {
    let _ = unlinkat(AT_FDCWD, linkpath, 0);
    symlinkat(target, AT_FDCWD, linkpath) >= 0
}

fn link_filtered_entries(
    src_dir: &str,
    dst_dir: &str,
    skip: &[&str],
    ltp_bin_filter: bool,
) -> usize {
    let mut linked = 0;
    let ok = for_each_dir_name(src_dir, |name| {
        if skip.iter().any(|skip_name| *skip_name == name) {
            return;
        }
        if ltp_bin_filter && !is_ltp_whitelisted(name) {
            return;
        }

        let target = alloc::format!("{}/{}", src_dir, name);
        let linkpath = alloc::format!("{}/{}", dst_dir, name);
        if create_symlink(&target, &linkpath) {
            linked += 1;
        }
    });
    if !ok {
        return 0;
    }
    linked
}

fn setup_filtered_ltp_view(libc: &str) {
    let src_root = alloc::format!("/{}", libc);
    if !file_exists(&src_root) {
        return;
    }

    let dst_root = alloc::format!("/sdcard/{}", libc);
    let src_ltp = alloc::format!("{}/ltp", src_root);
    let dst_ltp = alloc::format!("{}/ltp", dst_root);
    let src_testcases = alloc::format!("{}/testcases", src_ltp);
    let dst_testcases = alloc::format!("{}/testcases", dst_ltp);
    let src_bin = alloc::format!("{}/bin", src_testcases);
    let dst_bin = alloc::format!("{}/bin", dst_testcases);

    let _ = mkdir("/sdcard", 0o755);
    let _ = mkdir(&dst_root, 0o755);
    let _ = mkdir(&dst_ltp, 0o755);
    let _ = mkdir(&dst_testcases, 0o755);
    let _ = mkdir(&dst_bin, 0o755);

    let root_links = link_filtered_entries(&src_root, &dst_root, &["ltp"], false);
    let ltp_links = link_filtered_entries(&src_ltp, &dst_ltp, &["testcases"], false);
    let testcases_links = link_filtered_entries(&src_testcases, &dst_testcases, &["bin"], false);
    let bin_links = link_filtered_entries(&src_bin, &dst_bin, &[], true);

    println!(
        "[initproc] filtered /sdcard/{} root={} ltp={} testcases={} ltp_bin={}",
        libc, root_links, ltp_links, testcases_links, bin_links
    );
}

fn setup_filtered_ltp_views() {
    setup_filtered_ltp_view("musl");
    setup_filtered_ltp_view("glibc");
}

fn auto_test_disabled() -> bool {
    file_exists(AUTO_TEST_DISABLE_FLAG)
}

fn env_for_script(path: &str) -> &'static [&'static str] {
    if path.starts_with("/sdcard/glibc/") {
        SDCARD_GLIBC_ENV
    } else if path.starts_with("/sdcard/musl/") {
        SDCARD_MUSL_ENV
    } else if path.starts_with("/glibc/") {
        GLIBC_ENV
    } else {
        ENV
    }
}

fn script_workdir_and_name(path: &str) -> (&str, &str) {
    if let Some(script_name) = path.strip_prefix("/sdcard/musl/") {
        ("/sdcard/musl", script_name)
    } else if let Some(script_name) = path.strip_prefix("/sdcard/glibc/") {
        ("/sdcard/glibc", script_name)
    } else if let Some(script_name) = path.strip_prefix("/musl/") {
        ("/musl", script_name)
    } else if let Some(script_name) = path.strip_prefix("/glibc/") {
        ("/glibc", script_name)
    } else {
        ("/", path.strip_prefix('/').unwrap_or(path))
    }
}

fn preferred_test_script(path: &str) -> Option<alloc::string::String> {
    let sdcard_path = if let Some(script_name) = path.strip_prefix("/musl/") {
        alloc::format!("/sdcard/musl/{}", script_name)
    } else if let Some(script_name) = path.strip_prefix("/glibc/") {
        alloc::format!("/sdcard/glibc/{}", script_name)
    } else {
        return None;
    };

    if executable_exists(&sdcard_path) {
        Some(sdcard_path)
    } else {
        None
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
        let preferred_script = preferred_test_script(script);
        let script = preferred_script.as_deref().unwrap_or(script);
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
        let (workdir, env) = if file_exists("/sdcard/musl") {
            ("/sdcard/musl", SDCARD_MUSL_ENV)
        } else {
            ("/", ENV)
        };
        if chdir(workdir) < 0 {
            println!(
                "[initproc] failed to chdir {}, keeping current directory",
                workdir
            );
        }
        execve("/bin/sh", &["sh"], env);
        execve("/musl/busybox", &["busybox", "sh"], env);
        execve("/bin/busybox", &["busybox", "sh"], env);
        println!("[initproc] failed to start shell");
        user_lib::exit(127);
    }
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("exec init_proc");

    setup_busybox_links();
    setup_filtered_ltp_views();

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
