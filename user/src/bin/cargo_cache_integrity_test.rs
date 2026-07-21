#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

extern crate alloc;

use alloc::vec;
use user_lib::{AT_FDCWD, OpenFlags, close, exit, fork, fstat, open, read, waitpid};

struct CacheExpectation {
    name: &'static str,
    path: &'static str,
    size: usize,
    hash: u32,
}

const CACHE_FILES: [CacheExpectation; 3] = [
    CacheExpectation {
        name: "anyhow",
        path: "/root/.cargo/registry/index/index.crates.io-1949cf8c6b5b557f/.cache/an/yh/anyhow",
        size: 79_966,
        hash: 1_783_174_245,
    },
    CacheExpectation {
        name: "inferno",
        path: "/root/.cargo/registry/index/index.crates.io-1949cf8c6b5b557f/.cache/in/fe/inferno",
        size: 182_525,
        hash: 2_059_488_086,
    },
    CacheExpectation {
        name: "qemu-plugin",
        path: "/root/.cargo/registry/index/index.crates.io-1949cf8c6b5b557f/.cache/qe/mu/qemu-plugin",
        size: 11_600,
        hash: 90_265_093,
    },
];

fn hash_bytes(bytes: &[u8]) -> u32 {
    let mut hash = 5381u32;
    for byte in bytes {
        hash = hash.wrapping_mul(33).wrapping_add(*byte as u32);
    }
    hash
}

fn metadata_size(fd: usize) -> Option<usize> {
    let mut stat = [0u8; 128];
    if fstat(fd, &mut stat) != 0 {
        return None;
    }
    let size = i64::from_ne_bytes(stat[48..56].try_into().ok()?);
    usize::try_from(size).ok()
}

fn check_cache_file_bulk(expected: &CacheExpectation) -> bool {
    let fd = open(
        AT_FDCWD,
        expected.path,
        OpenFlags::RDONLY | OpenFlags::O_CLOEXEC,
        0,
    );
    if fd < 0 {
        println!(
            "[cargo_cache_integrity_test] {} bulk open failed: ret={} path={}",
            expected.name, fd, expected.path
        );
        return false;
    }

    let fd = fd as usize;
    let stat_size = metadata_size(fd);
    let mut data = vec![0u8; expected.size + 1024];
    let count = read(fd, &mut data);
    let eof = if count >= 0 {
        let mut probe = [0u8; 32];
        read(fd, &mut probe)
    } else {
        -1
    };
    let close_result = close(fd);
    let actual_hash = if count > 0 {
        hash_bytes(&data[..count as usize])
    } else {
        5381
    };
    let matches = stat_size == Some(expected.size)
        && count == expected.size as isize
        && eof == 0
        && close_result == 0
        && actual_hash == expected.hash;
    println!(
        "[cargo_cache_integrity_test] {} bulk stat_size={:?} read={} eof={} hash={:#010x} expected_size={} expected_hash={:#010x} close={} result={}",
        expected.name,
        stat_size,
        count,
        eof,
        actual_hash,
        expected.size,
        expected.hash,
        close_result,
        if matches { "PASS" } else { "FAIL" }
    );
    matches
}

fn check_cache_file_stream(expected: &CacheExpectation) -> bool {
    let fd = open(AT_FDCWD, expected.path, OpenFlags::RDONLY, 0);
    if fd < 0 {
        println!(
            "[cargo_cache_integrity_test] {} stream open failed: ret={} path={}",
            expected.name, fd, expected.path
        );
        return false;
    }

    let fd = fd as usize;
    let mut buffer = [0u8; 4096];
    let mut total = 0usize;
    let mut hash = 5381u32;
    let mut read_ok = true;
    loop {
        let count = read(fd, &mut buffer);
        if count < 0 {
            println!(
                "[cargo_cache_integrity_test] {} stream read failed: ret={} offset={}",
                expected.name, count, total
            );
            read_ok = false;
            break;
        }
        if count == 0 {
            break;
        }
        let count = count as usize;
        for byte in &buffer[..count] {
            hash = hash.wrapping_mul(33).wrapping_add(*byte as u32);
        }
        total += count;
    }
    let close_result = close(fd);
    let matches = read_ok && close_result == 0 && total == expected.size && hash == expected.hash;
    println!(
        "[cargo_cache_integrity_test] {} stream size={} expected_size={} hash={:#010x} expected_hash={:#010x} close={} result={}",
        expected.name,
        total,
        expected.size,
        hash,
        expected.hash,
        close_result,
        if matches { "PASS" } else { "FAIL" }
    );
    matches
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    // Cargo repeatedly forks compiler/build-script children. Reproduce the
    // important parent-side state transition before allocating read buffers:
    // resident writable pages become COW, while untouched lazy pages must
    // remain writable when the parent allocates them after the fork.
    let child = fork();
    if child == 0 {
        exit(0);
    }
    if child < 0 {
        println!("[cargo_cache_integrity_test] fork failed: {}", child);
        return 1;
    }
    let mut child_status = -1;
    let waited = waitpid(child as usize, &mut child_status);
    if waited != child || child_status != 0 {
        println!(
            "[cargo_cache_integrity_test] wait failed: child={} waited={} status={}",
            child, waited, child_status
        );
        return 1;
    }
    println!(
        "[cargo_cache_integrity_test] post-fork parent read phase: child={} status={}",
        child, child_status
    );

    let mut passed = true;
    for expected in &CACHE_FILES {
        passed &= check_cache_file_bulk(expected);
        passed &= check_cache_file_stream(expected);
    }
    if passed {
        println!("[cargo_cache_integrity_test] PASS");
        0
    } else {
        println!("[cargo_cache_integrity_test] FAIL");
        1
    }
}
