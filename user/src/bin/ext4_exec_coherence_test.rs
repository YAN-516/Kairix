#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    AT_FDCWD, OpenFlags, close, mmap, msync, munmap, open, read, renameat, unlinkat, write,
};

const MMAP_PATH: &str = "/ext4_shared_mmap_test.bin";
const SOURCE_PATH: &str = "/ext4_rename_source.bin";
const TARGET_PATH: &str = "/ext4_rename_target.bin";
const PAGE_SIZE: usize = 4096;
const DATA_LEN: usize = PAGE_SIZE * 2;
const PROT_READ: usize = 1;
const PROT_WRITE: usize = 2;
const MAP_SHARED: usize = 1;
const MS_SYNC: usize = 4;

fn write_all(fd: usize, data: &[u8]) -> bool {
    let mut done = 0usize;
    while done < data.len() {
        let written = write(fd, &data[done..]);
        if written <= 0 {
            return false;
        }
        done += written as usize;
    }
    true
}

fn read_all(fd: usize, data: &mut [u8]) -> bool {
    let mut done = 0usize;
    while done < data.len() {
        let count = read(fd, &mut data[done..]);
        if count <= 0 {
            return false;
        }
        done += count as usize;
    }
    true
}

fn shared_mmap_roundtrip() -> bool {
    let _ = unlinkat(AT_FDCWD, MMAP_PATH, 0);
    let fd = open(
        AT_FDCWD,
        MMAP_PATH,
        OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::RDWR,
        0o700,
    );
    if fd < 0 {
        return false;
    }
    let fd = fd as usize;
    let initial = [0u8; DATA_LEN];
    if !write_all(fd, &initial) {
        let _ = close(fd);
        return false;
    }
    let address = mmap(
        0,
        DATA_LEN,
        PROT_READ | PROT_WRITE,
        MAP_SHARED,
        fd as isize,
        0,
    );
    if address < 0 {
        let _ = close(fd);
        return false;
    }
    let mapped = unsafe { core::slice::from_raw_parts_mut(address as *mut u8, DATA_LEN) };
    for (index, byte) in mapped.iter_mut().enumerate() {
        *byte = (index as u8).wrapping_mul(37).wrapping_add(11);
    }
    let sync_result = msync(address as usize, DATA_LEN, MS_SYNC);
    let unmap_result = munmap(address as usize, DATA_LEN);
    let close_result = close(fd);
    if sync_result != 0 || unmap_result != 0 || close_result != 0 {
        return false;
    }

    let reader = open(AT_FDCWD, MMAP_PATH, OpenFlags::RDONLY, 0);
    if reader < 0 {
        return false;
    }
    let mut actual = [0u8; DATA_LEN];
    let read_ok = read_all(reader as usize, &mut actual);
    let _ = close(reader as usize);
    let _ = unlinkat(AT_FDCWD, MMAP_PATH, 0);
    read_ok
        && actual
            .iter()
            .enumerate()
            .all(|(index, byte)| *byte == (index as u8).wrapping_mul(37).wrapping_add(11))
}

fn rename_replace_roundtrip() -> bool {
    let _ = unlinkat(AT_FDCWD, SOURCE_PATH, 0);
    let _ = unlinkat(AT_FDCWD, TARGET_PATH, 0);
    for round in 0..16usize {
        let target = open(
            AT_FDCWD,
            TARGET_PATH,
            OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::RDWR,
            0o700,
        );
        if target < 0 {
            return false;
        }
        let stale = [0x22u8; PAGE_SIZE];
        if !write_all(target as usize, &stale) || close(target as usize) != 0 {
            return false;
        }

        let source = open(
            AT_FDCWD,
            SOURCE_PATH,
            OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::RDWR,
            0o700,
        );
        if source < 0 {
            return false;
        }
        let expected = [(round as u8).wrapping_mul(13).wrapping_add(0x51); PAGE_SIZE];
        if !write_all(source as usize, &expected) || close(source as usize) != 0 {
            return false;
        }
        if renameat(AT_FDCWD, SOURCE_PATH, AT_FDCWD, TARGET_PATH) != 0 {
            return false;
        }

        let reader = open(AT_FDCWD, TARGET_PATH, OpenFlags::RDONLY, 0);
        if reader < 0 {
            return false;
        }
        let mut actual = [0u8; PAGE_SIZE];
        let ok = read_all(reader as usize, &mut actual) && actual == expected;
        let _ = close(reader as usize);
        if !ok {
            return false;
        }
    }
    let _ = unlinkat(AT_FDCWD, SOURCE_PATH, 0);
    let _ = unlinkat(AT_FDCWD, TARGET_PATH, 0);
    true
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    let mmap_ok = shared_mmap_roundtrip();
    let rename_ok = rename_replace_roundtrip();
    if mmap_ok && rename_ok {
        println!("[ext4_exec_coherence_test] PASS");
        0
    } else {
        println!(
            "[ext4_exec_coherence_test] FAIL: mmap_ok={} rename_ok={}",
            mmap_ok, rename_ok
        );
        1
    }
}
