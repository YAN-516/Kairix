#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{mmap, mremap, munmap};

const PAGE_SIZE: usize = 4096;
const PROT_READ: usize = 0x1;
const PROT_WRITE: usize = 0x2;
const MAP_PRIVATE: usize = 0x02;
const MAP_ANONYMOUS: usize = 0x20;
const MREMAP_MAYMOVE: usize = 0x1;
const MREMAP_FIXED: usize = 0x2;
const MREMAP_DONTUNMAP: usize = 0x4;

fn map_anon(len: usize) -> Result<usize, isize> {
    let ret = mmap(
        0,
        len,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0,
    );
    if ret < 0 { Err(ret) } else { Ok(ret as usize) }
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[mremap_test] start");

    let source = match map_anon(2 * PAGE_SIZE) {
        Ok(addr) => addr,
        Err(ret) => {
            println!("[mremap_test] FAIL: source mmap ret={}", ret);
            return 1;
        }
    };
    let destination = match map_anon(3 * PAGE_SIZE) {
        Ok(addr) => addr,
        Err(ret) => {
            let _ = munmap(source, 2 * PAGE_SIZE);
            println!("[mremap_test] FAIL: destination mmap ret={}", ret);
            return 2;
        }
    };

    unsafe {
        *(source as *mut u8) = 0x31;
        *((source + PAGE_SIZE) as *mut u8) = 0x72;
        *(destination as *mut u8) = 0xff;
    }

    let invalid = mremap(
        source,
        2 * PAGE_SIZE,
        3 * PAGE_SIZE,
        MREMAP_FIXED,
        destination,
    );
    if invalid != -22 {
        let _ = munmap(source, 2 * PAGE_SIZE);
        let _ = munmap(destination, 3 * PAGE_SIZE);
        println!("[mremap_test] FAIL: FIXED without MAYMOVE ret={}", invalid);
        return 3;
    }

    let moved = mremap(
        source,
        2 * PAGE_SIZE,
        3 * PAGE_SIZE,
        MREMAP_MAYMOVE | MREMAP_FIXED,
        destination,
    );
    if moved != destination as isize {
        let _ = munmap(source, 2 * PAGE_SIZE);
        let _ = munmap(destination, 3 * PAGE_SIZE);
        println!("[mremap_test] FAIL: fixed move ret={:#x}", moved);
        return 4;
    }
    let moved_data_ok = unsafe {
        *(destination as *const u8) == 0x31
            && *((destination + PAGE_SIZE) as *const u8) == 0x72
            && *((destination + 2 * PAGE_SIZE) as *const u8) == 0
    };
    if !moved_data_ok {
        let _ = munmap(destination, 3 * PAGE_SIZE);
        println!("[mremap_test] FAIL: moved data mismatch");
        return 5;
    }

    let shrunk = mremap(destination, 3 * PAGE_SIZE, PAGE_SIZE, 0, 0);
    if shrunk != destination as isize || unsafe { *(destination as *const u8) } != 0x31 {
        let _ = munmap(destination, 3 * PAGE_SIZE);
        println!("[mremap_test] FAIL: shrink ret={:#x}", shrunk);
        return 6;
    }
    if munmap(destination, PAGE_SIZE) != 0 {
        println!("[mremap_test] FAIL: shrink cleanup");
        return 7;
    }

    let dontunmap_source = match map_anon(PAGE_SIZE) {
        Ok(addr) => addr,
        Err(ret) => {
            println!("[mremap_test] FAIL: DONTUNMAP mmap ret={}", ret);
            return 8;
        }
    };
    unsafe {
        *(dontunmap_source as *mut u8) = 0x5a;
    }
    let dontunmap_target = mremap(
        dontunmap_source,
        PAGE_SIZE,
        PAGE_SIZE,
        MREMAP_MAYMOVE | MREMAP_DONTUNMAP,
        0,
    );
    if dontunmap_target < 0 {
        let _ = munmap(dontunmap_source, PAGE_SIZE);
        println!("[mremap_test] FAIL: DONTUNMAP ret={}", dontunmap_target);
        return 9;
    }
    let dontunmap_target = dontunmap_target as usize;
    let dontunmap_ok = unsafe {
        *(dontunmap_target as *const u8) == 0x5a && *(dontunmap_source as *const u8) == 0
    };
    let target_unmap = munmap(dontunmap_target, PAGE_SIZE);
    let source_unmap = munmap(dontunmap_source, PAGE_SIZE);
    if !dontunmap_ok || target_unmap != 0 || source_unmap != 0 {
        println!(
            "[mremap_test] FAIL: DONTUNMAP data={} cleanup=({}, {})",
            dontunmap_ok, target_unmap, source_unmap
        );
        return 10;
    }

    println!("[mremap_test] PASS");
    0
}
