#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::arch::asm;
use user_lib::{mmap, munmap};

const PAGE_SIZE: usize = 4096;
const PROT_READ: usize = 1;
const PROT_WRITE: usize = 2;
const MAP_PRIVATE: usize = 2;
const MAP_ANONYMOUS: usize = 0x20;

#[cfg(target_arch = "riscv64")]
fn raw_syscall(id: usize, args: [usize; 6]) -> isize {
    let result: isize;
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") args[0] => result,
            in("a1") args[1],
            in("a2") args[2],
            in("a3") args[3],
            in("a4") args[4],
            in("a5") args[5],
            in("a7") id,
        );
    }
    result
}

#[cfg(target_arch = "loongarch64")]
fn raw_syscall(id: usize, args: [usize; 6]) -> isize {
    let result: isize;
    unsafe {
        asm!(
            "syscall 0",
            inlateout("$a0") args[0] => result,
            in("$a1") args[1],
            in("$a2") args[2],
            in("$a3") args[3],
            in("$a4") args[4],
            in("$a5") args[5],
            in("$a7") id,
        );
    }
    result
}

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

#[repr(C)]
struct Timespec {
    sec: i64,
    nsec: i64,
}

fn machine_matches(uts: &[u8; 390], expected: &[u8]) -> bool {
    let machine = &uts[4 * 65..5 * 65];
    machine.starts_with(expected) && machine.get(expected.len()) == Some(&0)
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[linux_abi_regression] start");
    let mut failures = 0;

    let mut uts = [0u8; 390];
    let uname = raw_syscall(160, [uts.as_mut_ptr() as usize, 0, 0, 0, 0, 0]);
    #[cfg(target_arch = "riscv64")]
    let machine_ok = machine_matches(&uts, b"riscv64");
    #[cfg(target_arch = "loongarch64")]
    let machine_ok = machine_matches(&uts, b"loongarch64");
    if uname != 0 || !machine_ok {
        println!("[linux_abi_regression] FAIL uname ret={}", uname);
        failures += 1;
    }

    let mut cpu = u32::MAX;
    let mut node = u32::MAX;
    let getcpu = raw_syscall(168, [
        &mut cpu as *mut u32 as usize,
        &mut node as *mut u32 as usize,
        0,
        0,
        0,
        0,
    ]);
    if getcpu != 0 || cpu >= 64 || node != 0 {
        println!(
            "[linux_abi_regression] FAIL getcpu ret={} cpu={} node={}",
            getcpu, cpu, node
        );
        failures += 1;
    }

    let name = b"abi-regression\0";
    let set_name = raw_syscall(167, [15, name.as_ptr() as usize, 0, 0, 0, 0]);
    let mut returned_name = [0u8; 16];
    let get_name = raw_syscall(167, [16, returned_name.as_mut_ptr() as usize, 0, 0, 0, 0]);
    let unknown_prctl = raw_syscall(167, [usize::MAX, 0, 0, 0, 0, 0]);
    if set_name != 0
        || get_name != 0
        || !returned_name.starts_with(b"abi-regression")
        || unknown_prctl != -22
    {
        println!(
            "[linux_abi_regression] FAIL prctl set={} get={} unknown={}",
            set_name, get_name, unknown_prctl
        );
        failures += 1;
    }

    let mapping = mmap(
        0,
        3 * PAGE_SIZE,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0,
    );
    if mapping < 0 {
        println!("[linux_abi_regression] FAIL mmap ret={}", mapping);
        failures += 1;
    } else {
        let base = mapping as usize;
        let unmap_middle = munmap(base + PAGE_SIZE, PAGE_SIZE);
        let protect_hole = raw_syscall(226, [base, 3 * PAGE_SIZE, PROT_READ, 0, 0, 0]);
        if unmap_middle != 0 || protect_hole != -12 {
            println!(
                "[linux_abi_regression] FAIL mprotect-hole unmap={} protect={}",
                unmap_middle, protect_hole
            );
            failures += 1;
        }
        let _ = munmap(base, PAGE_SIZE);
        let _ = munmap(base + 2 * PAGE_SIZE, PAGE_SIZE);
    }

    let timeout = Timespec { sec: 0, nsec: 0 };
    let mut pollfd = PollFd {
        fd: 511,
        events: 1,
        revents: 0,
    };
    let poll = raw_syscall(73, [
        &mut pollfd as *mut PollFd as usize,
        1,
        &timeout as *const Timespec as usize,
        0,
        0,
        0,
    ]);
    if poll != 1 || pollfd.revents & 0x20 == 0 {
        println!(
            "[linux_abi_regression] FAIL ppoll ret={} revents={:#x}",
            poll, pollfd.revents
        );
        failures += 1;
    }

    let mut readfds = [0u64; 8];
    readfds[7] = 1u64 << 63;
    let select = raw_syscall(72, [
        512,
        readfds.as_mut_ptr() as usize,
        0,
        0,
        &timeout as *const Timespec as usize,
        0,
    ]);
    if select != -9 {
        println!("[linux_abi_regression] FAIL pselect EBADF ret={}", select);
        failures += 1;
    }

    if failures == 0 {
        println!("[linux_abi_regression] PASS");
        0
    } else {
        println!("[linux_abi_regression] FAIL count={}", failures);
        1
    }
}
