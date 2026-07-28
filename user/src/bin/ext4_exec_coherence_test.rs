#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::sync::atomic::{AtomicBool, Ordering};
use user_lib::{
    close, exit, fstatat, ftruncate, lseek, mmap, msync, munmap, open, read, renameat, sync,
    thread_create, unlinkat, waittid, write, yield_, OpenFlags, AT_FDCWD,
};

const MMAP_PATH: &str = "/ext4_shared_mmap_test.bin";
const MMAP_AFTER_CLOSE_PATH: &str = "/ext4_shared_mmap_after_close_test.bin";
const SOURCE_PATH: &str = "/ext4_rename_source.bin";
const TARGET_PATH: &str = "/ext4_rename_target.bin";
const TRUNCATE_RACE_PATH: &str = "/ext4_truncate_load_race.bin";
const PAGE_SIZE: usize = 4096;
const DATA_LEN: usize = PAGE_SIZE * 2;
const PROT_READ: usize = 1;
const PROT_WRITE: usize = 2;
const MAP_SHARED: usize = 1;
const MS_SYNC: usize = 4;
const SEEK_SET: i32 = 0;
const TRUNCATE_RACE_ROUNDS: usize = 64;
const TRUNCATE_PREFIX_LEN: usize = 73;
const TRUNCATE_SUFFIX_LEN: usize = 91;
const MMAP_AFTER_CLOSE_PAGES: usize = 96;

#[repr(C)]
#[derive(Clone, Copy, Default)]
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

static TRUNCATE_RACE_RUN: AtomicBool = AtomicBool::new(false);

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

