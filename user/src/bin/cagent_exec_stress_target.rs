#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::ptr::{read_volatile, write_volatile};
use user_lib::{AT_FDCWD, OpenFlags, close, fstat, getpid, open, read};

const PAGE_SIZE: usize = 4096;
const STACK_PAGES: usize = 32;
static mut WRITABLE_DATA: [u64; 2048] = [0; 2048];

#[inline(never)]
fn exercise_private_pages(seed: usize) -> u64 {
    let mut stack = [0u8; STACK_PAGES * PAGE_SIZE];
    let mut checksum = 0u64;
    for page in 0..STACK_PAGES {
        let offset = page * PAGE_SIZE;
        let value = seed.wrapping_add(page * 37) as u8;
        stack[offset] = value;
        checksum = checksum.wrapping_add(stack[offset] as u64);
    }

    let data = (&raw mut WRITABLE_DATA).cast::<u64>();
    for index in 0..2048 {
        let value = (seed as u64).rotate_left((index & 63) as u32) ^ index as u64;
        unsafe {
            write_volatile(data.add(index), value);
            checksum ^= read_volatile(data.add(index));
        }
    }
    core::hint::black_box(checksum)
}

fn verify_executable() -> bool {
    let fd = open(
        AT_FDCWD,
        "/cagent_exec_stress_target",
        OpenFlags::RDONLY,
        0,
    );
    if fd < 0 {
        return false;
    }
    let fd = fd as usize;
    let mut stat = [0u8; 256];
    let mut header = [0u8; PAGE_SIZE];
    let ok = fstat(fd, &mut stat) == 0
        && read(fd, &mut header) > 64
        && header[..4] == [0x7f, b'E', b'L', b'F'];
    let _ = close(fd);
    ok
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    let checksum = exercise_private_pages(getpid() as usize);
    if verify_executable() && checksum != u64::MAX {
        0
    } else {
        println!(
            "[cagent_exec_stress_target] FAIL pid={} checksum={:#x}",
            getpid(),
            checksum
        );
        1
    }
}
