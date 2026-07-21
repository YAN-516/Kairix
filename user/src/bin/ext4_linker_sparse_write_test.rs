#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{AT_FDCWD, OpenFlags, close, linkat, lseek, open, read, sync, unlinkat, write};

const PATH: &str = "/ext4_linker_sparse_write_test.bin";
const ALIAS_PATH: &str = "/ext4_linker_sparse_write_test.alias";
const PAGE_SIZE: usize = 4096;
const HIGH_OFFSET: usize = 99 * PAGE_SIZE + 912;
const LOW_PAGE: usize = 7;
const LOW_IN_PAGE: usize = 1696;
const LOW_OFFSET: usize = LOW_PAGE * PAGE_SIZE + LOW_IN_PAGE;
const LOW_LEN: usize = 1698;
const SEEK_SET: i32 = 0;

fn seek(fd: usize, offset: usize) -> bool {
    lseek(fd, offset as isize, SEEK_SET) == offset as isize
}

fn verify_low_page(fd: usize) -> bool {
    if !seek(fd, LOW_PAGE * PAGE_SIZE) {
        return false;
    }
    let mut page = [0xa5u8; PAGE_SIZE];
    if read(fd, &mut page) != PAGE_SIZE as isize {
        return false;
    }
    page[..LOW_IN_PAGE].iter().all(|byte| *byte == 0)
        && page[LOW_IN_PAGE..LOW_IN_PAGE + LOW_LEN]
            .iter()
            .all(|byte| *byte == 0x5a)
        && page[LOW_IN_PAGE + LOW_LEN..].iter().all(|byte| *byte == 0)
}

fn verify_high_data(fd: usize) -> bool {
    if !seek(fd, HIGH_OFFSET) {
        return false;
    }
    let mut data = [0u8; 32];
    read(fd, &mut data) == data.len() as isize && data.iter().all(|byte| *byte == 0xc3)
}

fn verify_replaced_alias() -> bool {
    const OLD_PAGES: usize = 128;
    const NEW_LEN: usize = PAGE_SIZE + 1904;

    let _ = unlinkat(AT_FDCWD, ALIAS_PATH, 0);
    let old_fd = open(
        AT_FDCWD,
        PATH,
        OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::RDWR,
        0o700,
    );
    if old_fd < 0 {
        return false;
    }
    let old_fd = old_fd as usize;
    let old_page = [0x11u8; PAGE_SIZE];
    for _ in 0..OLD_PAGES {
        if write(old_fd, &old_page) != PAGE_SIZE as isize {
            let _ = close(old_fd);
            return false;
        }
    }
    // Closing queues a large dirty inode. The following O_TRUNC must prevent
    // any page already snapshotted by deferred writeback from reaching disk.
    if close(old_fd) < 0 {
        return false;
    }

    let new_fd = open(AT_FDCWD, PATH, OpenFlags::O_TRUNC | OpenFlags::RDWR, 0o700);
    if new_fd < 0 {
        return false;
    }
    let new_fd = new_fd as usize;
    let first = [0xa7u8; PAGE_SIZE];
    let second = [0x3cu8; NEW_LEN - PAGE_SIZE];
    if write(new_fd, &first) != first.len() as isize
        || write(new_fd, &second) != second.len() as isize
        || linkat(AT_FDCWD, PATH, AT_FDCWD, ALIAS_PATH, 0) != 0
        || close(new_fd) < 0
        || sync() < 0
    {
        return false;
    }

    let alias_fd = open(AT_FDCWD, ALIAS_PATH, OpenFlags::RDONLY, 0);
    if alias_fd < 0 {
        return false;
    }
    let alias_fd = alias_fd as usize;
    let mut first_read = [0u8; PAGE_SIZE];
    let mut second_read = [0u8; NEW_LEN - PAGE_SIZE];
    let mut eof = [0u8; 1];
    let valid = read(alias_fd, &mut first_read) == first_read.len() as isize
        && first_read.iter().all(|byte| *byte == 0xa7)
        && read(alias_fd, &mut second_read) == second_read.len() as isize
        && second_read.iter().all(|byte| *byte == 0x3c)
        && read(alias_fd, &mut eof) == 0;
    let _ = close(alias_fd);
    valid
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[ext4_linker_sparse_write_test] start");
    let _ = unlinkat(AT_FDCWD, PATH, 0);
    let _ = unlinkat(AT_FDCWD, ALIAS_PATH, 0);
    let fd = open(
        AT_FDCWD,
        PATH,
        OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::RDWR,
        0o600,
    );
    if fd < 0 {
        println!("[ext4_linker_sparse_write_test] FAIL: open={}", fd);
        return 1;
    }
    let fd = fd as usize;

    // Linkers commonly populate a high section first, growing the VFS inode
    // while the backing ext4 inode is still shorter, then return to partially
    // overwrite a lower page. The untouched gap must read as zero, not EIO.
    let high = [0xc3u8; 32];
    if !seek(fd, HIGH_OFFSET) || write(fd, &high) != high.len() as isize {
        println!("[ext4_linker_sparse_write_test] FAIL: high write");
        let _ = close(fd);
        let _ = unlinkat(AT_FDCWD, PATH, 0);
        return 2;
    }

    let low = [0x5au8; LOW_LEN];
    if !seek(fd, LOW_OFFSET) || write(fd, &low) != LOW_LEN as isize {
        println!("[ext4_linker_sparse_write_test] FAIL: low partial write");
        let _ = close(fd);
        let _ = unlinkat(AT_FDCWD, PATH, 0);
        return 3;
    }

    if !verify_low_page(fd) || !verify_high_data(fd) {
        println!("[ext4_linker_sparse_write_test] FAIL: cached verification");
        let _ = close(fd);
        let _ = unlinkat(AT_FDCWD, PATH, 0);
        return 4;
    }

    if sync() < 0 || close(fd) < 0 {
        println!("[ext4_linker_sparse_write_test] FAIL: sync/close");
        let _ = unlinkat(AT_FDCWD, PATH, 0);
        return 5;
    }

    let fd = open(AT_FDCWD, PATH, OpenFlags::RDONLY, 0);
    if fd < 0 {
        println!("[ext4_linker_sparse_write_test] FAIL: reopen={}", fd);
        let _ = unlinkat(AT_FDCWD, PATH, 0);
        return 6;
    }
    let fd = fd as usize;
    let persisted = verify_low_page(fd) && verify_high_data(fd);
    let _ = close(fd);
    if !persisted {
        let _ = unlinkat(AT_FDCWD, PATH, 0);
        println!("[ext4_linker_sparse_write_test] FAIL: persisted verification");
        return 7;
    }

    if !verify_replaced_alias() {
        let _ = unlinkat(AT_FDCWD, ALIAS_PATH, 0);
        let _ = unlinkat(AT_FDCWD, PATH, 0);
        println!("[ext4_linker_sparse_write_test] FAIL: truncate/writeback generation");
        return 8;
    }
    let _ = unlinkat(AT_FDCWD, ALIAS_PATH, 0);
    let _ = unlinkat(AT_FDCWD, PATH, 0);
    println!("[ext4_linker_sparse_write_test] PASS");
    0
}
