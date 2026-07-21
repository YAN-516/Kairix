#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{AT_FDCWD, OpenFlags, close, linkat, open, renameat, unlinkat};

const SOURCE: &str = "/ext4_negative_dentry_source";
const RENAMED: &str = "/ext4_negative_dentry_renamed";
const LINKED: &str = "/ext4_negative_dentry_linked";
const ENOENT: isize = -2;

fn expect_missing(path: &str) -> bool {
    open(AT_FDCWD, path, OpenFlags::RDONLY, 0) == ENOENT
}

fn expect_open(path: &str) -> bool {
    let fd = open(AT_FDCWD, path, OpenFlags::RDONLY, 0);
    fd >= 0 && close(fd as usize) == 0
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    let _ = unlinkat(AT_FDCWD, SOURCE, 0);
    let _ = unlinkat(AT_FDCWD, RENAMED, 0);
    let _ = unlinkat(AT_FDCWD, LINKED, 0);

    // Populate and hit the negative cache repeatedly.
    for attempt in 0..4 {
        if !expect_missing(SOURCE) {
            println!(
                "[ext4_negative_dentry_test] FAIL: missing lookup attempt={}",
                attempt
            );
            return 1;
        }
    }

    // Creating a name that was negatively cached must invalidate the cache.
    let created = open(
        AT_FDCWD,
        SOURCE,
        OpenFlags::O_CREAT | OpenFlags::RDWR,
        0o600,
    );
    if created < 0 || close(created as usize) != 0 || !expect_open(SOURCE) {
        println!(
            "[ext4_negative_dentry_test] FAIL: create visibility fd={}",
            created
        );
        return 2;
    }

    // A rename target can already have a negative cache entry.
    if !expect_missing(RENAMED)
        || renameat(AT_FDCWD, SOURCE, AT_FDCWD, RENAMED) != 0
        || !expect_open(RENAMED)
        || !expect_missing(SOURCE)
    {
        println!("[ext4_negative_dentry_test] FAIL: rename visibility");
        return 3;
    }

    // The same invariant applies to hard-link creation.
    if !expect_missing(LINKED)
        || linkat(AT_FDCWD, RENAMED, AT_FDCWD, LINKED, 0) != 0
        || !expect_open(LINKED)
    {
        println!("[ext4_negative_dentry_test] FAIL: link visibility");
        return 4;
    }

    let unlink_linked = unlinkat(AT_FDCWD, LINKED, 0);
    let unlink_renamed = unlinkat(AT_FDCWD, RENAMED, 0);
    if unlink_linked != 0
        || unlink_renamed != 0
        || !expect_missing(LINKED)
        || !expect_missing(RENAMED)
    {
        println!(
            "[ext4_negative_dentry_test] FAIL: unlink visibility linked={} renamed={}",
            unlink_linked, unlink_renamed
        );
        return 5;
    }

    println!("[ext4_negative_dentry_test] PASS");
    0
}
