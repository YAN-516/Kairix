#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::ptr::{read_volatile, write_volatile};
use user_lib::{AT_FDCWD, OpenFlags, close, mmap, msync, munmap, open, pread64, unlinkat, write};

const PATH: &str = "/mmap_fault_around_test.bin";
const PAGE_SIZE: usize = 4096;
const PAGES: usize = 16;
const LEN: usize = PAGE_SIZE * PAGES;
const PROT_READ: usize = 0x1;
const PROT_WRITE: usize = 0x2;
const MAP_SHARED: usize = 0x01;
const MAP_PRIVATE: usize = 0x02;
const MS_SYNC: usize = 0x04;

fn pattern(page: usize) -> u8 {
    (page as u8).wrapping_mul(29).wrapping_add(7)
}

fn create_file() -> bool {
    let _ = unlinkat(AT_FDCWD, PATH, 0);
    let fd = open(
        AT_FDCWD,
        PATH,
        OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::RDWR,
        0o600,
    );
    if fd < 0 {
        return false;
    }
    let fd = fd as usize;
    let mut page_data = [0u8; PAGE_SIZE];
    for page in 0..PAGES {
        page_data.fill(pattern(page));
        if write(fd, &page_data) != PAGE_SIZE as isize {
            let _ = close(fd);
            return false;
        }
    }
    close(fd) == 0
}

fn verify_private_cow() -> bool {
    let fd = open(AT_FDCWD, PATH, OpenFlags::RDONLY, 0);
    if fd < 0 {
        return false;
    }
    let fd = fd as usize;
    let address = mmap(0, LEN, PROT_READ | PROT_WRITE, MAP_PRIVATE, fd as isize, 0);
    if address < 0 {
        let _ = close(fd);
        return false;
    }
    let base = address as *mut u8;

    // The first read loads the ext4 window. Pages checked below should be
    // installed by fault-around while retaining their read-only COW PTEs.
    let mut contents_ok = unsafe { read_volatile(base) == pattern(0) };
    for page in 1..PAGES {
        let first = unsafe { read_volatile(base.add(page * PAGE_SIZE)) };
        let last = unsafe { read_volatile(base.add((page + 1) * PAGE_SIZE - 1)) };
        contents_ok &= first == pattern(page) && last == pattern(page);
    }

    const COW_PAGE: usize = 7;
    unsafe {
        write_volatile(base.add(COW_PAGE * PAGE_SIZE), 0xa5);
        write_volatile(base.add((COW_PAGE + 1) * PAGE_SIZE - 1), 0x5a);
    }
    let private_ok = unsafe {
        read_volatile(base.add(COW_PAGE * PAGE_SIZE)) == 0xa5
            && read_volatile(base.add((COW_PAGE + 1) * PAGE_SIZE - 1)) == 0x5a
    };
    let cleanup_ok = munmap(address as usize, LEN) == 0 && close(fd) == 0;

    let verify_fd = open(AT_FDCWD, PATH, OpenFlags::RDONLY, 0);
    if verify_fd < 0 {
        return false;
    }
    let mut disk_page = [0u8; PAGE_SIZE];
    let read_ok =
        pread64(verify_fd as usize, &mut disk_page, COW_PAGE * PAGE_SIZE) == PAGE_SIZE as isize;
    let file_unchanged = disk_page.iter().all(|byte| *byte == pattern(COW_PAGE));
    contents_ok
        && private_ok
        && cleanup_ok
        && read_ok
        && file_unchanged
        && close(verify_fd as usize) == 0
}

fn verify_shared_dirty_tracking() -> bool {
    let fd = open(AT_FDCWD, PATH, OpenFlags::RDWR, 0);
    if fd < 0 {
        return false;
    }
    let fd = fd as usize;
    let address = mmap(0, LEN, PROT_READ | PROT_WRITE, MAP_SHARED, fd as isize, 0);
    if address < 0 {
        let _ = close(fd);
        return false;
    }
    let base = address as *mut u8;
    let demand_ok = unsafe { read_volatile(base) == pattern(0) };

    // This page is inside the first fault-around window. Its speculative PTE
    // must remain read-only so the first store reaches shared dirty tracking.
    const SHARED_PAGE: usize = 11;
    unsafe {
        write_volatile(base.add(SHARED_PAGE * PAGE_SIZE), 0x3c);
        write_volatile(base.add((SHARED_PAGE + 1) * PAGE_SIZE - 1), 0xc3);
    }
    let sync_ok = msync(address as usize, LEN, MS_SYNC) == 0;
    let cleanup_ok = munmap(address as usize, LEN) == 0 && close(fd) == 0;

    let verify_fd = open(AT_FDCWD, PATH, OpenFlags::RDONLY, 0);
    if verify_fd < 0 {
        return false;
    }
    let mut disk_page = [0u8; PAGE_SIZE];
    let read_ok =
        pread64(verify_fd as usize, &mut disk_page, SHARED_PAGE * PAGE_SIZE) == PAGE_SIZE as isize;
    let persisted = disk_page[0] == 0x3c
        && disk_page[PAGE_SIZE - 1] == 0xc3
        && disk_page[1..PAGE_SIZE - 1]
            .iter()
            .all(|byte| *byte == pattern(SHARED_PAGE));
    demand_ok && sync_ok && cleanup_ok && read_ok && persisted && close(verify_fd as usize) == 0
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[mmap_fault_around_test] start pages={}", PAGES);
    if !create_file() {
        println!("[mmap_fault_around_test] FAIL: setup");
        return 1;
    }

    let private_ok = verify_private_cow();
    let shared_ok = verify_shared_dirty_tracking();
    let unlink_ok = unlinkat(AT_FDCWD, PATH, 0) == 0;
    println!(
        "[mmap_fault_around_test] private_cow={} shared_dirty={} unlink={}",
        private_ok, shared_ok, unlink_ok
    );
    if private_ok && shared_ok && unlink_ok {
        println!("[mmap_fault_around_test] PASS");
        0
    } else {
        println!("[mmap_fault_around_test] FAIL");
        2
    }
}
