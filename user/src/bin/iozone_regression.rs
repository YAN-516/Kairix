#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    OpenFlags, close, exit, fork, lseek, mmap, munmap, open, read, sync, unlinkat, waitpid, write,
};

const AT_FDCWD: isize = -100;
const SEEK_SET: i32 = 0;
const PROT_READ: usize = 0x1;
const PROT_WRITE: usize = 0x2;
const MAP_PRIVATE: usize = 0x02;
const MAP_ANONYMOUS: usize = 0x20;
const MAP_FIXED_NOREPLACE: usize = 0x100000;
const CHILDREN: usize = 4;
const FILE_SIZE: usize = 1024 * 1024;
const RECORD_SIZE: usize = 1024;
const RECORDS: usize = FILE_SIZE / RECORD_SIZE;
const STRIDE_RECORDS: usize = 17;
const BULK_SIZE: usize = 128 * 1024;
const CHILD_OK: i32 = 42;

const PATHS: [&str; CHILDREN] = [
    "iozone_regress_0.dat",
    "iozone_regress_1.dat",
    "iozone_regress_2.dat",
    "iozone_regress_3.dat",
];

fn fill_record(buf: &mut [u8; RECORD_SIZE], child: usize, record: usize) {
    let base = ((child as u8) << 5) ^ (record as u8);
    for (idx, byte) in buf.iter_mut().enumerate() {
        *byte = base.wrapping_add(idx as u8);
    }
}

fn verify_record(buf: &[u8; RECORD_SIZE], child: usize, record: usize) -> bool {
    let base = ((child as u8) << 5) ^ (record as u8);
    for (idx, byte) in buf.iter().enumerate() {
        if *byte != base.wrapping_add(idx as u8) {
            println!(
                "[iozone_regression] verify mismatch child={} record={} byte={} got={} expected={}",
                child,
                record,
                idx,
                *byte,
                base.wrapping_add(idx as u8)
            );
            return false;
        }
    }
    true
}

fn write_child(child: usize) -> ! {
    let path = PATHS[child];
    let _ = unlinkat(AT_FDCWD, path, 0);
    let fd = open(
        AT_FDCWD,
        path,
        OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::RDWR,
        0o666,
    );
    if fd < 0 {
        println!("[iozone_regression] child={} open failed ret={}", child, fd);
        exit(1);
    }

    let mut buf = [0u8; RECORD_SIZE];
    for record in 0..RECORDS {
        fill_record(&mut buf, child, record);
        let ret = write(fd as usize, &buf);
        if ret != RECORD_SIZE as isize {
            println!(
                "[iozone_regression] child={} write record={} ret={}",
                child, record, ret
            );
            let _ = close(fd as usize);
            exit(2);
        }
        if record != 0 && record % 256 == 0 {
            println!("[iozone_regression] child={} wrote_kb={}", child, record);
        }
    }

    println!("[iozone_regression] child={} closing", child);
    let ret = close(fd as usize);
    if ret != 0 {
        println!(
            "[iozone_regression] child={} close failed ret={}",
            child, ret
        );
        exit(3);
    }
    println!("[iozone_regression] child={} done", child);
    exit(CHILD_OK);
}

fn wait_child(pid: isize) -> bool {
    let mut status = 0;
    let waited = waitpid(pid as usize, &mut status);
    let exited = (status & 0x7f) == 0;
    let exit_code = (status >> 8) & 0xff;
    println!(
        "[iozone_regression] wait pid={} waited={} status={} exit={}",
        pid, waited, status, exit_code
    );
    waited == pid && exited && exit_code == CHILD_OK
}

fn wait_expected_child(pid: isize, expected: i32) -> bool {
    let mut status = 0;
    let waited = waitpid(pid as usize, &mut status);
    let exited = (status & 0x7f) == 0;
    let exit_code = (status >> 8) & 0xff;
    println!(
        "[iozone_regression] wait pid={} waited={} status={} exit={} expected={}",
        pid, waited, status, exit_code, expected
    );
    waited == pid && exited && exit_code == expected
}

fn stride_read_child(child: usize) -> bool {
    let path = PATHS[child];
    let fd = open(AT_FDCWD, path, OpenFlags::RDONLY, 0);
    if fd < 0 {
        println!(
            "[iozone_regression] read open child={} failed ret={}",
            child, fd
        );
        return false;
    }

    let mut buf = [0u8; RECORD_SIZE];
    for start in 0..STRIDE_RECORDS {
        let mut record = start;
        while record < RECORDS {
            let off = (record * RECORD_SIZE) as isize;
            let seek_ret = lseek(fd as usize, off, SEEK_SET);
            if seek_ret != off {
                println!(
                    "[iozone_regression] lseek child={} record={} ret={}",
                    child, record, seek_ret
                );
                let _ = close(fd as usize);
                return false;
            }
            let read_ret = read(fd as usize, &mut buf);
            if read_ret != RECORD_SIZE as isize {
                println!(
                    "[iozone_regression] read child={} record={} ret={}",
                    child, record, read_ret
                );
                let _ = close(fd as usize);
                return false;
            }
            if !verify_record(&buf, child, record) {
                let _ = close(fd as usize);
                return false;
            }
            record += STRIDE_RECORDS;
        }
    }

    let _ = close(fd as usize);
    true
}

fn concurrent_stride_read_child(child: usize) -> ! {
    println!("[iozone_regression] concurrent read child={} start", child);
    if stride_read_child(child) {
        println!("[iozone_regression] concurrent read child={} done", child);
        exit(CHILD_OK);
    }
    println!("[iozone_regression] concurrent read child={} failed", child);
    exit(6);
}

