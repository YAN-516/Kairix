#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::arch::asm;
use core::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use user_lib::waitpid;

const PAGE_SIZE: usize = 4096;
const WORKERS: usize = 4;
const ROUNDS: usize = 32;
const CHILD_POLLS: usize = 256;
const CHILD_OK: i32 = 42;

const SYS_PSELECT6: usize = 72;
const SYS_EXIT_GROUP: usize = 94;
const SYS_GETPID: usize = 172;
const SYS_CLONE: usize = 220;
const SYS_SHMGET: usize = 194;
const SYS_SHMCTL: usize = 195;
const SYS_SHMAT: usize = 196;
const SYS_SHMDT: usize = 197;

const SIGCHLD: usize = 17;
const CLONE_PARENT_SETTID: usize = 0x0010_0000;
const CLONE_CHILD_CLEARTID: usize = 0x0020_0000;
const CLONE_CHILD_SETTID: usize = 0x0100_0000;
const GLIBC_FORK_FLAGS: usize = CLONE_CHILD_SETTID | CLONE_CHILD_CLEARTID | SIGCHLD;
const INVALID_TID_PTR: usize = usize::MAX - 0xfff;

const IPC_CREAT: usize = 0o1000;
const IPC_RMID: usize = 0;

#[repr(C)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

