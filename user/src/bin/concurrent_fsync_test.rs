#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::arch::asm;
use core::sync::atomic::{AtomicU32, Ordering};
use user_lib::{OpenFlags, close, exit, fork, open, unlinkat, waitpid, write};

const PAGE_SIZE: usize = 4096;
const WORKERS: usize = 4;
const ROUNDS: usize = 16;
const FILE_SIZE: usize = 1024 * 1024;
const RECORD_SIZE: usize = 1024;
const CHILD_OK: i32 = 42;

const AT_FDCWD: isize = -100;
const SYS_FSYNC: usize = 82;
const SYS_FUTEX: usize = 98;
const SYS_SHMGET: usize = 194;
const SYS_SHMCTL: usize = 195;
const SYS_SHMAT: usize = 196;
const SYS_SHMDT: usize = 197;

const FUTEX_WAIT: usize = 0;
const FUTEX_WAKE: usize = 1;
const IPC_CREAT: usize = 0o1000;
const IPC_RMID: usize = 0;

const PATHS: [&str; WORKERS] = [
    "concurrent_fsync_0.dat",
    "concurrent_fsync_1.dat",
    "concurrent_fsync_2.dat",
    "concurrent_fsync_3.dat",
];

#[repr(C, align(64))]
struct SharedState {
    generation: AtomicU32,
    arrived: AtomicU32,
    failures: AtomicU32,
    stages: [AtomicU32; WORKERS],
}

#[cfg(target_arch = "riscv64")]
#[inline(always)]
unsafe fn raw_syscall(id: usize, args: [usize; 6]) -> isize {
    let mut ret: isize;
    unsafe {
        asm!(
            "ecall",
            inlateout("x10") args[0] => ret,
            in("x11") args[1],
            in("x12") args[2],
            in("x13") args[3],
            in("x14") args[4],
            in("x15") args[5],
            in("x17") id,
        );
    }
    ret
}

#[cfg(target_arch = "loongarch64")]
#[inline(always)]
unsafe fn raw_syscall(id: usize, args: [usize; 6]) -> isize {
    let ret: isize;
    unsafe {
        asm!(
            "syscall 0",
            inlateout("$a0") args[0] => ret,
            in("$a1") args[1],
            in("$a2") args[2],
            in("$a3") args[3],
            in("$a4") args[4],
            in("$a5") args[5],
            in("$a7") id,
        );
    }
    ret
}

fn fsync(fd: usize) -> isize {
    unsafe { raw_syscall(SYS_FSYNC, [fd, 0, 0, 0, 0, 0]) }
}

fn futex(addr: *const AtomicU32, op: usize, val: u32) -> isize {
    unsafe { raw_syscall(SYS_FUTEX, [addr as usize, op, val as usize, 0, 0, 0]) }
}

fn wait_at_barrier(state: &SharedState) -> bool {
    let generation = state.generation.load(Ordering::Acquire);
    let arrived = state.arrived.fetch_add(1, Ordering::AcqRel) + 1;
    if arrived == WORKERS as u32 {
        state.arrived.store(0, Ordering::Relaxed);
        state.generation.fetch_add(1, Ordering::Release);
        return futex(&state.generation, FUTEX_WAKE, u32::MAX) >= 0;
    }

    while state.generation.load(Ordering::Acquire) == generation {
        let ret = futex(&state.generation, FUTEX_WAIT, generation);
        if ret < 0 && ret != -4 && ret != -11 {
            return false;
        }
    }
    true
}

fn child_main(worker: usize, round: usize, state: &SharedState) -> ! {
    let path = PATHS[worker];
    println!(
        "[concurrent_fsync_test] round={} child={} open enter",
        round + 1,
        worker
    );
    let fd = open(
        AT_FDCWD,
        path,
        OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::RDWR,
        0o666,
    );
    if fd < 0 {
        println!(
            "[concurrent_fsync_test] round={} child={} open ret={}",
            round + 1,
            worker,
            fd
        );
        exit(2);
    }
    println!(
        "[concurrent_fsync_test] round={} child={} open done fd={}",
        round + 1,
        worker,
        fd
    );

    state.stages[worker].store(1, Ordering::Release);
    if !wait_at_barrier(state) {
        state.failures.fetch_add(1, Ordering::Relaxed);
        exit(3);
    }

    let mut record = [0u8; RECORD_SIZE];
    for (index, byte) in record.iter_mut().enumerate() {
        *byte = (round as u8)
            .wrapping_mul(17)
            .wrapping_add((worker as u8).wrapping_mul(31))
            .wrapping_add(index as u8);
    }
    for record_index in 0..(FILE_SIZE / RECORD_SIZE) {
        record[0] = (record_index & 0xff) as u8;
        let ret = write(fd as usize, &record);
        if ret != RECORD_SIZE as isize {
            println!(
                "[concurrent_fsync_test] round={} child={} write record={} ret={}",
                round + 1,
                worker,
                record_index,
                ret
            );
            let _ = close(fd as usize);
            exit(4);
        }
        if record_index != 0 && record_index % 256 == 0 {
            println!(
                "[concurrent_fsync_test] round={} child={} wrote_kb={}",
                round + 1,
                worker,
                record_index
            );
        }
    }

    state.stages[worker].store(2, Ordering::Release);
    if !wait_at_barrier(state) {
        state.failures.fetch_add(1, Ordering::Relaxed);
        exit(5);
    }

    println!(
        "[concurrent_fsync_test] round={} child={} fsync enter",
        round + 1,
        worker
    );
    state.stages[worker].store(3, Ordering::Release);
    let ret = fsync(fd as usize);
    if ret != 0 {
        println!(
            "[concurrent_fsync_test] round={} child={} fsync ret={}",
            round + 1,
            worker,
            ret
        );
        let _ = close(fd as usize);
        exit(6);
    }
    state.stages[worker].store(4, Ordering::Release);
    println!(
        "[concurrent_fsync_test] round={} child={} fsync done",
        round + 1,
        worker
    );

    if !wait_at_barrier(state) {
        state.failures.fetch_add(1, Ordering::Relaxed);
        exit(7);
    }
    state.stages[worker].store(5, Ordering::Release);
    if close(fd as usize) != 0 {
        exit(8);
    }
    state.stages[worker].store(6, Ordering::Release);
    exit(CHILD_OK);
}

