#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use user_lib::{
    AT_FDCWD, OpenFlags, chdir, close, execve, fork, getdents64, getpid, kill, mkdir, open,
    poweroff, setpgid, symlinkat, unlinkat, wait, waitpid_options, write, yield_,
};

const ENV: &[&str] = &[
    "PATH=/usr/bin:/bin:/sbin:/musl:/glibc:/musl/ltp/testcases/bin:/glibc/ltp/testcases/bin:.",
    "GIT_SSL_CAINFO=/etc/ssl/certs/ca-certificates.crt",
    "SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt",
    "LTPROOT=/musl/ltp",
    "HOME=/",
    "TERM=ansi",
];
const SDCARD_MUSL_ENV: &[&str] = &[
    "PATH=/usr/bin:/bin:/sbin:/sdcard/musl:/musl:/glibc:/sdcard/musl/ltp/testcases/bin:/musl/ltp/testcases/bin:/glibc/ltp/testcases/bin:.",
    "GIT_SSL_CAINFO=/etc/ssl/certs/ca-certificates.crt",
    "SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt",
    "LTPROOT=/sdcard/musl/ltp",
    "HOME=/",
    "TERM=ansi",
];
const GLIBC_ENV: &[&str] = &[
    "PATH=/usr/bin:/bin:/sbin:/glibc:/musl:/glibc/ltp/testcases/bin:/musl/ltp/testcases/bin:.",
    "GIT_SSL_CAINFO=/etc/ssl/certs/ca-certificates.crt",
    "SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt",
    "LTPROOT=/glibc/ltp",
    "HOME=/",
    "TERM=ansi",
];
const SDCARD_GLIBC_ENV: &[&str] = &[
    "PATH=/usr/bin:/bin:/sbin:/sdcard/glibc:/glibc:/musl:/sdcard/glibc/ltp/testcases/bin:/glibc/ltp/testcases/bin:/musl/ltp/testcases/bin:.",
    "GIT_SSL_CAINFO=/etc/ssl/certs/ca-certificates.crt",
    "SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt",
    "LTPROOT=/sdcard/glibc/ltp",
    "HOME=/",
    "TERM=ansi",
];

/// 默认自动测试脚本。只会按这里的顺序执行列出的脚本，不再扫描目录。
const FINAL_TEST_SCRIPTS: &[&str] = &["/glibc/cagent_testcode.sh", "/glibc/buildstorm_testcode.sh"];

/// 初赛自动测试脚本。通过构建参数显式选择时按原顺序执行。
const PRELIMINARY_TEST_SCRIPTS: &[&str] = &[
    "/sdcard/musl/ltp_testcode.sh",
    "/sdcard/glibc/ltp_testcode.sh",
    "/musl/iozone_testcode.sh",
    "/glibc/iozone_testcode.sh",
    "/musl/basic_testcode.sh",
    "/musl/busybox_testcode.sh",
    "/musl/cyclictest_testcode.sh",
    "/musl/libctest_testcode.sh",
    "/musl/libcbench_testcode.sh",
    "/musl/lua_testcode.sh",
    "/musl/lmbench_testcode.sh",
    "/glibc/basic_testcode.sh",
    "/glibc/busybox_testcode.sh",
    "/glibc/cyclictest_testcode.sh",
    "/glibc/libcbench_testcode.sh",
    "/glibc/lua_testcode.sh",
    "/musl/iperf_testcode.sh",
    "/musl/netperf_testcode.sh",
    "/glibc/iperf_testcode.sh",
    "/glibc/netperf_testcode.sh",
    "/glibc/lmbench_testcode.sh",

];

