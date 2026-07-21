#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use user_lib::{exit, fork, getpid, gettid, mmap, thread_create, waitpid, yield_};

const WORKERS: usize = 4;
const PAGE_SIZE: usize = 4096;
const MAP_LEN: usize = WORKERS * PAGE_SIZE;
const PROT_READ: usize = 0x1;
const PROT_WRITE: usize = 0x2;
const MAP_PRIVATE: usize = 0x02;
const MAP_ANONYMOUS: usize = 0x20;
static STARTED: AtomicUsize = AtomicUsize::new(0);
static WRITERS_ACTIVE: AtomicUsize = AtomicUsize::new(0);
static START_FORK: AtomicBool = AtomicBool::new(false);
static CHILD_PID: AtomicUsize = AtomicUsize::new(0);
static MAPPING_BASE: AtomicUsize = AtomicUsize::new(0);

fn load_page(base: usize, page: usize) -> usize {
    unsafe { read_volatile((base + page * PAGE_SIZE) as *const usize) }
}

fn store_page(base: usize, page: usize, value: usize) {
    unsafe { write_volatile((base + page * PAGE_SIZE) as *mut usize, value) }
}

extern "C" fn worker(index: usize) -> ! {
    STARTED.fetch_add(1, Ordering::Release);
    while !START_FORK.load(Ordering::Acquire) {
        yield_();
    }

    if index == 0 {
        while WRITERS_ACTIVE.load(Ordering::Acquire) != WORKERS - 1 {
            yield_();
        }
        let child = fork();
        if child == 0 {
            let pid = getpid();
            let tid = gettid();
            let base = MAPPING_BASE.load(Ordering::Acquire);
            let snapshot = [load_page(base, 1), load_page(base, 2), load_page(base, 3)];
            // Parent siblings keep writing these pages. After fork they must
            // fault and get private copies; stale writable TLB entries would
            // otherwise mutate the child's snapshot through the shared frame.
            for _ in 0..20_000 {
                yield_();
            }
            let cow_stable = snapshot[0] == load_page(base, 1)
                && snapshot[1] == load_page(base, 2)
                && snapshot[2] == load_page(base, 3);
            println!(
                "[multithread_fork_test] child after fork: pid={} tid={} cow_stable={}",
                pid, tid, cow_stable
            );
            exit(if pid == tid && pid > 0 && cow_stable {
                0
            } else {
                3
            });
        }
        if child < 0 {
            CHILD_PID.store(usize::MAX, Ordering::Release);
        } else {
            CHILD_PID.store(child as usize, Ordering::Release);
        }
    } else {
        let base = MAPPING_BASE.load(Ordering::Acquire);
        let mut value = index;
        store_page(base, index, value);
        WRITERS_ACTIVE.fetch_add(1, Ordering::Release);
        loop {
            value = value.wrapping_add(WORKERS);
            store_page(base, index, value);
            if value & 0xff == 0 {
                yield_();
            }
        }
    }

    loop {
        yield_();
    }
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[multithread_fork_test] start: workers={}", WORKERS);
    let mapped = mmap(
        0,
        MAP_LEN,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0,
    );
    if mapped < 0 {
        println!("[multithread_fork_test] FAIL: mmap={}", mapped);
        return 1;
    }
    let base = mapped as usize;
    for page in 0..WORKERS {
        store_page(base, page, page);
    }
    MAPPING_BASE.store(base, Ordering::Release);

    for index in 0..WORKERS {
        let tid = thread_create(worker, index);
        if tid < 0 {
            println!(
                "[multithread_fork_test] FAIL: worker={} create_ret={}",
                index, tid
            );
            return 1;
        }
    }

    while STARTED.load(Ordering::Acquire) != WORKERS {
        yield_();
    }
    START_FORK.store(true, Ordering::Release);

    let child = loop {
        let child = CHILD_PID.load(Ordering::Acquire);
        if child != 0 {
            break child;
        }
        yield_();
    };
    if child == usize::MAX {
        println!("[multithread_fork_test] FAIL: fork returned an error");
        return 1;
    }

    let mut status = 0;
    let waited = waitpid(child, &mut status);
    if waited == child as isize && status == 0 {
        println!("[multithread_fork_test] PASS");
        0
    } else {
        println!(
            "[multithread_fork_test] FAIL: child={} waited={} status={}",
            child, waited, status
        );
        1
    }
}