#[repr(C, align(64))]
struct SharedState {
    ready: [AtomicU32; WORKERS],
    go: AtomicU32,
    errors: AtomicU32,
    child_tid_seen: [AtomicI32; WORKERS],
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

fn glibc_style_fork(child_tid: &AtomicI32) -> isize {
    #[cfg(target_arch = "riscv64")]
    let args = [
        GLIBC_FORK_FLAGS,
        0,
        0,
        0,
        child_tid as *const AtomicI32 as usize,
        0,
    ];
    #[cfg(target_arch = "loongarch64")]
    let args = [
        GLIBC_FORK_FLAGS,
        0,
        0,
        child_tid as *const AtomicI32 as usize,
        0,
        0,
    ];
    unsafe { raw_syscall(SYS_CLONE, args) }
}

fn invalid_tid_pointer_fork() -> isize {
    let flags = CLONE_PARENT_SETTID | CLONE_CHILD_SETTID | SIGCHLD;
    #[cfg(target_arch = "riscv64")]
    let args = [flags, 0, INVALID_TID_PTR, 0, INVALID_TID_PTR, 0];
    #[cfg(target_arch = "loongarch64")]
    let args = [flags, 0, INVALID_TID_PTR, INVALID_TID_PTR, 0, 0];
    unsafe { raw_syscall(SYS_CLONE, args) }
}

fn getpid() -> isize {
    unsafe { raw_syscall(SYS_GETPID, [0; 6]) }
}

fn poll_one_us() -> isize {
    let mut timeout = Timespec {
        tv_sec: 0,
        tv_nsec: 1_000,
    };
    unsafe {
        raw_syscall(SYS_PSELECT6, [
            0,
            0,
            0,
            0,
            &mut timeout as *mut Timespec as usize,
            0,
        ])
    }
}

fn exit_group(code: i32) -> ! {
    unsafe {
        raw_syscall(SYS_EXIT_GROUP, [code as usize, 0, 0, 0, 0, 0]);
    }
    loop {
        core::hint::spin_loop();
    }
}

fn child_main(worker: usize, tag: u32, child_tid: &AtomicI32, state: &SharedState) -> ! {
    let pid = getpid();
    let seen_tid = child_tid.load(Ordering::Acquire);
    state.child_tid_seen[worker].store(seen_tid, Ordering::Release);
    if pid <= 0 || seen_tid != pid as i32 {
        println!(
            "[glibc_fork_select_test] child={} pid={} child_tid={} mismatch",
            worker, pid, seen_tid
        );
        state.errors.fetch_add(1, Ordering::Relaxed);
        exit_group(2);
    }

    state.ready[worker].store(tag, Ordering::Release);
    println!(
        "[glibc_fork_select_test] child={} pid={} ready child_tid={}",
        worker, pid, seen_tid
    );
    while state.go.load(Ordering::Acquire) != tag {
        let ret = poll_one_us();
        if ret != 0 && ret != -4 {
            state.errors.fetch_add(1, Ordering::Relaxed);
            exit_group(3);
        }
    }

    for _ in 0..CHILD_POLLS {
        let ret = poll_one_us();
        if ret != 0 && ret != -4 {
            state.errors.fetch_add(1, Ordering::Relaxed);
            exit_group(4);
        }
    }
    println!(
        "[glibc_fork_select_test] child={} pid={} polls done",
        worker, pid
    );
    exit_group(CHILD_OK);
}

fn wait_child(pid: isize, round: usize, worker: usize) -> bool {
    let mut status = 0i32;
    let waited = waitpid(pid as usize, &mut status);
    let exit_code = (status >> 8) & 0xff;
    if waited != pid || (status & 0x7f) != 0 || exit_code != CHILD_OK {
        println!(
            "[glibc_fork_select_test] FAIL: round={} child={} pid={} waited={} status={} exit={}",
            round + 1,
            worker,
            pid,
            waited,
            status,
            exit_code
        );
        return false;
    }
    true
}

fn wait_invalid_tid_child(pid: isize) -> bool {
    let mut status = 0i32;
    let waited = waitpid(pid as usize, &mut status);
    let exit_code = (status >> 8) & 0xff;
    if waited != pid || (status & 0x7f) != 0 || exit_code != CHILD_OK {
        println!(
            "[glibc_fork_select_test] FAIL: invalid tid child pid={} waited={} status={} exit={}",
            pid, waited, status, exit_code
        );
        return false;
    }
    true
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!(
        "[glibc_fork_select_test] start: rounds={}, workers={}, child_polls={}",
        ROUNDS, WORKERS, CHILD_POLLS
    );

    // Linux treats the parent/child TID stores as best-effort put_user calls.
    // Bad pointers must not roll back an otherwise successful clone.
    let invalid_tid_child = invalid_tid_pointer_fork();
    if invalid_tid_child == 0 {
        exit_group(CHILD_OK);
    }
    if invalid_tid_child < 0 {
        println!(
            "[glibc_fork_select_test] FAIL: invalid tid pointer clone ret={}",
            invalid_tid_child
        );
        return 1;
    }
    if !wait_invalid_tid_child(invalid_tid_child) {
        return 2;
    }
    println!(
        "[glibc_fork_select_test] invalid tid pointer clone pid={} pass",
        invalid_tid_child
    );

    let shmid = unsafe { raw_syscall(SYS_SHMGET, [0, PAGE_SIZE, IPC_CREAT | 0o600, 0, 0, 0]) };
    if shmid < 0 {
        println!("[glibc_fork_select_test] FAIL: shmget ret={}", shmid);
        return 1;
    }
    let addr = unsafe { raw_syscall(SYS_SHMAT, [shmid as usize, 0, 0, 0, 0, 0]) };
    if addr < 0 {
        println!("[glibc_fork_select_test] FAIL: shmat ret={}", addr);
        return 2;
    }
    if unsafe { raw_syscall(SYS_SHMCTL, [shmid as usize, IPC_RMID, 0, 0, 0, 0]) } < 0 {
        println!("[glibc_fork_select_test] FAIL: IPC_RMID");
        return 3;
    }
    let state = unsafe { &*(addr as *const SharedState) };

    for round in 0..ROUNDS {
        let tag = (round + 1) as u32;
        state.go.store(0, Ordering::Relaxed);
        state.errors.store(0, Ordering::Relaxed);
        for worker in 0..WORKERS {
            state.ready[worker].store(0, Ordering::Relaxed);
            state.child_tid_seen[worker].store(-1, Ordering::Relaxed);
        }

        let child_tids = [const { AtomicI32::new(-1) }; WORKERS];
        let mut pids = [-1isize; WORKERS];
        for worker in 0..WORKERS {
            let pid = glibc_style_fork(&child_tids[worker]);
            if pid == 0 {
                child_main(worker, tag, &child_tids[worker], state);
            }
            if pid < 0 {
                println!(
                    "[glibc_fork_select_test] FAIL: round={} clone child={} ret={}",
                    round + 1,
                    worker,
                    pid
                );
                return 4;
            }
            if child_tids[worker].load(Ordering::Acquire) != -1 {
                println!(
                    "[glibc_fork_select_test] FAIL: round={} parent child_tid[{}]={}",
                    round + 1,
                    worker,
                    child_tids[worker].load(Ordering::Relaxed)
                );
                return 5;
            }
            pids[worker] = pid;
            println!(
                "[glibc_fork_select_test] round={} parent cloned child={} pid={}",
                round + 1,
                worker,
                pid
            );
        }

        let mut parent_polls = 0usize;
        while state
            .ready
            .iter()
            .any(|ready| ready.load(Ordering::Acquire) != tag)
        {
            let ret = poll_one_us();
            if ret != 0 && ret != -4 {
                println!(
                    "[glibc_fork_select_test] FAIL: round={} parent pselect ret={}",
                    round + 1,
                    ret
                );
                return 6;
            }
            parent_polls += 1;
        }
        println!(
            "[glibc_fork_select_test] round={} children ready parent_polls={} tids=[{}, {}, {}, {}]",
            round + 1,
            parent_polls,
            state.child_tid_seen[0].load(Ordering::Acquire),
            state.child_tid_seen[1].load(Ordering::Acquire),
            state.child_tid_seen[2].load(Ordering::Acquire),
            state.child_tid_seen[3].load(Ordering::Acquire),
        );
        state.go.store(tag, Ordering::Release);

        for (worker, pid) in pids.into_iter().enumerate() {
            if !wait_child(pid, round, worker) {
                return 7;
            }
        }
        if state.errors.load(Ordering::Acquire) != 0 {
            println!(
                "[glibc_fork_select_test] FAIL: round={} shared errors={}",
                round + 1,
                state.errors.load(Ordering::Relaxed)
            );
            return 8;
        }
        println!("[glibc_fork_select_test] round={} pass", round + 1);
    }

    let detach = unsafe { raw_syscall(SYS_SHMDT, [addr as usize, 0, 0, 0, 0, 0]) };
    if detach < 0 {
        println!("[glibc_fork_select_test] FAIL: shmdt ret={}", detach);
        return 9;
    }
    println!("[glibc_fork_select_test] PASS");
    0
}
