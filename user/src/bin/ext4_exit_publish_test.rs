#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    AT_FDCWD, OpenFlags, close, exit, fork, ftruncate, mmap, open, read, sync, unlinkat, waitpid,
    write,
};

const WRITE_PATH: &str = "/ext4_exit_publish_write.bin";
const MMAP_PATH: &str = "/ext4_exit_publish_mmap.bin";
const PAGE_SIZE: usize = 4096;
const PAGES: usize = 32;
const FILE_SIZE: usize = PAGE_SIZE * PAGES;
const ROUNDS: usize = 16;
const PROT_READ: usize = 1;
const PROT_WRITE: usize = 2;
const MAP_SHARED: usize = 1;

fn expected_byte(round: usize, page: usize, index: usize, mmap_mode: bool) -> u8 {
    (round as u8)
        .wrapping_mul(29)
        .wrapping_add((page as u8).wrapping_mul(17))
        .wrapping_add((index as u8).wrapping_mul(13))
        .wrapping_add(if mmap_mode { 0x61 } else { 0x23 })
}

fn child_write(path: &str, round: usize, mmap_mode: bool) -> ! {
    let fd = open(
        AT_FDCWD,
        path,
        OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::RDWR,
        0o600,
    );
    if fd < 0 {
        exit(101);
    }
    let fd = fd as usize;

    if mmap_mode {
        if ftruncate(fd, FILE_SIZE) != 0 {
            exit(102);
        }
        let address = mmap(
            0,
            FILE_SIZE,
            PROT_READ | PROT_WRITE,
            MAP_SHARED,
            fd as isize,
            0,
        );
        if address < 0 || close(fd) != 0 {
            exit(103);
        }
        let mapped = unsafe { core::slice::from_raw_parts_mut(address as *mut u8, FILE_SIZE) };
        for page in 0..PAGES {
            for index in 0..PAGE_SIZE {
                mapped[page * PAGE_SIZE + index] = expected_byte(round, page, index, true);
            }
        }
        // Deliberately leave the mapping live. Process exit must publish all
        // shared-page dirtiness before waitpid lets the parent consume it.
        exit(0);
    }

    let mut page_data = [0u8; PAGE_SIZE];
    for page in 0..PAGES {
        for (index, byte) in page_data.iter_mut().enumerate() {
            *byte = expected_byte(round, page, index, false);
        }
        let mut done = 0usize;
        while done < PAGE_SIZE {
            let written = write(fd, &page_data[done..]);
            if written <= 0 {
                exit(104);
            }
            done += written as usize;
        }
    }
    // Deliberately do not close the descriptor. The exit path owns publication.
    exit(0);
}

fn verify(path: &str, round: usize, mmap_mode: bool) -> bool {
    let fd = open(AT_FDCWD, path, OpenFlags::RDONLY, 0);
    if fd < 0 {
        return false;
    }
    let fd = fd as usize;
    let mut page_data = [0u8; PAGE_SIZE];
    let mut valid = true;
    for page in 0..PAGES {
        let mut done = 0usize;
        while done < PAGE_SIZE {
            let count = read(fd, &mut page_data[done..]);
            if count <= 0 {
                valid = false;
                break;
            }
            done += count as usize;
        }
        if !valid
            || page_data
                .iter()
                .enumerate()
                .any(|(index, byte)| *byte != expected_byte(round, page, index, mmap_mode))
        {
            valid = false;
            break;
        }
    }
    valid &= close(fd) == 0;
    valid
}

fn run_case(path: &str, round: usize, mmap_mode: bool) -> bool {
    let _ = unlinkat(AT_FDCWD, path, 0);
    let child = fork();
    if child == 0 {
        child_write(path, round, mmap_mode);
    }
    if child < 0 {
        return false;
    }

    let mut status = -1;
    let waited = waitpid(child as usize, &mut status);
    let immediate_ok = waited == child && status == 0 && verify(path, round, mmap_mode);
    let persisted_ok = sync() == 0 && verify(path, round, mmap_mode);
    let _ = unlinkat(AT_FDCWD, path, 0);
    if !immediate_ok || !persisted_ok {
        println!(
            "[ext4_exit_publish_test] fail round={} mode={} child={} waited={} status={:#x} immediate={} persisted={}",
            round,
            if mmap_mode { "mmap" } else { "write" },
            child,
            waited,
            status,
            immediate_ok,
            persisted_ok,
        );
    }
    immediate_ok && persisted_ok
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[ext4_exit_publish_test] start rounds={}", ROUNDS);
    for round in 0..ROUNDS {
        if !run_case(WRITE_PATH, round, false) || !run_case(MMAP_PATH, round, true) {
            return 1;
        }
        if (round + 1) % 4 == 0 {
            println!("[ext4_exit_publish_test] progress={}/{}", round + 1, ROUNDS);
        }
    }
    println!("[ext4_exit_publish_test] PASS rounds={}", ROUNDS);
    0
}
