#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{AT_FDCWD, OpenFlags, close, mmap, munmap, open, write};

const PAGE_SIZE: usize = 4096;
const MAP_PRIVATE: usize = 0x02;
const PROT_READ: usize = 0x01;

fn fail(message: &str, code: i32) -> i32 {
    println!("[file_vma_trim_test] FAIL: {}", message);
    code
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[file_vma_trim_test] start");

    let path = "/tmp/file_vma_trim_test.bin";
    let fd = open(
        AT_FDCWD,
        path,
        OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::WRONLY,
        0,
    );
    if fd < 0 {
        return fail("create", 1);
    }

    let pages = [
        [0x11_u8; PAGE_SIZE],
        [0x22_u8; PAGE_SIZE],
        [0x33_u8; PAGE_SIZE],
    ];
    for page in &pages {
        if write(fd as usize, page) != PAGE_SIZE as isize {
            let _ = close(fd as usize);
            return fail("write", 2);
        }
    }
    let _ = close(fd as usize);

    let fd = open(AT_FDCWD, path, OpenFlags::RDONLY, 0);
    if fd < 0 {
        return fail("reopen", 3);
    }
    let mapped = mmap(0, 3 * PAGE_SIZE, PROT_READ, MAP_PRIVATE, fd, 0);
    let _ = close(fd as usize);
    if mapped < 0 {
        return fail("mmap", 4);
    }
    let base = mapped as usize;

    // Fault the remaining pages only after removing the prefix, so this
    // directly verifies the trimmed VMA's file-offset invariant.
    if munmap(base, PAGE_SIZE) != 0 {
        let _ = munmap(base + PAGE_SIZE, 2 * PAGE_SIZE);
        return fail("prefix munmap", 5);
    }
    let data_ok = unsafe {
        *((base + PAGE_SIZE) as *const u8) == 0x22 && *((base + 2 * PAGE_SIZE) as *const u8) == 0x33
    };
    let cleanup = munmap(base + PAGE_SIZE, 2 * PAGE_SIZE);
    if !data_ok {
        return fail("trimmed mapping used the wrong file offset", 6);
    }
    if cleanup != 0 {
        return fail("cleanup munmap", 7);
    }

    println!("[file_vma_trim_test] PASS");
    0
}
