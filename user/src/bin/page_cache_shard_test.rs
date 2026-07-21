#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    AT_FDCWD, OpenFlags, close, exit, fork, get_time, lseek, open, read, unlinkat, waitpid, write,
    yield_,
};

const PAGE_SIZE: usize = 4096;
const WORKERS: usize = 8;
const FILE_PAGES: usize = 16;
const READ_ROUNDS: usize = 8;
const SEEK_SET: i32 = 0;
const PATHS: [&str; WORKERS] = [
    "/page_cache_shard_test.0",
    "/page_cache_shard_test.1",
    "/page_cache_shard_test.2",
    "/page_cache_shard_test.3",
    "/page_cache_shard_test.4",
    "/page_cache_shard_test.5",
    "/page_cache_shard_test.6",
    "/page_cache_shard_test.7",
];

fn pattern(worker: usize, page: usize) -> u8 {
    (worker.wrapping_mul(37).wrapping_add(page.wrapping_mul(13)) & 0xff) as u8
}

fn create_file(worker: usize) -> bool {
    let _ = unlinkat(AT_FDCWD, PATHS[worker], 0);
    let fd = open(
        AT_FDCWD,
        PATHS[worker],
        OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::RDWR,
        0o600,
    );
    if fd < 0 {
        return false;
    }
    let fd = fd as usize;
    let mut page = [0u8; PAGE_SIZE];
    for page_id in 0..FILE_PAGES {
        page.fill(pattern(worker, page_id));
        if write(fd, &page) != PAGE_SIZE as isize {
            let _ = close(fd);
            return false;
        }
    }
    close(fd) == 0
}

fn verify_file(worker: usize) -> bool {
    let fd = open(AT_FDCWD, PATHS[worker], OpenFlags::RDONLY, 0);
    if fd < 0 {
        return false;
    }
    let fd = fd as usize;
    let mut page = [0u8; PAGE_SIZE];
    for round in 0..READ_ROUNDS {
        if lseek(fd, 0, SEEK_SET) != 0 {
            let _ = close(fd);
            return false;
        }
        for page_id in 0..FILE_PAGES {
            if read(fd, &mut page) != PAGE_SIZE as isize
                || page.iter().any(|byte| *byte != pattern(worker, page_id))
            {
                let _ = close(fd);
                return false;
            }
        }
        if round & 1 == 0 {
            yield_();
        }
    }
    close(fd) == 0
}

fn verify_truncate(worker: usize) -> bool {
    let replacement_len = 113 + worker;
    let replacement = pattern(worker, FILE_PAGES + 1);
    let fd = open(
        AT_FDCWD,
        PATHS[worker],
        OpenFlags::O_TRUNC | OpenFlags::RDWR,
        0o600,
    );
    if fd < 0 {
        println!(
            "[page_cache_shard_test] truncate open failed worker={} fd={}",
            worker, fd
        );
        return false;
    }
    let fd = fd as usize;
    let page = [replacement; PAGE_SIZE];
    let write_ret = write(fd, &page[..replacement_len]);
    let close_ret = close(fd);
    if write_ret != replacement_len as isize || close_ret != 0 {
        println!(
            "[page_cache_shard_test] truncate write failed worker={} write={} expected={} close={}",
            worker, write_ret, replacement_len, close_ret
        );
        return false;
    }

    let fd = open(AT_FDCWD, PATHS[worker], OpenFlags::RDONLY, 0);
    if fd < 0 {
        println!(
            "[page_cache_shard_test] reopen failed worker={} fd={}",
            worker, fd
        );
        return false;
    }
    let fd = fd as usize;
    let mut readback = [0u8; PAGE_SIZE];
    let mut eof = [0u8; 1];
    let read_ret = read(fd, &mut readback);
    let content_ok = readback[..replacement_len]
        .iter()
        .all(|byte| *byte == replacement);
    let eof_ret = read(fd, &mut eof);
    let ok = read_ret == replacement_len as isize && content_ok && eof_ret == 0;
    if !ok {
        println!(
            "[page_cache_shard_test] truncate verify failed worker={} read={} expected={} content_ok={} eof={}",
            worker, read_ret, replacement_len, content_ok, eof_ret
        );
    }
    let _ = close(fd);
    ok
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!(
        "[page_cache_shard_test] start workers={} pages={} rounds={}",
        WORKERS, FILE_PAGES, READ_ROUNDS
    );
    for worker in 0..WORKERS {
        if !create_file(worker) {
            println!("[page_cache_shard_test] FAIL: setup worker={}", worker);
            return 1;
        }
    }

    let started = get_time();
    let mut children = [0usize; WORKERS];
    for worker in 0..WORKERS {
        let child = fork();
        if child == 0 {
            exit(if verify_file(worker) { 0 } else { 2 });
        }
        if child < 0 {
            println!("[page_cache_shard_test] FAIL: fork worker={}", worker);
            return 2;
        }
        children[worker] = child as usize;
    }

    let mut children_ok = true;
    for child in children {
        let mut status = -1;
        if waitpid(child, &mut status) != child as isize || status != 0 {
            children_ok = false;
        }
    }
    let elapsed_ms = get_time().saturating_sub(started);

    let mut truncate_ok = true;
    for worker in 0..WORKERS {
        truncate_ok &= verify_truncate(worker);
        let _ = unlinkat(AT_FDCWD, PATHS[worker], 0);
    }

    println!(
        "[page_cache_shard_test] elapsed_ms={} children_ok={} truncate_ok={}",
        elapsed_ms, children_ok, truncate_ok
    );
    if children_ok && truncate_ok {
        println!("[page_cache_shard_test] PASS");
        0
    } else {
        println!("[page_cache_shard_test] FAIL");
        3
    }
}
