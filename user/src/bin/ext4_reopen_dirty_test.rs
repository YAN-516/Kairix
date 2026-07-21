#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{AT_FDCWD, OpenFlags, close, lseek, open, read, unlinkat, write};

const PATH: &str = "/ext4_reopen_dirty_test.bin";
const DATA_LEN: usize = 8192;
const SEEK_SET: i32 = 0;
const SEEK_END: i32 = 2;

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    let _ = unlinkat(AT_FDCWD, PATH, 0);
    let writer = open(
        AT_FDCWD,
        PATH,
        OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::RDWR,
        0o600,
    );
    if writer < 0 {
        println!("[ext4_reopen_dirty_test] FAIL: writer open={}", writer);
        return 1;
    }
    let writer = writer as usize;

    let payload = [0x5au8; DATA_LEN];
    let written = write(writer, &payload);
    if written != DATA_LEN as isize {
        println!(
            "[ext4_reopen_dirty_test] FAIL: write={} expected={}",
            written, DATA_LEN
        );
        let _ = close(writer);
        let _ = unlinkat(AT_FDCWD, PATH, 0);
        return 2;
    }

    // Keep the writer open so its dirty pages cannot have been queued by
    // close. Reopening must preserve the shared in-memory inode size even
    // though lwext4's on-disk descriptor can still report the old length.
    let reader = open(AT_FDCWD, PATH, OpenFlags::RDONLY, 0);
    if reader < 0 {
        println!("[ext4_reopen_dirty_test] FAIL: reader open={}", reader);
        let _ = close(writer);
        let _ = unlinkat(AT_FDCWD, PATH, 0);
        return 3;
    }
    let reader = reader as usize;
    let end = lseek(reader, 0, SEEK_END);
    let rewind = lseek(reader, 0, SEEK_SET);
    let mut actual = [0u8; DATA_LEN];
    let read_len = read(reader, &mut actual);
    let data_ok = actual == payload;

    let _ = close(reader);
    let _ = close(writer);
    let _ = unlinkat(AT_FDCWD, PATH, 0);

    if end == DATA_LEN as isize && rewind == 0 && read_len == DATA_LEN as isize && data_ok {
        println!("[ext4_reopen_dirty_test] PASS");
        0
    } else {
        println!(
            "[ext4_reopen_dirty_test] FAIL: end={} rewind={} read={} data_ok={}",
            end, rewind, read_len, data_ok
        );
        4
    }
}
