#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::ptr::{read_volatile, write_volatile};
use user_lib::{close, exit, fork, mmap, munmap, pipe, read, waitpid, write};

const PAGE_SIZE: usize = 4096;
const MAP_LEN: usize = 3 * PAGE_SIZE;
const PROT_READ: usize = 0x1;
const PROT_WRITE: usize = 0x2;
const MAP_SHARED: usize = 0x01;
const MAP_ANONYMOUS: usize = 0x20;
const CHILD_OK: i32 = 42;

fn load(base: usize, page: usize) -> u8 {
    unsafe { read_volatile((base + page * PAGE_SIZE) as *const u8) }
}

fn store(base: usize, page: usize, value: u8) {
    unsafe {
        write_volatile((base + page * PAGE_SIZE) as *mut u8, value);
    }
}

fn child(base: usize, parent_to_child: [i32; 2], child_to_parent: [i32; 2]) -> ! {
    let _ = close(parent_to_child[1] as usize);
    let _ = close(child_to_parent[0] as usize);

    if load(base, 0) != 0x11 {
        println!("[shared_anon_fork_test] FAIL: child pre-fork page not shared");
        exit(2);
    }

    // Page 1 has never been touched. Its first fault occurs in the child
    // after fork and must publish the page to the parent's mapping.
    store(base, 0, 0x22);
    store(base, 1, 0x33);
    if write(child_to_parent[1] as usize, &[b'C']) != 1 {
        println!("[shared_anon_fork_test] FAIL: child notify");
        exit(3);
    }

    let mut token = [0u8; 1];
    if read(parent_to_child[0] as usize, &mut token) != 1 || token[0] != b'P' {
        println!("[shared_anon_fork_test] FAIL: child wait");
        exit(4);
    }

    // Page 2 is first faulted by the parent after fork.
    if load(base, 0) != 0x22 || load(base, 1) != 0x33 || load(base, 2) != 0x44 {
        println!(
            "[shared_anon_fork_test] FAIL: child values [{}, {}, {}]",
            load(base, 0),
            load(base, 1),
            load(base, 2)
        );
        exit(5);
    }
    exit(CHILD_OK);
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[shared_anon_fork_test] start");
    let mapped = mmap(
        0,
        MAP_LEN,
        PROT_READ | PROT_WRITE,
        MAP_SHARED | MAP_ANONYMOUS,
        -1,
        0,
    );
    if mapped < 0 {
        println!("[shared_anon_fork_test] FAIL: mmap={}", mapped);
        return 1;
    }
    let base = mapped as usize;

    // Only page 0 is resident before fork. Pages 1 and 2 deliberately remain
    // untouched so both post-fork first-fault directions are exercised.
    store(base, 0, 0x11);

    let mut parent_to_child = [-1i32; 2];
    let mut child_to_parent = [-1i32; 2];
    if pipe(&mut parent_to_child) < 0 || pipe(&mut child_to_parent) < 0 {
        println!("[shared_anon_fork_test] FAIL: pipe");
        let _ = munmap(base, MAP_LEN);
        return 2;
    }

    let pid = fork();
    if pid == 0 {
        child(base, parent_to_child, child_to_parent);
    }
    if pid < 0 {
        println!("[shared_anon_fork_test] FAIL: fork={}", pid);
        let _ = munmap(base, MAP_LEN);
        return 3;
    }

    let _ = close(parent_to_child[0] as usize);
    let _ = close(child_to_parent[1] as usize);
    let mut token = [0u8; 1];
    if read(child_to_parent[0] as usize, &mut token) != 1 || token[0] != b'C' {
        println!("[shared_anon_fork_test] FAIL: parent wait");
        return 4;
    }
    if load(base, 0) != 0x22 || load(base, 1) != 0x33 {
        println!(
            "[shared_anon_fork_test] FAIL: parent values [{}, {}]",
            load(base, 0),
            load(base, 1)
        );
        return 5;
    }

    store(base, 2, 0x44);
    if write(parent_to_child[1] as usize, &[b'P']) != 1 {
        println!("[shared_anon_fork_test] FAIL: parent notify");
        return 6;
    }

    let mut status = 0;
    let waited = waitpid(pid as usize, &mut status);
    let exit_code = (status >> 8) & 0xff;
    let _ = close(parent_to_child[1] as usize);
    let _ = close(child_to_parent[0] as usize);
    let unmap_ret = munmap(base, MAP_LEN);
    if waited != pid || exit_code != CHILD_OK || unmap_ret != 0 {
        println!(
            "[shared_anon_fork_test] FAIL: waited={} status={} exit={} munmap={}",
            waited, status, exit_code, unmap_ret
        );
        return 7;
    }

    println!("[shared_anon_fork_test] PASS");
    0
}