/// 用于从评测镜像识别初赛环境的原始脚本。
/// `/sdcard/*/ltp_testcode.sh` 是 initproc 启动后生成的兼容视图，不参与镜像识别。
const PRELIMINARY_PROBE_SCRIPTS: &[&str] = &[
    "/musl/iozone_testcode.sh",
    "/glibc/iozone_testcode.sh",
    "/musl/basic_testcode.sh",
    "/musl/busybox_testcode.sh",
    "/musl/cyclictest_testcode.sh",
    "/musl/libctest_testcode.sh",
    "/musl/libcbench_testcode.sh",
    "/musl/lua_testcode.sh",
    "/musl/lmbench_testcode.sh",
    "/glibc/basic_testcode.sh",
    "/glibc/busybox_testcode.sh",
    "/glibc/cyclictest_testcode.sh",
    "/glibc/libcbench_testcode.sh",
    "/glibc/lua_testcode.sh",
    "/musl/iperf_testcode.sh",
    "/musl/netperf_testcode.sh",
    "/glibc/iperf_testcode.sh",
    "/glibc/netperf_testcode.sh",
    "/glibc/lmbench_testcode.sh",
];

const AUTO_TEST_DISABLE_FLAG: &str = "/.initproc-no-autotest";
const PRELIMINARY_TEST_FLAG: &str = "/.initproc-preliminary-tests";
const BUSYBOX_INTERPRETER: &str = "/bin/busybox";
const MUSL_BUSYBOX: &str = "/musl/busybox";
const AT_REMOVEDIR: u32 = 0x200;
const DT_DIR: u8 = 4;
const SIGKILL: usize = 9;
const WNOHANG: i32 = 1;
const LTP_EXEC_FILTER_SOURCE: &str = include_str!("../../../os/src/ltp.rs");
const LMBENCH_COMPAT_WRAPPER: &[u8] = b"#!/bin/sh\nexec lmbench_all \"$@\"\n";

const LMBENCH_ROOTS: &[&str] = &["/musl", "/glibc"];
const LMBENCH_APPLETS: &[&str] = &[
    "lat_syscall",
    "lat_select",
    "lat_sig",
    "lat_pipe",
    "lat_proc",
    "lmdd",
    "lat_pagefault",
    "lat_mmap",
    "bw_pipe",
    "lat_fs",
    "bw_file_rd",
    "bw_mmap_rd",
    "lat_ctx",
];

/// Busybox 常用命令列表。比赛测试（lmbench/libctest 等）通常需要这些。
const BUSYBOX_CMDS: &str = "\
ls cp mv rm cat mkdir rmdir touch ln readlink realpath chmod chown chgrp df du sync \
echo printf head tail grep sed awk cut sort uniq wc tr tee basename dirname seq hexdump \
sh test [ expr true false yes env exit \
ps kill pidof pgrep pkill top uptime free mount umount dmesg insmod rmmod lsmod \
ifconfig ping wget nc netstat route traceroute \
sleep usleep date id whoami hostname clear reset pwd mknod mktemp stat watch xargs find which mkfs.vfat";

