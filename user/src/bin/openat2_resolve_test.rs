#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{OpenFlags, OpenHow, close, mkdir, open, openat2, symlinkat, unlinkat};

const AT_FDCWD: isize = -100;
const RESOLVE_NO_XDEV: u64 = 0x01;
const RESOLVE_NO_MAGICLINKS: u64 = 0x02;
const RESOLVE_NO_SYMLINKS: u64 = 0x04;
const RESOLVE_BENEATH: u64 = 0x08;
const RESOLVE_IN_ROOT: u64 = 0x10;

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[openat2_resolve_test] start");
    let root = "/tmp/openat2-resolve-test";
    let _ = mkdir(root, 0o755);
    let dirfd = open(AT_FDCWD, root, OpenFlags::O_DIRECTORY, 0);
    if dirfd < 0 {
        println!("[openat2_resolve_test] FAIL dirfd={}", dirfd);
        return 1;
    }
    let child = open(dirfd, "child", OpenFlags::O_CREAT | OpenFlags::RDWR, 0o644);
    if child < 0 {
        println!("[openat2_resolve_test] FAIL create={}", child);
        let _ = close(dirfd as usize);
        return 2;
    }
    let _ = close(child as usize);
    let _ = unlinkat(dirfd, "link", 0);
    if symlinkat("child", dirfd, "link") != 0 {
        println!("[openat2_resolve_test] FAIL symlink");
        let _ = close(dirfd as usize);
        return 3;
    }

    let in_root = OpenHow {
        flags: 0,
        mode: 0,
        resolve: RESOLVE_IN_ROOT,
    };
    let rooted = openat2(dirfd, "/child", &in_root);
    if rooted < 0 {
        println!("[openat2_resolve_test] FAIL IN_ROOT={}", rooted);
        let _ = close(dirfd as usize);
        return 4;
    }
    let _ = close(rooted as usize);

    let beneath = OpenHow {
        flags: 0,
        mode: 0,
        resolve: RESOLVE_BENEATH,
    };
    if openat2(dirfd, "../etc/passwd", &beneath) != -18 {
        println!("[openat2_resolve_test] FAIL BENEATH escape");
        let _ = close(dirfd as usize);
        return 5;
    }

    let no_symlinks = OpenHow {
        flags: 0,
        mode: 0,
        resolve: RESOLVE_NO_SYMLINKS,
    };
    if openat2(dirfd, "link", &no_symlinks) != -40 {
        println!("[openat2_resolve_test] FAIL NO_SYMLINKS");
        let _ = close(dirfd as usize);
        return 6;
    }

    let no_magiclinks = OpenHow {
        flags: 0,
        mode: 0,
        resolve: RESOLVE_NO_MAGICLINKS,
    };
    if openat2(AT_FDCWD, "/proc/self/exe", &no_magiclinks) != -40 {
        println!("[openat2_resolve_test] FAIL NO_MAGICLINKS");
        let _ = close(dirfd as usize);
        return 7;
    }

    let no_xdev = OpenHow {
        flags: 0,
        mode: 0,
        resolve: RESOLVE_NO_XDEV,
    };
    if openat2(AT_FDCWD, "/proc/self/exe", &no_xdev) != -18 {
        println!("[openat2_resolve_test] FAIL NO_XDEV");
        let _ = close(dirfd as usize);
        return 8;
    }

    let _ = unlinkat(dirfd, "link", 0);
    let _ = unlinkat(dirfd, "child", 0);
    let _ = close(dirfd as usize);
    println!("[openat2_resolve_test] PASS");
    0
}
