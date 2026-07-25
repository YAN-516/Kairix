#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    IoVec, Rlimit64, SignalSet, close, getpid, getrandom, kill, pipe, prlimit64, read, readv,
    signalfd4, sigprocmask, write,
};

const SIG_BLOCK: i32 = 0;
const SIGUSR1: i32 = 10;
const SFD_NONBLOCK: i32 = 0o4000;

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[syscall_abi_regression_test] start");

    let mut unsupported = Rlimit64::default();
    if prlimit64(0, 3, None, Some(&mut unsupported)) != -22 {
        println!("[syscall_abi_regression_test] FAIL prlimit fake success");
        return 1;
    }
    let mut nofile = Rlimit64::default();
    if prlimit64(0, 7, None, Some(&mut nofile)) != 0 || nofile.rlim_cur > nofile.rlim_max {
        println!("[syscall_abi_regression_test] FAIL prlimit supported");
        return 2;
    }

    let mut fds = [-1i32; 2];
    if pipe(&mut fds) != 0 || write(fds[1] as usize, b"x") != 1 {
        println!("[syscall_abi_regression_test] FAIL pipe setup");
        return 3;
    }
    let mut first = [0xa5u8; 4];
    let mut second = [0x5au8; 4];
    let iov = [
        IoVec {
            base: first.as_mut_ptr(),
            len: first.len(),
        },
        IoVec {
            base: second.as_mut_ptr(),
            len: second.len(),
        },
    ];
    let read_count = readv(fds[0] as usize, &iov);
    let _ = close(fds[0] as usize);
    let _ = close(fds[1] as usize);
    if read_count != 1 || first[0] != b'x' || second != [0x5a; 4] || readv(usize::MAX, &iov) != -9 {
        println!("[syscall_abi_regression_test] FAIL readv short/EBADF");
        return 4;
    }

    let mut random = [0u8; 32];
    if getrandom(&mut random, 0) != random.len() as isize
        || random.iter().all(|byte| *byte == 0)
        || getrandom(&mut random, 0x8000_0000) != -22
    {
        println!("[syscall_abi_regression_test] FAIL getrandom");
        return 5;
    }

    let mut mask = SignalSet::empty();
    mask.add(SIGUSR1);
    if sigprocmask(SIG_BLOCK, Some(&mask), None) != 0 {
        println!("[syscall_abi_regression_test] FAIL block signal");
        return 6;
    }
    let signal_fd = signalfd4(-1, &mask, SFD_NONBLOCK);
    if signal_fd < 0 || kill(getpid(), SIGUSR1 as usize) != 0 {
        println!("[syscall_abi_regression_test] FAIL signalfd setup");
        return 7;
    }
    let mut info = [0u8; 128];
    let got = read(signal_fd as usize, &mut info);
    let signo = u32::from_ne_bytes([info[0], info[1], info[2], info[3]]);
    let drained = read(signal_fd as usize, &mut info);
    let _ = close(signal_fd as usize);
    if got != 128 || signo != SIGUSR1 as u32 || drained != -11 {
        println!(
            "[syscall_abi_regression_test] FAIL signalfd got={} signo={} drained={}",
            got, signo, drained
        );
        return 8;
    }

    println!("[syscall_abi_regression_test] PASS");
    0
}
