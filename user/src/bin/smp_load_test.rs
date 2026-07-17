#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::hint::black_box;
use core::slice;
use user_lib::{close, exit, fork, get_time, pipe, read, waitpid, write, yield_};

const WORKER_COUNT: usize = 8;
const WORK_ITERATIONS: usize = 20_000_000;
const CHILD_OK: i32 = 0;

#[inline(never)]
fn cpu_work(worker: usize) -> usize {
    let mut value = 0x9e37_79b9_7f4a_7c15usize ^ worker.wrapping_mul(0x100_0000_01b3usize);

    for iteration in 0..WORK_ITERATIONS {
        value = value
            .wrapping_add(iteration ^ worker)
            .rotate_left(((iteration + worker) & 31) as u32);
        value ^= value >> 7;
        value = value.wrapping_mul(0x100_0000_01b3usize);

        if iteration & 0x3fff == 0 {
            black_box(value);
        }
    }

    black_box(value)
}

fn write_exact(fd: usize, mut bytes: &[u8]) -> bool {
    while !bytes.is_empty() {
        let written = write(fd, bytes);
        if written == -4 {
            continue;
        }
        if written <= 0 {
            return false;
        }
        bytes = &bytes[written as usize..];
    }
    true
}

fn read_exact(fd: usize, mut bytes: &mut [u8]) -> bool {
    while !bytes.is_empty() {
        let read_len = read(fd, bytes);
        if read_len == -4 {
            continue;
        }
        if read_len <= 0 {
            return false;
        }
        let (_, remaining) = bytes.split_at_mut(read_len as usize);
        bytes = remaining;
    }
    true
}

fn write_worker_result(fd: usize, worker: usize, checksum: usize) -> bool {
    let result = [worker, checksum];
    let bytes = unsafe {
        slice::from_raw_parts(
            result.as_ptr() as *const u8,
            core::mem::size_of_val(&result),
        )
    };
    write_exact(fd, bytes)
}

fn read_worker_result(fd: usize) -> Option<(usize, usize)> {
    let mut result = [0usize; 2];
    let bytes = unsafe {
        slice::from_raw_parts_mut(
            result.as_mut_ptr() as *mut u8,
            core::mem::size_of_val(&result),
        )
    };
    read_exact(fd, bytes).then_some((result[0], result[1]))
}

fn child_main(
    worker: usize,
    ready_pipe: [i32; 2],
    start_pipe: [i32; 2],
    result_pipe: [i32; 2],
) -> ! {
    let _ = close(ready_pipe[0] as usize);
    let _ = close(start_pipe[1] as usize);
    let _ = close(result_pipe[0] as usize);

    let ready = [1u8];
    if !write_exact(ready_pipe[1] as usize, &ready) {
        exit(2);
    }
    let _ = close(ready_pipe[1] as usize);

    let mut start = [0u8; 1];
    if !read_exact(start_pipe[0] as usize, &mut start) {
        exit(3);
    }
    let _ = close(start_pipe[0] as usize);

    let checksum = cpu_work(worker);
    if !write_worker_result(result_pipe[1] as usize, worker, checksum) {
        exit(4);
    }
    let _ = close(result_pipe[1] as usize);
    exit(CHILD_OK);
}

fn elapsed_ms(start: isize, end: isize) -> Option<usize> {
    if start < 0 || end < start {
        None
    } else {
        Some((end - start) as usize)
    }
}

