#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::ptr::write_volatile;
use user_lib::{AT_FDCWD, OpenFlags, SigAction, close, exit, fork, mmap, open, sigaction, waitpid, write};

const PAGE_SIZE: usize = 4096;
const PROT_READ: usize = 0x1;
const MAP_SHARED: usize = 0x01;
const SIGBUS: i32 = 7;
const SIGSEGV: i32 = 11;
const SIGSEGV_OK: i32 = 42;
const SIGBUS_BAD: i32 = 43;

unsafe extern "C" fn fault_handler(sig: i32) {
    if sig == SIGSEGV {
        exit(SIGSEGV_OK);
    }
    if sig == SIGBUS {
        exit(SIGBUS_BAD);
    }
    exit(44);
}

fn decode_exit(status: i32) -> Option<i32> {
    if (status & 0x7f) == 0 {
        Some((status >> 8) & 0xff)
    } else {
        None
    }
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("[mmap_prot_sig] start");

    let path = "mmap_prot_sig.tmp";
    let fd = open(
        AT_FDCWD,
        path,
        OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::WRONLY,
        0,
    );
    if fd < 0 {
        println!("[mmap_prot_sig] create failed: {}", fd);
        return 1;
    }
    let data = [0x5au8];
    let wrote = write(fd as usize, &data);
    let _ = close(fd as usize);
    if wrote != data.len() as isize {
        println!("[mmap_prot_sig] write failed: {}", wrote);
        return 1;
    }

    let fd = open(AT_FDCWD, path, OpenFlags::RDONLY, 0);
    if fd < 0 {
        println!("[mmap_prot_sig] reopen failed: {}", fd);
        return 1;
    }

    let pid = fork();
    if pid == 0 {
        let act = SigAction::custom(fault_handler);
        if sigaction(SIGSEGV, Some(&act), None) != 0 {
            exit(10);
        }
        if sigaction(SIGBUS, Some(&act), None) != 0 {
            exit(11);
        }

        let addr = mmap(0, PAGE_SIZE, PROT_READ, MAP_SHARED, fd, 0);
        if addr < 0 {
            exit(12);
        }

        unsafe {
            write_volatile(addr as *mut u8, 0xa5);
        }

        exit(13);
    }
    let _ = close(fd as usize);
    if pid < 0 {
        println!("[mmap_prot_sig] fork failed: {}", pid);
        return 1;
    }

    let mut status = 0;
    let waited = waitpid(pid as usize, &mut status);
    if waited != pid {
        println!(
            "[mmap_prot_sig] wait failed: pid {}, waited {}, status {}",
            pid, waited, status
        );
        return 1;
    }

    match decode_exit(status) {
        Some(SIGSEGV_OK) => {
            println!("[mmap_prot_sig] PASS: write to PROT_READ mmap raised SIGSEGV");
            0
        }
        Some(SIGBUS_BAD) => {
            println!("[mmap_prot_sig] FAIL: got SIGBUS, expected SIGSEGV");
            1
        }
        Some(13) => {
            println!("[mmap_prot_sig] FAIL: write to PROT_READ mmap succeeded");
            1
        }
        Some(code) => {
            println!("[mmap_prot_sig] FAIL: child exit {}", code);
            1
        }
        None => {
            println!(
                "[mmap_prot_sig] FAIL: child was killed before custom handler, status {}",
                status
            );
            1
        }
    }
}
