#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use user_lib::{close, execve, fork, mkdir, open, symlinkat, unlinkat, wait, yield_, OpenFlags, AT_FDCWD};

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
    

    "mkfs.ext2","mkfs.vfat",
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


    let _ = unlinkat(AT_FDCWD, "/bin/mkfs.ext3", 0);
    // let _ = symlinkat("/bin/mkfs.ext2", AT_FDCWD, "/bin/mkfs.ext3");
    let _ = unlinkat(AT_FDCWD, "/bin/mkfs.ext4", 0);
    // let _ = symlinkat("/bin/mkfs.ext2", AT_FDCWD, "/bin/mkfs.ext4");

    
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("exec init_proc");

    setup_busybox_links();

    if fork() == 0 {
        let envp = [
            "PATH=/bin:/sbin:/musl:/usr/bin",
            "HOME=/",
            "TERM=vt100",
        ];
        execve("/bin/sh", &["sh"], &envp);
    } else {
        println!("this is parent");
        loop {
            let mut exit_code: i32 = 0;

            let pid = wait(&mut exit_code);
            if pid == -1 {
                yield_();
                continue;
            }
            // println!(
            //     "[initproc] Released a zombie process, pid={}, exit_code={}",
            //     pid, exit_code,
            // );
        }
    }
    0
}
