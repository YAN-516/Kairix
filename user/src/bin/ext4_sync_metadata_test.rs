#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

extern crate alloc;

use alloc::format;
use user_lib::{AT_FDCWD, OpenFlags, close, mkdir, open, read, renameat, sync, unlinkat, write};

const SOURCE_DIR: &str = "/ext4_sync_metadata_source";
const TARGET_DIR: &str = "/ext4_sync_metadata_target";
const FILE_COUNT: usize = 64;
const AT_REMOVEDIR: u32 = 0x200;

fn cleanup() {
    for index in 0..FILE_COUNT {
        let source = format!("{}/file_{:02}", SOURCE_DIR, index);
        let target = format!("{}/file_{:02}", TARGET_DIR, index);
        let _ = unlinkat(AT_FDCWD, &source, 0);
        let _ = unlinkat(AT_FDCWD, &target, 0);
    }
    let _ = unlinkat(AT_FDCWD, SOURCE_DIR, AT_REMOVEDIR);
    let _ = unlinkat(AT_FDCWD, TARGET_DIR, AT_REMOVEDIR);
}

fn create_and_rename_files() -> bool {
    for index in 0..FILE_COUNT {
        let source = format!("{}/file_{:02}", SOURCE_DIR, index);
        let target = format!("{}/file_{:02}", TARGET_DIR, index);
        let fd = open(
            AT_FDCWD,
            &source,
            OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::WRONLY,
            0o600,
        );
        if fd < 0 {
            return false;
        }
        let expected = [index as u8; 512];
        if write(fd as usize, &expected) != expected.len() as isize
            || close(fd as usize) != 0
            || renameat(AT_FDCWD, &source, AT_FDCWD, &target) != 0
        {
            return false;
        }
    }
    true
}

fn verify_files() -> bool {
    for index in 0..FILE_COUNT {
        let target = format!("{}/file_{:02}", TARGET_DIR, index);
        let fd = open(AT_FDCWD, &target, OpenFlags::RDONLY, 0);
        if fd < 0 {
            return false;
        }
        let mut actual = [0u8; 512];
        let count = read(fd as usize, &mut actual);
        let close_result = close(fd as usize);
        if count != actual.len() as isize
            || close_result != 0
            || actual.iter().any(|byte| *byte != index as u8)
        {
            return false;
        }
    }
    true
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    cleanup();
    let setup_ok = mkdir(SOURCE_DIR, 0o700) == 0 && mkdir(TARGET_DIR, 0o700) == 0;
    let mutation_ok = setup_ok && create_and_rename_files();
    // close() may defer physical write-back, but a reopen must still observe
    // the shared page-cache contents before an explicit sync.
    let coherent_before_sync = mutation_ok && verify_files();
    let sync_result = if coherent_before_sync { sync() } else { -1 };
    let verify_after_sync = sync_result == 0 && verify_files();
    cleanup();
    let final_sync = sync();

    if verify_after_sync && final_sync == 0 {
        println!("[ext4_sync_metadata_test] PASS");
        0
    } else {
        println!(
            "[ext4_sync_metadata_test] FAIL: setup={} mutation={} coherent_before_sync={} sync={} verify_after_sync={} final_sync={}",
            setup_ok, mutation_ok, coherent_before_sync, sync_result, verify_after_sync, final_sync
        );
        1
    }
}