fn wait_child(pid: isize) -> bool {
    let mut status = 0i32;
    let waited = waitpid(pid as usize, &mut status);
    let exited = (status & 0x7f) == 0;
    let exit_code = (status >> 8) & 0xff;
    if waited != pid || !exited || exit_code != CHILD_OK {
        println!(
            "[smp_load_test] wait failed: pid={}, waited={}, status={}, exit_code={}",
            pid, waited, status, exit_code
        );
        false
    } else {
        true
    }
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!(
        "[smp_load_test] start: workers={}, iterations_per_worker={}",
        WORKER_COUNT, WORK_ITERATIONS
    );

    let mut expected = [0usize; WORKER_COUNT];
    let serial_start = get_time();
    for (worker, checksum) in expected.iter_mut().enumerate() {
        *checksum = cpu_work(worker);
    }
    let serial_end = get_time();
    let serial_ms = elapsed_ms(serial_start, serial_end);
    println!("[smp_load_test] serial_ms={:?}", serial_ms);

    let mut ready_pipe = [-1i32; 2];
    let mut start_pipe = [-1i32; 2];
    let mut result_pipe = [-1i32; 2];
    if pipe(&mut ready_pipe) < 0 || pipe(&mut start_pipe) < 0 || pipe(&mut result_pipe) < 0 {
        println!("[smp_load_test] FAIL: pipe creation failed");
        return 1;
    }

    let mut pids = [-1isize; WORKER_COUNT];
    let mut created = 0usize;
    for worker in 0..WORKER_COUNT {
        let pid = fork();
        if pid == 0 {
            child_main(worker, ready_pipe, start_pipe, result_pipe);
        }
        if pid < 0 {
            println!(
                "[smp_load_test] fork failed: worker={}, ret={}",
                worker, pid
            );
            break;
        }
        pids[worker] = pid;
        created += 1;
    }

    let _ = close(ready_pipe[1] as usize);
    let _ = close(start_pipe[0] as usize);
    let _ = close(result_pipe[1] as usize);

    let mut ready = [0u8; WORKER_COUNT];
    let ready_ok = read_exact(ready_pipe[0] as usize, &mut ready[..created]);
    let _ = close(ready_pipe[0] as usize);
    println!(
        "[smp_load_test] barrier: created={}, ready_ok={}",
        created, ready_ok
    );

    let parallel_start = get_time();
    let start_tokens = [1u8; WORKER_COUNT];
    let start_ok = write_exact(start_pipe[1] as usize, &start_tokens[..created]);
    let _ = close(start_pipe[1] as usize);

    let mut seen = [false; WORKER_COUNT];
    let mut results_ok = true;
    for _ in 0..created {
        match read_worker_result(result_pipe[0] as usize) {
            Some((worker, checksum)) if worker < WORKER_COUNT && !seen[worker] => {
                seen[worker] = true;
                if checksum != expected[worker] {
                    println!(
                        "[smp_load_test] checksum mismatch: worker={}, expected={:#x}, got={:#x}",
                        worker, expected[worker], checksum
                    );
                    results_ok = false;
                }
            }
            Some((worker, checksum)) => {
                println!(
                    "[smp_load_test] invalid result: worker={}, checksum={:#x}",
                    worker, checksum
                );
                results_ok = false;
            }
            None => {
                println!("[smp_load_test] result pipe closed early");
                results_ok = false;
                break;
            }
        }
    }
    let parallel_end = get_time();
    let _ = close(result_pipe[0] as usize);

    let mut waits_ok = true;
    for pid in pids.iter().take(created) {
        waits_ok &= wait_child(*pid);
    }

    let parallel_ms = elapsed_ms(parallel_start, parallel_end);
    println!("[smp_load_test] parallel_ms={:?}", parallel_ms);
    if let (Some(serial_ms), Some(parallel_ms)) = (serial_ms, parallel_ms) {
        let speedup_x100 = serial_ms.saturating_mul(100) / parallel_ms.max(1);
        println!(
            "[smp_load_test] speedup_x100={} (100 means no speedup)",
            speedup_x100
        );
        if speedup_x100 >= 150 {
            println!("[smp_load_test] parallelism_observed=YES");
        } else {
            println!("[smp_load_test] parallelism_observed=NO");
        }
    } else {
        println!("[smp_load_test] timing unavailable");
    }

    let all_seen = seen.iter().take(created).all(|value| *value);
    if created == WORKER_COUNT && ready_ok && start_ok && results_ok && all_seen && waits_ok {
        println!("[smp_load_test] PASS");
        0
    } else {
        println!(
            "[smp_load_test] FAIL: created={}, ready_ok={}, start_ok={}, results_ok={}, all_seen={}, waits_ok={}",
            created, ready_ok, start_ok, results_ok, all_seen, waits_ok
        );
        for _ in 0..WORKER_COUNT {
            let _ = yield_();
        }
        1
    }
}
