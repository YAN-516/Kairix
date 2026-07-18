#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    AT_FDCWD, OpenFlags, close, fstat, fstatat, ftruncate, mkdir, open, sync, unlinkat, write,
};

const TEST_DIR: &str = "/ext4_stat_blocks_test";
const TEST_FILE: &str = "/ext4_stat_blocks_test/sparse";
const AT_REMOVEDIR: u32 = 0x200;
const S_IFMT: u32 = 0o170000;
const S_IFDIR: u32 = 0o040000;
const SPARSE_SIZE: usize = 1024 * 1024;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct LinuxStat {
    st_dev: u64,
    st_ino: u64,
    st_mode: u32,
    st_nlink: u32,
    st_uid: u32,
    st_gid: u32,
    st_rdev: u64,
    __pad1: u64,
    st_size: i64,
    st_blksize: i32,
    __pad2: i32,
    st_blocks: u64,
    st_atime_sec: i64,
    st_atime_nsec: i64,
    st_mtime_sec: i64,
    st_mtime_nsec: i64,
    st_ctime_sec: i64,
    st_ctime_nsec: i64,
    __glibc_reserved: [i32; 2],
}

const _: [(); 128] = [(); core::mem::size_of::<LinuxStat>()];

fn stat_bytes(stat: &mut LinuxStat) -> &mut [u8] {
    unsafe {
        core::slice::from_raw_parts_mut(
            stat as *mut LinuxStat as *mut u8,
            core::mem::size_of::<LinuxStat>(),
        )
    }
}

fn stat_path(path: &str) -> Result<LinuxStat, isize> {
    let mut stat = LinuxStat::default();
    let ret = fstatat(AT_FDCWD, path, stat_bytes(&mut stat), 0);
    if ret < 0 { Err(ret) } else { Ok(stat) }
}

fn stat_fd(fd: usize) -> Result<LinuxStat, isize> {
    let mut stat = LinuxStat::default();
    let ret = fstat(fd, stat_bytes(&mut stat));
    if ret < 0 { Err(ret) } else { Ok(stat) }
}

fn cleanup() {
    let _ = unlinkat(AT_FDCWD, TEST_FILE, 0);
    let _ = unlinkat(AT_FDCWD, TEST_DIR, AT_REMOVEDIR);
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[ext4_stat_blocks_test] start");
    cleanup();

    let mkdir_ret = mkdir(TEST_DIR, 0o755);
    if mkdir_ret < 0 {
        println!("[ext4_stat_blocks_test] mkdir failed: {}", mkdir_ret);
        return 1;
    }

    let dir_path_stat = match stat_path(TEST_DIR) {
        Ok(stat) => stat,
        Err(ret) => {
            println!("[ext4_stat_blocks_test] fstatat(dir) failed: {}", ret);
            cleanup();
            return 2;
        }
    };
    let dir_fd = open(
        AT_FDCWD,
        TEST_DIR,
        OpenFlags::RDONLY | OpenFlags::O_DIRECTORY,
        0,
    );
    if dir_fd < 0 {
        println!("[ext4_stat_blocks_test] open(dir) failed: {}", dir_fd);
        cleanup();
        return 3;
    }
    let dir_fd_stat = match stat_fd(dir_fd as usize) {
        Ok(stat) => stat,
        Err(ret) => {
            println!("[ext4_stat_blocks_test] fstat(dir) failed: {}", ret);
            let _ = close(dir_fd as usize);
            cleanup();
            return 4;
        }
    };
    let _ = close(dir_fd as usize);
    println!(
        "[ext4_stat_blocks_test] dir size={} blocks={} blksize={}",
        dir_path_stat.st_size, dir_path_stat.st_blocks, dir_path_stat.st_blksize
    );
    if dir_path_stat.st_mode & S_IFMT != S_IFDIR
        || dir_path_stat.st_size <= 0
        || dir_path_stat.st_blocks == 0
        || dir_path_stat.st_blksize < 1024
        || dir_path_stat.st_blocks != dir_fd_stat.st_blocks
    {
        println!("[ext4_stat_blocks_test] directory metadata mismatch");
        cleanup();
        return 5;
    }

    let file_fd = open(
        AT_FDCWD,
        TEST_FILE,
        OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::RDWR,
        0o644,
    );
    if file_fd < 0 {
        println!("[ext4_stat_blocks_test] open(file) failed: {}", file_fd);
        cleanup();
        return 6;
    }
    let file_fd = file_fd as usize;
    if ftruncate(file_fd, SPARSE_SIZE) < 0 {
        println!("[ext4_stat_blocks_test] ftruncate failed");
        let _ = close(file_fd);
        cleanup();
        return 7;
    }
    let _ = sync();

    let sparse_stat = match stat_path(TEST_FILE) {
        Ok(stat) => stat,
        Err(ret) => {
            println!("[ext4_stat_blocks_test] stat(sparse) failed: {}", ret);
            let _ = close(file_fd);
            cleanup();
            return 8;
        }
    };
    println!(
        "[ext4_stat_blocks_test] sparse size={} blocks={}",
        sparse_stat.st_size, sparse_stat.st_blocks
    );
    let full_logical_blocks = (SPARSE_SIZE / 512) as u64;
    if sparse_stat.st_size != SPARSE_SIZE as i64 || sparse_stat.st_blocks >= full_logical_blocks {
        println!("[ext4_stat_blocks_test] sparse allocation was reported as logical size");
        let _ = close(file_fd);
        cleanup();
        return 9;
    }

    if write(file_fd, &[0x5a]) != 1 {
        println!("[ext4_stat_blocks_test] write failed");
        let _ = close(file_fd);
        cleanup();
        return 10;
    }
    let _ = sync();
    let allocated_path = match stat_path(TEST_FILE) {
        Ok(stat) => stat,
        Err(ret) => {
            println!("[ext4_stat_blocks_test] stat(allocated) failed: {}", ret);
            let _ = close(file_fd);
            cleanup();
            return 11;
        }
    };
    let allocated_fd = match stat_fd(file_fd) {
        Ok(stat) => stat,
        Err(ret) => {
            println!("[ext4_stat_blocks_test] fstat(allocated) failed: {}", ret);
            let _ = close(file_fd);
            cleanup();
            return 12;
        }
    };
    println!(
        "[ext4_stat_blocks_test] allocated size={} path_blocks={} fd_blocks={}",
        allocated_path.st_size, allocated_path.st_blocks, allocated_fd.st_blocks
    );
    let ok = allocated_path.st_size == SPARSE_SIZE as i64
        && allocated_path.st_blocks > 0
        && allocated_path.st_blocks < full_logical_blocks
        && allocated_path.st_blocks == allocated_fd.st_blocks;

    let _ = close(file_fd);
    cleanup();
    if !ok {
        println!("[ext4_stat_blocks_test] allocated block metadata mismatch");
        return 13;
    }

    println!("[ext4_stat_blocks_test] PASS");
    0
}