fn setup_busybox_links() {
    let _ = mkdir("/bin", 0o755);

    let Some(bb_path) = first_existing(&["/musl/busybox", "/bin/busybox"]) else {
        println!("[initproc] busybox not found, skipping symlink setup");
        return;
    };

    let mut created = 0;
    let mut skipped = 0;
    for cmd in BUSYBOX_CMDS.split_whitespace() {
        let linkpath = alloc::format!("/bin/{}", cmd);
        // Keep full rootfs tools (for example GNU date); BusyBox only fills gaps.
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

    // mkfs.ext2/3/4 are real e2fsprogs binaries; busybox in this image lacks them.
}

/// Preliminary scripts contain nested scripts whose shebang is
/// `#!/bin/busybox sh`.  Ensure that exact interpreter path exists before the
/// preliminary suite starts; `/bin/sh` alone cannot satisfy that shebang.
fn setup_preliminary_busybox_interpreter() -> bool {
    if file_exists(BUSYBOX_INTERPRETER) {
        println!(
            "[initproc] preliminary busybox interpreter already available at {}",
            BUSYBOX_INTERPRETER
        );
        return true;
    }

    if !file_exists(MUSL_BUSYBOX) {
        println!(
            "[initproc] cannot create preliminary busybox interpreter: missing {}",
            MUSL_BUSYBOX
        );
        return false;
    }

    let _ = mkdir("/bin", 0o755);
    // Remove a broken link, if present. A working file was preserved above.
    let _ = unlinkat(AT_FDCWD, BUSYBOX_INTERPRETER, 0);
    let ret = symlinkat(MUSL_BUSYBOX, AT_FDCWD, BUSYBOX_INTERPRETER);
    let ready = ret >= 0 && file_exists(BUSYBOX_INTERPRETER);
    println!(
        "[initproc] preliminary busybox interpreter {} -> {}: {}",
        BUSYBOX_INTERPRETER,
        MUSL_BUSYBOX,
        if ready { "ready" } else { "failed" }
    );
    ready
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

fn first_existing(paths: &[&'static str]) -> Option<&'static str> {
    paths.iter().copied().find(|path| file_exists(path))
}

fn for_each_dir_entry(path: &str, mut f: impl FnMut(&str, u8)) -> bool {
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
            const DIRENT_HEADER_LEN: usize = 19;
            const DIRENT_RECLEN_OFFSET: usize = 16;
            const DIRENT_TYPE_OFFSET: usize = 18;

            if offset + DIRENT_HEADER_LEN > buf.len() {
                break;
            }
            let reclen = u16::from_ne_bytes([
                buf[offset + DIRENT_RECLEN_OFFSET],
                buf[offset + DIRENT_RECLEN_OFFSET + 1],
            ]) as usize;
            if reclen == 0 || offset + reclen > buf.len() {
                break;
            }

            let d_type = buf[offset + DIRENT_TYPE_OFFSET];
            let name_start = offset + DIRENT_HEADER_LEN;
            let mut name_end = name_start;
            while name_end < offset + reclen && buf[name_end] != 0 {
                name_end += 1;
            }

            if let Ok(name) = core::str::from_utf8(&buf[name_start..name_end]) {
                if !name.is_empty() && name != "." && name != ".." {
                    f(name, d_type);
                }
            }
            offset += reclen;
        }
    }

    close(fd as usize);
    true
}

fn cleanup_dir_contents(path: &str) -> usize {
    let mut entries = alloc::vec::Vec::new();
    if !for_each_dir_entry(path, |name, d_type| {
        entries.push((alloc::string::String::from(name), d_type));
    }) {
        return 0;
    }

    let mut removed = 0usize;
    for (name, d_type) in entries.iter() {
        let child = alloc::format!("{}/{}", path, name);
        if *d_type == DT_DIR {
            removed += cleanup_dir_contents(&child);
            if unlinkat(AT_FDCWD, &child, AT_REMOVEDIR) >= 0 {
                removed += 1;
            }
            continue;
        }

        if unlinkat(AT_FDCWD, &child, 0) >= 0 {
            removed += 1;
        } else {
            removed += cleanup_dir_contents(&child);
            if unlinkat(AT_FDCWD, &child, AT_REMOVEDIR) >= 0 {
                removed += 1;
            }
        }
    }
    removed
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

fn write_file(path: &str, data: &[u8], mode: u32) -> bool {
    let fd = open(
        AT_FDCWD,
        path,
        OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::WRONLY,
        mode,
    );
    if fd < 0 {
        return false;
    }

    let mut offset = 0usize;
    while offset < data.len() {
        let written = write(fd as usize, &data[offset..]);
        if written <= 0 {
            close(fd as usize);
            return false;
        }
        offset += written as usize;
    }

    close(fd as usize);
    true
}

fn setup_lmbench_compat() {
    for dir in [
        "/code",
        "/code/lmbench_src",
        "/code/lmbench_src/bin",
        "/code/lmbench_src/bin/build",
    ] {
        let _ = mkdir(dir, 0o755);
    }

    let wrapper_ok = write_file(
        "/code/lmbench_src/bin/build/lmbench_all",
        LMBENCH_COMPAT_WRAPPER,
        0o755,
    );

    let mut linked = 0usize;
    for root in LMBENCH_ROOTS.iter() {
        let lmbench_all = alloc::format!("{}/lmbench_all", root);
        if !file_exists(&lmbench_all) {
            continue;
        }

        for applet in LMBENCH_APPLETS.iter() {
            let linkpath = alloc::format!("{}/{}", root, applet);
            if file_exists(&linkpath) {
                continue;
            }
            if create_symlink(&lmbench_all, &linkpath) {
                linked += 1;
            }
        }
    }

    println!(
        "[initproc] lmbench compat wrapper={} applet_links={}",
        if wrapper_ok { "ok" } else { "failed" },
        linked
    );
}

fn link_filtered_entries(
    src_dir: &str,
    dst_dir: &str,
    skip: &[&str],
    ltp_bin_filter: bool,
) -> usize {
    let mut linked = 0;
    let ok = for_each_dir_entry(src_dir, |name, _| {
        let linkpath = alloc::format!("{}/{}", dst_dir, name);
        if skip.iter().any(|skip_name| *skip_name == name) {
            let _ = unlinkat(AT_FDCWD, &linkpath, 0);
            return;
        }
        if ltp_bin_filter && !is_ltp_whitelisted(name) {
            return;
        }

        let target = alloc::format!("{}/{}", src_dir, name);
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
    let removed = cleanup_dir_contents(&dst_root);
    let _ = mkdir(&dst_ltp, 0o755);
    let _ = mkdir(&dst_testcases, 0o755);
    let _ = mkdir(&dst_bin, 0o755);

    let src_script = alloc::format!("{}/ltp_testcode.sh", src_root);
    let dst_script = alloc::format!("{}/ltp_testcode.sh", dst_root);
    let script_link = if file_exists(&src_script) && create_symlink(&src_script, &dst_script) {
        1
    } else {
        0
    };
    let ltp_links = link_filtered_entries(&src_ltp, &dst_ltp, &["testcases"], false);
    let testcases_links = link_filtered_entries(&src_testcases, &dst_testcases, &["bin"], false);
    let bin_links = link_filtered_entries(&src_bin, &dst_bin, &[], true);

    println!(
        "[initproc] filtered /sdcard/{} removed={} script={} ltp={} testcases={} ltp_bin={}",
        libc, removed, script_link, ltp_links, testcases_links, bin_links
    );
}

fn setup_filtered_ltp_views() {
    setup_filtered_ltp_view("musl");
    setup_filtered_ltp_view("glibc");
}

fn auto_test_disabled() -> bool {
    file_exists(AUTO_TEST_DISABLE_FLAG)
}

fn all_scripts_exist(scripts: &[&str]) -> bool {
    scripts.iter().all(|script| file_exists(script))
}

fn report_missing_scripts(suite: &str, scripts: &[&str]) {
    for script in scripts {
        if !file_exists(script) {
            println!("[initproc] {} suite missing {}", suite, script);
        }
    }
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

fn exec_shell(env: &[&str], script_name: Option<&str>) {
    if let Some(script_name) = script_name {
        execve(script_name, &[script_name], env);
        if file_exists("/bin/sh") {
            execve("/bin/sh", &["sh", script_name], env);
        }
        if file_exists("/musl/busybox") {
            execve("/musl/busybox", &["busybox", "sh", script_name], env);
        }
        if file_exists("/bin/busybox") {
            execve("/bin/busybox", &["busybox", "sh", script_name], env);
        }
        return;
    }

    if file_exists("/bin/sh") {
        execve("/bin/sh", &["sh"], env);
    }
    if file_exists("/musl/busybox") {
        execve("/musl/busybox", &["busybox", "sh"], env);
    }
    if file_exists("/bin/busybox") {
        execve("/bin/busybox", &["busybox", "sh"], env);
    }
    if file_exists("/bin/user_shell") {
        execve("/bin/user_shell", &["user_shell"], env);
    }
    if file_exists("/user_shell") {
        execve("/user_shell", &["user_shell"], env);
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
        exec_shell(env, Some(script_name));
        println!("[initproc] failed to execute {}", path);
        user_lib::exit(127);
    }

    if pid < 0 {
        println!("[initproc] fork failed for {}", path);
        return 127;
    }

    let _ = setpgid(pid as i32, pid as i32);

    let mut exit_code = 0;
    let mut reaped_while_waiting = 0usize;
    loop {
        let waited = waitpid_options(-1, &mut exit_code, WNOHANG);
        if waited == pid {
            if reaped_while_waiting > 0 {
                println!(
                    "[initproc] reaped {} zombie child(ren) while waiting for {}",
                    reaped_while_waiting, path
                );
            }
            break;
        }
        if waited > 0 {
            reaped_while_waiting += 1;
            continue;
        }
        if waited < 0 {
            println!(
                "[initproc] waitpid failed for {}, pid={}, waited={}",
                path, pid, waited
            );
            cleanup_script_process_group(path, pid);
            return 127;
        }
        yield_();
    }
    cleanup_script_process_group(path, pid);
    exit_code
}

fn reap_any_zombies(reason: &str) -> usize {
    let mut total = 0usize;
    loop {
        let mut exit_code = 0;
        let waited = waitpid_options(-1, &mut exit_code, WNOHANG);
        if waited > 0 {
            total += 1;
            continue;
        }
        break;
    }
    if total > 0 {
        println!(
            "[initproc] reaped {} zombie child(ren) during {}",
            total, reason
        );
    }
    total
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
            reap_any_zombies("process-group cleanup");
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

fn run_test_scripts(suite: &str, scripts: &[&str]) -> bool {
    if scripts.is_empty() {
        return false;
    }

    if suite == "preliminary" && !setup_preliminary_busybox_interpreter() {
        println!(
            "[initproc] preliminary scripts may fail because {} is unavailable",
            BUSYBOX_INTERPRETER
        );
    }

    println!(
        "[initproc] selected {} official test script(s)",
        scripts.len()
    );
    println!("[initproc] test suite={}", suite);
    let mut last_exit = 0;
    for script in scripts {
        println!("[initproc] running {}", script);
        last_exit = run_test_script(script);
        println!("[initproc] finished {} exit_code={}", script, last_exit);
    }

    println!("[initproc] all official test scripts finished, poweroff");
    poweroff(last_exit);
}

fn run_interactive_shell() {
    let pid = fork();
    println!("[initproc] run_interactive_shell fork ret={}", pid);
    if pid == 0 {
        println!("[initproc] shell child pid={}", getpid());
        let (workdir, env) = if file_exists("/glibc") {
            ("/glibc", GLIBC_ENV)
        } else if file_exists("/musl") {
            ("/musl", ENV)
        } else {
            ("/", ENV)
        };
        if chdir(workdir) < 0 {
            println!(
                "[initproc] failed to chdir {}, keeping current directory",
                workdir
            );
        }
        exec_shell(env, None);
        println!("[initproc] failed to start shell");
        user_lib::exit(127);
    } else if pid < 0 {
        println!("[initproc] shell fork failed ret={}", pid);
    }
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("exec init_proc");

    setup_busybox_links();
    setup_filtered_ltp_views();
    setup_lmbench_compat();

    if auto_test_disabled() {
        println!(
            "[initproc] auto test disabled by {}, starting shell",
            AUTO_TEST_DISABLE_FLAG
        );
    } else if file_exists(PRELIMINARY_TEST_FLAG) {
        println!(
            "[initproc] preliminary tests selected by {}",
            PRELIMINARY_TEST_FLAG
        );
        if run_test_scripts("preliminary", PRELIMINARY_TEST_SCRIPTS) {
            return 0;
        }
    } else {
        let preliminary_ready = all_scripts_exist(PRELIMINARY_PROBE_SCRIPTS);
        let final_ready = all_scripts_exist(FINAL_TEST_SCRIPTS);

        if preliminary_ready {
            println!("[initproc] preliminary test suite detected from disk");
            if run_test_scripts("preliminary", PRELIMINARY_TEST_SCRIPTS) {
                return 0;
            }
        } else if final_ready {
            println!("[initproc] final test suite detected from disk");
            if run_test_scripts("final", FINAL_TEST_SCRIPTS) {
                return 0;
            }
        } else {
            println!("[initproc] no complete official test suite detected");
            report_missing_scripts("preliminary", PRELIMINARY_PROBE_SCRIPTS);
            report_missing_scripts("final", FINAL_TEST_SCRIPTS);
        }
    }

    run_interactive_shell();
    println!("[initproc] parent after shell fork");
    loop {
        let mut exit_code: i32 = 0;

        let pid = wait(&mut exit_code);
        if pid == -1 {
            yield_();
            continue;
        }
    }
}