fn shared_mmap_after_close_roundtrip() -> bool {
    let _ = unlinkat(AT_FDCWD, MMAP_AFTER_CLOSE_PATH, 0);
    let len = MMAP_AFTER_CLOSE_PAGES * PAGE_SIZE;
    let fd = open(
        AT_FDCWD,
        MMAP_AFTER_CLOSE_PATH,
        OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::RDWR,
        0o600,
    );
    if fd < 0 || ftruncate(fd as usize, len) != 0 {
        if fd >= 0 {
            let _ = close(fd as usize);
        }
        return false;
    }
    let address = mmap(0, len, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if address < 0 || close(fd as usize) != 0 || sync() != 0 {
        if address >= 0 {
            let _ = munmap(address as usize, len);
        }
        return false;
    }

    // This is the linker ordering that previously lost all pages dirtied after
    // close had drained the only queued file reference.
    let mapped = unsafe { core::slice::from_raw_parts_mut(address as *mut u8, len) };
    for page in 0..MMAP_AFTER_CLOSE_PAGES {
        mapped[page * PAGE_SIZE] = (page as u8).wrapping_mul(29).wrapping_add(7);
        mapped[(page + 1) * PAGE_SIZE - 1] = (page as u8).wrapping_mul(31).wrapping_add(3);
    }
    if munmap(address as usize, len) != 0 || sync() != 0 {
        return false;
    }

    let mut stat = LinuxStat::default();
    let stat_bytes = unsafe {
        core::slice::from_raw_parts_mut(
            &mut stat as *mut LinuxStat as *mut u8,
            core::mem::size_of::<LinuxStat>(),
        )
    };
    let stat_ok = fstatat(AT_FDCWD, MMAP_AFTER_CLOSE_PATH, stat_bytes, 0) == 0;
    let expected_blocks = (len / 512) as u64;
    let reader = open(AT_FDCWD, MMAP_AFTER_CLOSE_PATH, OpenFlags::RDONLY, 0);
    if !stat_ok || stat.st_blocks < expected_blocks || reader < 0 {
        if reader >= 0 {
            let _ = close(reader as usize);
        }
        let _ = unlinkat(AT_FDCWD, MMAP_AFTER_CLOSE_PATH, 0);
        return false;
    }
    let mut page_data = [0u8; PAGE_SIZE];
    let mut content_ok = true;
    for page in 0..MMAP_AFTER_CLOSE_PAGES {
        if read(reader as usize, &mut page_data) != PAGE_SIZE as isize
            || page_data[0] != (page as u8).wrapping_mul(29).wrapping_add(7)
            || page_data[PAGE_SIZE - 1] != (page as u8).wrapping_mul(31).wrapping_add(3)
        {
            content_ok = false;
            break;
        }
    }
    let close_ok = close(reader as usize) == 0;
    let _ = unlinkat(AT_FDCWD, MMAP_AFTER_CLOSE_PATH, 0);
    content_ok && close_ok
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

extern "C" fn truncate_race_reader(_: usize) -> ! {
    let mut page = [0u8; PAGE_SIZE];
    while TRUNCATE_RACE_RUN.load(Ordering::Acquire) {
        let fd = open(AT_FDCWD, TRUNCATE_RACE_PATH, OpenFlags::RDONLY, 0);
        if fd >= 0 {
            let _ = read(fd as usize, &mut page);
            let _ = close(fd as usize);
        }
        yield_();
    }
    exit(0)
}

fn join_thread(tid: isize) -> bool {
    loop {
        let result = waittid(tid as usize);
        if result == -11 {
            yield_();
            continue;
        }
        return result == 0;
    }
}

fn truncate_load_publication_race() -> bool {
    let _ = unlinkat(AT_FDCWD, TRUNCATE_RACE_PATH, 0);
    let initial_fd = open(
        AT_FDCWD,
        TRUNCATE_RACE_PATH,
        OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::RDWR,
        0o700,
    );
    if initial_fd < 0 {
        return false;
    }
    let initial = [0x19u8; PAGE_SIZE];
    if !write_all(initial_fd as usize, &initial) || close(initial_fd as usize) != 0 {
        return false;
    }

    TRUNCATE_RACE_RUN.store(true, Ordering::Release);
    let tid = thread_create(truncate_race_reader, 0);
    if tid < 0 {
        TRUNCATE_RACE_RUN.store(false, Ordering::Release);
        return false;
    }

    let mut valid = true;
    for round in 0..TRUNCATE_RACE_ROUNDS {
        let prefix_byte = (round as u8).wrapping_mul(17).wrapping_add(0x31);
        let suffix_byte = prefix_byte ^ 0xa5;
        let fd = open(
            AT_FDCWD,
            TRUNCATE_RACE_PATH,
            OpenFlags::O_TRUNC | OpenFlags::RDWR,
            0o700,
        );
        if fd < 0 {
            println!(
                "[ext4_exec_coherence_test] truncate_race_fail round={} stage=truncate_open rc={}",
                round, fd
            );
            valid = false;
            break;
        }
        let fd = fd as usize;
        let prefix = [prefix_byte; TRUNCATE_PREFIX_LEN];
        let suffix = [suffix_byte; TRUNCATE_SUFFIX_LEN];
        if !write_all(fd, &prefix) {
            println!(
                "[ext4_exec_coherence_test] truncate_race_fail round={} stage=prefix_write",
                round
            );
            let _ = close(fd);
            valid = false;
            break;
        }
        let expected_seek = (PAGE_SIZE - TRUNCATE_SUFFIX_LEN) as isize;
        let seek_result = lseek(fd, expected_seek, SEEK_SET);
        if seek_result != expected_seek {
            println!(
                "[ext4_exec_coherence_test] truncate_race_fail round={} stage=suffix_seek rc={} expected={}",
                round, seek_result, expected_seek
            );
            let _ = close(fd);
            valid = false;
            break;
        }
        if !write_all(fd, &suffix) {
            println!(
                "[ext4_exec_coherence_test] truncate_race_fail round={} stage=suffix_write",
                round
            );
            let _ = close(fd);
            valid = false;
            break;
        }
        let close_result = close(fd);
        if close_result != 0 {
            println!(
                "[ext4_exec_coherence_test] truncate_race_fail round={} stage=writer_close rc={}",
                round, close_result
            );
            valid = false;
            break;
        }

        let reader = open(AT_FDCWD, TRUNCATE_RACE_PATH, OpenFlags::RDONLY, 0);
        if reader < 0 {
            println!(
                "[ext4_exec_coherence_test] truncate_race_fail round={} stage=verify_open rc={}",
                round, reader
            );
            valid = false;
            break;
        }
        let mut actual = [0u8; PAGE_SIZE];
        let read_ok = read_all(reader as usize, &mut actual);
        let reader_close = close(reader as usize);
        let mismatch = actual.iter().enumerate().find_map(|(offset, byte)| {
            let expected = if offset < TRUNCATE_PREFIX_LEN {
                prefix_byte
            } else if offset >= PAGE_SIZE - TRUNCATE_SUFFIX_LEN {
                suffix_byte
            } else {
                0
            };
            (*byte != expected).then_some((offset, *byte, expected))
        });
        if !read_ok || reader_close != 0 || mismatch.is_some() {
            let (bad_offset, bad_actual, bad_expected) =
                mismatch.unwrap_or((usize::MAX, 0, 0));
            println!(
                "[ext4_exec_coherence_test] truncate_race_fail round={} stage=verify read_ok={} close_rc={} bad_offset={} actual={:#x} expected={:#x}",
                round,
                read_ok,
                reader_close,
                bad_offset,
                bad_actual,
                bad_expected
            );
            valid = false;
            break;
        }
    }

    TRUNCATE_RACE_RUN.store(false, Ordering::Release);
    let joined = join_thread(tid);
    if !joined {
        println!("[ext4_exec_coherence_test] truncate_race_fail stage=join");
    }
    valid &= joined;
    let _ = unlinkat(AT_FDCWD, TRUNCATE_RACE_PATH, 0);
    valid
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    let mmap_ok = shared_mmap_roundtrip();
    let mmap_after_close_ok = shared_mmap_after_close_roundtrip();
    let rename_ok = rename_replace_roundtrip();
    let truncate_race_ok = truncate_load_publication_race();
    if mmap_ok && mmap_after_close_ok && rename_ok && truncate_race_ok {
        println!("[ext4_exec_coherence_test] PASS");
        0
    } else {
        println!(
            "[ext4_exec_coherence_test] FAIL: mmap_ok={} mmap_after_close_ok={} rename_ok={} truncate_race_ok={}",
            mmap_ok, mmap_after_close_ok, rename_ok, truncate_race_ok
        );
        1
    }
}