fn wait_child(pid: isize, round: usize, worker: usize, state: &SharedState) -> bool {
    let mut status = 0i32;
    let waited = waitpid(pid as usize, &mut status);
    let exit_code = (status >> 8) & 0xff;
    if waited != pid || (status & 0x7f) != 0 || exit_code != CHILD_OK {
        println!(
            "[concurrent_fsync_test] FAIL: round={} child={} pid={} waited={} status={} exit={} stages=[{}, {}, {}, {}]",
            round + 1,
            worker,
            pid,
            waited,
            status,
            exit_code,
            state.stages[0].load(Ordering::Acquire),
            state.stages[1].load(Ordering::Acquire),
            state.stages[2].load(Ordering::Acquire),
            state.stages[3].load(Ordering::Acquire),
        );
        return false;
    }
    true
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!(
        "[concurrent_fsync_test] start: rounds={}, workers={}, file_size_kb={}",
        ROUNDS,
        WORKERS,
        FILE_SIZE / 1024
    );

    let shmid = unsafe { raw_syscall(SYS_SHMGET, [0, PAGE_SIZE, IPC_CREAT | 0o600, 0, 0, 0]) };
    if shmid < 0 {
        println!("[concurrent_fsync_test] FAIL: shmget ret={}", shmid);
        return 1;
    }
    let addr = unsafe { raw_syscall(SYS_SHMAT, [shmid as usize, 0, 0, 0, 0, 0]) };
    if addr < 0 {
        println!("[concurrent_fsync_test] FAIL: shmat ret={}", addr);
        return 1;
    }
    if unsafe { raw_syscall(SYS_SHMCTL, [shmid as usize, IPC_RMID, 0, 0, 0, 0]) } < 0 {
        println!("[concurrent_fsync_test] FAIL: IPC_RMID");
        return 1;
    }
    let state = unsafe { &*(addr as *const SharedState) };

    for round in 0..ROUNDS {
        for (worker, path) in PATHS.iter().enumerate() {
            let _ = unlinkat(AT_FDCWD, path, 0);
            state.stages[worker].store(0, Ordering::Relaxed);
        }
        state.failures.store(0, Ordering::Relaxed);

        let mut pids = [-1isize; WORKERS];
        for worker in 0..WORKERS {
            let pid = fork();
            if pid == 0 {
                child_main(worker, round, state);
            }
            if pid < 0 {
                println!(
                    "[concurrent_fsync_test] FAIL: round={} fork child={} ret={}",
                    round + 1,
                    worker,
                    pid
                );
                return 2;
            }
            pids[worker] = pid;
        }

        for (worker, pid) in pids.into_iter().enumerate() {
            if !wait_child(pid, round, worker, state) {
                return 3;
            }
        }
        if state.failures.load(Ordering::Acquire) != 0 {
            println!(
                "[concurrent_fsync_test] FAIL: round={} barrier failures={}",
                round + 1,
                state.failures.load(Ordering::Relaxed)
            );
            return 4;
        }
        println!("[concurrent_fsync_test] round={} pass", round + 1);
    }

    for path in PATHS {
        let _ = unlinkat(AT_FDCWD, path, 0);
    }
    let detach = unsafe { raw_syscall(SYS_SHMDT, [addr as usize, 0, 0, 0, 0, 0]) };
    if detach < 0 {
        println!("[concurrent_fsync_test] FAIL: shmdt ret={}", detach);
        return 5;
    }

    println!("[concurrent_fsync_test] PASS");
    0
}
