#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use user_lib::{
    AT_FDCWD, OpenFlags, close, exit, linkat, lseek, open, read, sync, thread_create, unlinkat,
    waittid, write, yield_,
};

const PATH: &str = "/ext4_linker_sparse_write_test.bin";
const ALIAS_PATH: &str = "/ext4_linker_sparse_write_test.alias";
const WRITEBACK_RACE_PATH: &str = "/ext4_linker_writeback_race_test.bin";
const PAGE_SIZE: usize = 4096;
const HIGH_OFFSET: usize = 99 * PAGE_SIZE + 912;
const LOW_PAGE: usize = 7;
const LOW_IN_PAGE: usize = 1696;
const LOW_OFFSET: usize = LOW_PAGE * PAGE_SIZE + LOW_IN_PAGE;
const LOW_LEN: usize = 1698;
const SPARSE_HOLE_PAGE: usize = LOW_PAGE + 1;
const SEEK_SET: i32 = 0;
const WRITEBACK_RACE_CHUNK: usize = 64 * 1024;
const WRITEBACK_RACE_ROUNDS: usize = 32;

static WRITEBACK_RACE_FD: AtomicUsize = AtomicUsize::new(usize::MAX);
static WRITEBACK_RACE_START: AtomicBool = AtomicBool::new(false);
static WRITEBACK_RACE_DONE: AtomicBool = AtomicBool::new(false);
static WRITEBACK_RACE_FAILED: AtomicBool = AtomicBool::new(false);
static WRITEBACK_RACE_PAYLOAD: [u8; WRITEBACK_RACE_CHUNK] = [0x6d; WRITEBACK_RACE_CHUNK];

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

fn verify_full_sparse_hole(fd: usize) -> bool {
    if !seek(fd, SPARSE_HOLE_PAGE * PAGE_SIZE) {
        return false;
    }
    let mut page = [0xa5u8; PAGE_SIZE];
    read(fd, &mut page) == PAGE_SIZE as isize && page.iter().all(|byte| *byte == 0)
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

extern "C" fn writeback_race_writer(_: usize) -> ! {
    while !WRITEBACK_RACE_START.load(Ordering::Acquire) {
        yield_();
    }
    let fd = WRITEBACK_RACE_FD.load(Ordering::Acquire);
    for _ in 0..WRITEBACK_RACE_ROUNDS {
        if write(fd, &WRITEBACK_RACE_PAYLOAD) != WRITEBACK_RACE_CHUNK as isize {
            WRITEBACK_RACE_FAILED.store(true, Ordering::Release);
            break;
        }
    }
    WRITEBACK_RACE_DONE.store(true, Ordering::Release);
    exit(0)
}

fn join_thread(tid: isize) -> isize {
    loop {
        let result = waittid(tid as usize);
        if result != -11 {
            return result;
        }
        yield_();
    }
}

fn verify_concurrent_eof_writeback() -> bool {
    let _ = unlinkat(AT_FDCWD, WRITEBACK_RACE_PATH, 0);
    let fd = open(
        AT_FDCWD,
        WRITEBACK_RACE_PATH,
        OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::RDWR,
        0o600,
    );
    if fd < 0 {
        return false;
    }
    let fd = fd as usize;
    WRITEBACK_RACE_FD.store(fd, Ordering::Release);
    WRITEBACK_RACE_START.store(false, Ordering::Release);
    WRITEBACK_RACE_DONE.store(false, Ordering::Release);
    WRITEBACK_RACE_FAILED.store(false, Ordering::Release);

    let tid = thread_create(writeback_race_writer, 0);
    if tid < 0 {
        let _ = close(fd);
        let _ = unlinkat(AT_FDCWD, WRITEBACK_RACE_PATH, 0);
        return false;
    }
    WRITEBACK_RACE_START.store(true, Ordering::Release);
    while !WRITEBACK_RACE_DONE.load(Ordering::Acquire) {
        if sync() < 0 {
            WRITEBACK_RACE_FAILED.store(true, Ordering::Release);
            break;
        }
        yield_();
    }
    let joined = join_thread(tid);
    let write_ok = joined == 0 && !WRITEBACK_RACE_FAILED.load(Ordering::Acquire);
    let flushed = sync() == 0;
    let closed = close(fd) == 0;
    if !write_ok || !flushed || !closed {
        let _ = unlinkat(AT_FDCWD, WRITEBACK_RACE_PATH, 0);
        return false;
    }

    let fd = open(AT_FDCWD, WRITEBACK_RACE_PATH, OpenFlags::RDONLY, 0);
    if fd < 0 {
        let _ = unlinkat(AT_FDCWD, WRITEBACK_RACE_PATH, 0);
        return false;
    }
    let fd = fd as usize;
    let expected_len = WRITEBACK_RACE_CHUNK * WRITEBACK_RACE_ROUNDS;
    let mut verified = 0usize;
    let mut page = [0u8; PAGE_SIZE];
    while verified < expected_len {
        let read_len = read(fd, &mut page);
        if read_len != PAGE_SIZE as isize || page.iter().any(|byte| *byte != 0x6d) {
            break;
        }
        verified += PAGE_SIZE;
    }
    let mut eof = [0u8; 1];
    let valid = verified == expected_len && read(fd, &mut eof) == 0;
    let _ = close(fd);
    let _ = unlinkat(AT_FDCWD, WRITEBACK_RACE_PATH, 0);
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

    if !verify_low_page(fd) || !verify_full_sparse_hole(fd) || !verify_high_data(fd) {
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
    let persisted = verify_low_page(fd) && verify_full_sparse_hole(fd) && verify_high_data(fd);
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
    if !verify_concurrent_eof_writeback() {
        let _ = unlinkat(AT_FDCWD, WRITEBACK_RACE_PATH, 0);
        let _ = unlinkat(AT_FDCWD, ALIAS_PATH, 0);
        let _ = unlinkat(AT_FDCWD, PATH, 0);
        println!("[ext4_linker_sparse_write_test] FAIL: concurrent EOF writeback");
        return 9;
    }
    let _ = unlinkat(AT_FDCWD, ALIAS_PATH, 0);
    let _ = unlinkat(AT_FDCWD, PATH, 0);
    println!("[ext4_linker_sparse_write_test] PASS");
    0
}