fn run_concurrent_stride_reads() -> bool {
    let mut pids = [0isize; CHILDREN];
    for child in 0..CHILDREN {
        let pid = fork();
        if pid == 0 {
            concurrent_stride_read_child(child);
        }
        if pid < 0 {
            println!(
                "[iozone_regression] concurrent read fork child={} failed ret={}",
                child, pid
            );
            return false;
        }
        pids[child] = pid;
    }

    for pid in pids {
        if !wait_expected_child(pid, CHILD_OK) {
            return false;
        }
    }
    true
}

fn mmap_buffer(hint: usize, len: usize) -> Option<&'static mut [u8]> {
    let mut addr = mmap(
        hint,
        len,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED_NOREPLACE,
        -1,
        0,
    );
    if addr < 0 {
        println!(
            "[iozone_regression] mmap fixed buffer failed hint={:#x} len={} ret={}",
            hint, len, addr
        );
        addr = mmap(
            0,
            len,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS,
            -1,
            0,
        );
        if addr < 0 {
            println!(
                "[iozone_regression] mmap fallback buffer failed len={} ret={}",
                len, addr
            );
            return None;
        }
    }
    println!(
        "[iozone_regression] mmap buffer addr={:#x} len={}",
        addr as usize, len
    );
    Some(unsafe { core::slice::from_raw_parts_mut(addr as *mut u8, len) })
}

fn fread_like_1k_child(child: usize) -> bool {
    let Some(buf) = mmap_buffer(0x4010_0000, RECORD_SIZE) else {
        return false;
    };
    let path = PATHS[child];
    let fd = open(AT_FDCWD, path, OpenFlags::RDONLY, 0);
    if fd < 0 {
        println!(
            "[iozone_regression] fread-like open child={} failed ret={}",
            child, fd
        );
        let _ = munmap(buf.as_ptr() as usize, RECORD_SIZE);
        return false;
    }

    for record in 0..RECORDS {
        let read_ret = read(fd as usize, &mut buf[..RECORD_SIZE]);
        if read_ret != RECORD_SIZE as isize {
            println!(
                "[iozone_regression] fread-like read child={} record={} buf={:#x} ret={}",
                child,
                record,
                buf.as_ptr() as usize,
                read_ret
            );
            let _ = close(fd as usize);
            let _ = munmap(buf.as_ptr() as usize, RECORD_SIZE);
            return false;
        }
        let record_buf = unsafe { &*(buf.as_ptr() as *const [u8; RECORD_SIZE]) };
        if !verify_record(record_buf, child, record) {
            let _ = close(fd as usize);
            let _ = munmap(buf.as_ptr() as usize, RECORD_SIZE);
            return false;
        }
    }

    let _ = close(fd as usize);
    let _ = munmap(buf.as_ptr() as usize, RECORD_SIZE);
    true
}

fn fread_like_bulk_child(child: usize) -> bool {
    let Some(buf) = mmap_buffer(0x4010_0000, BULK_SIZE) else {
        return false;
    };
    let path = PATHS[child];
    let fd = open(AT_FDCWD, path, OpenFlags::RDONLY, 0);
    if fd < 0 {
        println!(
            "[iozone_regression] bulk open child={} failed ret={}",
            child, fd
        );
        let _ = munmap(buf.as_ptr() as usize, BULK_SIZE);
        return false;
    }

    let mut done = 0usize;
    while done < FILE_SIZE {
        let chunk = (FILE_SIZE - done).min(BULK_SIZE);
        let read_ret = read(fd as usize, &mut buf[..chunk]);
        if read_ret != chunk as isize {
            println!(
                "[iozone_regression] bulk read child={} offset={} buf={:#x} len={} ret={}",
                child,
                done,
                buf.as_ptr() as usize,
                chunk,
                read_ret
            );
            let _ = close(fd as usize);
            let _ = munmap(buf.as_ptr() as usize, BULK_SIZE);
            return false;
        }
        let records = chunk / RECORD_SIZE;
        for idx in 0..records {
            let record = done / RECORD_SIZE + idx;
            let start = idx * RECORD_SIZE;
            let record_buf =
                unsafe { &*(buf[start..start + RECORD_SIZE].as_ptr() as *const [u8; RECORD_SIZE]) };
            if !verify_record(record_buf, child, record) {
                let _ = close(fd as usize);
                let _ = munmap(buf.as_ptr() as usize, BULK_SIZE);
                return false;
            }
        }
        done += chunk;
    }

    let _ = close(fd as usize);
    let _ = munmap(buf.as_ptr() as usize, BULK_SIZE);
    true
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("[iozone_regression] start");
    let mut pids = [0isize; CHILDREN];
    for child in 0..CHILDREN {
        let pid = fork();
        if pid == 0 {
            write_child(child);
        }
        if pid < 0 {
            println!(
                "[iozone_regression] fork child={} failed ret={}",
                child, pid
            );
            return 1;
        }
        pids[child] = pid;
    }

    for pid in pids {
        if !wait_child(pid) {
            return 2;
        }
    }

    println!("[iozone_regression] sync before read");
    let sync_ret = sync();
    println!("[iozone_regression] sync ret={}", sync_ret);

    println!("[iozone_regression] concurrent stride read");
    if !run_concurrent_stride_reads() {
        return 3;
    }

    for child in 0..CHILDREN {
        if !stride_read_child(child) {
            return 4;
        }
    }

    println!("[iozone_regression] fread-like 1k read");
    for child in 0..CHILDREN {
        if !fread_like_1k_child(child) {
            return 5;
        }
    }

    println!("[iozone_regression] fread-like 128k read");
    for child in 0..CHILDREN {
        if !fread_like_bulk_child(child) {
            return 6;
        }
    }

    println!("[iozone_regression] pass");
    0
}
