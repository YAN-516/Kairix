#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::hint::black_box;
use core::slice;
use user_lib::{close, exit, fork, pipe, read, waitpid, write, yield_};

const WORKERS: usize = 8;
const ROUNDS: usize = 120_000;
const CHILD_OK: i32 = 42;

#[inline(never)]
fn fp_work(seed: usize, allow_yield: bool) -> u64 {
    let seed = seed as f64 + 1.0;
    let mut a = 0.125 + seed * 0.000_001;
    let mut b = 0.250 + seed * 0.000_002;
    let mut c = 0.375 + seed * 0.000_003;
    let mut d = 0.500 + seed * 0.000_004;
    let mut e = 0.625 + seed * 0.000_005;
    let mut f = 0.750 + seed * 0.000_006;
    let mut g = 0.875 + seed * 0.000_007;
    let mut h = 1.000 + seed * 0.000_008;

    for round in 0..ROUNDS {
        // Keep several independent floating-point values live so timer
        // preemption and explicit yields exercise both caller- and
        // callee-saved FP registers.
        a = (a + h) * 0.500_000_1;
        b = (b + a) * 0.499_999_9;
        c = (c + b) * 0.500_000_3;
        d = (d + c) * 0.499_999_7;
        e = (e + d) * 0.500_000_5;
        f = (f + e) * 0.499_999_5;
        g = (g + f) * 0.500_000_7;
        h = (h + g) * 0.499_999_3;

        if round & 0x3f == 0 {
            black_box((a, b, c, d, e, f, g, h));
            if allow_yield {
                yield_();
            }
        }
    }

    a.to_bits()
        ^ b.to_bits().rotate_left(7)
        ^ c.to_bits().rotate_left(13)
        ^ d.to_bits().rotate_left(19)
        ^ e.to_bits().rotate_left(29)
        ^ f.to_bits().rotate_left(37)
        ^ g.to_bits().rotate_left(43)
        ^ h.to_bits().rotate_left(53)
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

fn child_main(worker: usize, result_pipe: [i32; 2]) -> ! {
    let _ = close(result_pipe[0] as usize);
    let result = [worker as u64, fp_work(worker, true)];
    let bytes = unsafe {
        slice::from_raw_parts(
            result.as_ptr() as *const u8,
            core::mem::size_of_val(&result),
        )
    };
    if !write_exact(result_pipe[1] as usize, bytes) {
        exit(2);
    }
    let _ = close(result_pipe[1] as usize);
    exit(CHILD_OK);
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!(
        "[fp_context_test] start: workers={}, rounds={}",
        WORKERS, ROUNDS
    );

    let mut expected = [0u64; WORKERS];
    for (worker, checksum) in expected.iter_mut().enumerate() {
        *checksum = fp_work(worker, false);
    }

    let mut result_pipe = [-1i32; 2];
    if pipe(&mut result_pipe) < 0 {
        println!("[fp_context_test] FAIL: pipe creation failed");
        return 1;
    }

    let mut pids = [-1isize; WORKERS];
    for worker in 0..WORKERS {
        let pid = fork();
        if pid == 0 {
            child_main(worker, result_pipe);
        }
        if pid < 0 {
            println!("[fp_context_test] FAIL: fork worker={} ret={}", worker, pid);
            return 2;
        }
        pids[worker] = pid;
    }
    let _ = close(result_pipe[1] as usize);

    let mut seen = [false; WORKERS];
    for _ in 0..WORKERS {
        let mut result = [0u64; 2];
        let bytes = unsafe {
            slice::from_raw_parts_mut(
                result.as_mut_ptr() as *mut u8,
                core::mem::size_of_val(&result),
            )
        };
        if !read_exact(result_pipe[0] as usize, bytes) {
            println!("[fp_context_test] FAIL: short result read");
            return 3;
        }
        let worker = result[0] as usize;
        if worker >= WORKERS || seen[worker] || result[1] != expected[worker] {
            let expected_value = expected.get(worker).copied().unwrap_or(0);
            println!(
                "[fp_context_test] FAIL: worker={} got={:#x} expected={:#x}",
                worker, result[1], expected_value
            );
            return 4;
        }
        seen[worker] = true;
    }
    let _ = close(result_pipe[0] as usize);

    for pid in pids {
        let mut status = 0i32;
        let waited = waitpid(pid as usize, &mut status);
        let exit_code = (status >> 8) & 0xff;
        if waited != pid || (status & 0x7f) != 0 || exit_code != CHILD_OK {
            println!(
                "[fp_context_test] FAIL: pid={} waited={} status={} exit={}",
                pid, waited, status, exit_code
            );
            return 5;
        }
    }

    println!("[fp_context_test] PASS");
    0
}
