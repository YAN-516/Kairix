#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::ptr::{read_volatile, write_volatile};
use user_lib::{close, exit, fork, mmap, munmap, pipe, read, waitpid, write, yield_};

const PAGE_SIZE: usize = 4096;
const PROT_READ: usize = 0x1;
const PROT_WRITE: usize = 0x2;
const MAP_PRIVATE: usize = 0x02;
const MAP_ANONYMOUS: usize = 0x20;

const PAGES: usize = 96;
const MAP_LEN: usize = PAGES * PAGE_SIZE;
const BATCH: usize = 8;
const ROUNDS: usize = 128;
const CHILD_WRITES: usize = 24;
const REMAP_ROUNDS: usize = 128;
const REMAP_PAGES: usize = 12;
const ZOMBIE_BURST: usize = 512;
const CHILD_OK: i32 = 17;
const REMAP_CHILD_OK: i32 = 23;
const ZOMBIE_CHILD_OK: i32 = 31;

fn parent_pattern(page: usize) -> u8 {
    (page as u8).wrapping_mul(37).wrapping_add(11)
}

fn child_pattern(round: usize, slot: usize, write_idx: usize, old: u8) -> u8 {
    old ^ 0xa5 ^ (round as u8).wrapping_mul(3) ^ (slot as u8) ^ (write_idx as u8)
}

fn read_page_byte(base: usize, page: usize) -> u8 {
    unsafe { read_volatile((base + page * PAGE_SIZE) as *const u8) }
}

fn write_page_byte(base: usize, page: usize, value: u8) {
    unsafe {
        write_volatile((base + page * PAGE_SIZE) as *mut u8, value);
    }
}

fn fill_parent_mapping(base: usize) {
    for page in 0..PAGES {
        write_page_byte(base, page, parent_pattern(page));
    }
}

fn verify_parent_mapping(base: usize, round: usize) -> bool {
    for page in 0..PAGES {
        let expected = parent_pattern(page);
        let got = read_page_byte(base, page);
        if got != expected {
            println!(
                "[fork_cow_pressure] COW FAIL after round {}: page {}, expected {}, got {}",
                round, page, expected, got
            );
            return false;
        }
    }
    true
}

fn child_work(base: usize, round: usize, slot: usize, read_fd: i32, write_fd: i32) -> ! {
    let _ = close(read_fd as usize);
    for write_idx in 0..CHILD_WRITES {
        let page = (round * 13 + slot * 7 + write_idx * 5) % PAGES;
        let old = read_page_byte(base, page);
        write_page_byte(base, page, child_pattern(round, slot, write_idx, old));
        let _ = read_page_byte(base, page);
        if write_idx % 4 == 0 {
            let _ = yield_();
        }
    }

    let token = [((round ^ slot) & 0xff) as u8];
    let wrote = write(write_fd as usize, &token);
    let _ = close(write_fd as usize);
    if wrote != 1 {
        println!(
            "[fork_cow_pressure] child pipe write failed: round {}, slot {}, ret {}",
            round, slot, wrote
        );
        exit(2);
    }
    exit(CHILD_OK);
}

fn wait_for_child(pid: isize, expected_code: i32, label: &str) -> bool {
    let mut status = 0;
    let waited = waitpid(pid as usize, &mut status);
    let exited = (status & 0x7f) == 0;
    let exit_code = (status >> 8) & 0xff;
    if waited != pid || !exited || exit_code != expected_code {
        println!(
            "[fork_cow_pressure] {} wait failed: pid {}, waited {}, status {}, decoded exit {}",
            label, pid, waited, status, exit_code
        );
        return false;
    }
    true
}

fn run_fork_cow_pipe_pressure(base: usize) -> i32 {
    let mut total_children = 0usize;

    for round in 0..ROUNDS {
        let mut pids = [-1isize; BATCH];
        let mut read_fds = [-1i32; BATCH];
        let mut created = 0usize;

        for slot in 0..BATCH {
            let mut fds = [-1i32; 2];
            let pipe_ret = pipe(&mut fds);
            if pipe_ret < 0 {
                println!(
                    "[fork_cow_pressure] pipe failed: round {}, slot {}, ret {}",
                    round, slot, pipe_ret
                );
                return 1;
            }

            let pid = fork();
            if pid == 0 {
                child_work(base, round, slot, fds[0], fds[1]);
            }
            if pid < 0 {
                let _ = close(fds[0] as usize);
                let _ = close(fds[1] as usize);
                println!(
                    "[fork_cow_pressure] fork failed: round {}, slot {}, ret {}",
                    round, slot, pid
                );
                return 1;
            }

            let _ = close(fds[1] as usize);
            pids[slot] = pid;
            read_fds[slot] = fds[0];
            created += 1;
        }

        for slot in 0..created {
            let mut token = [0u8; 1];
            let nread = read(read_fds[slot] as usize, &mut token);
            let _ = close(read_fds[slot] as usize);
            let expected = ((round ^ slot) & 0xff) as u8;
            if nread != 1 || token[0] != expected {
                println!(
                    "[fork_cow_pressure] pipe read failed: round {}, slot {}, ret {}, byte {}, expected {}",
                    round, slot, nread, token[0], expected
                );
                return 1;
            }
        }

        for slot in 0..created {
            if !wait_for_child(pids[slot], CHILD_OK, "fork/cow/pipe") {
                return 1;
            }
            total_children += 1;
        }

        if !verify_parent_mapping(base, round) {
            return 1;
        }

        if round % 16 == 0 {
            println!(
                "[fork_cow_pressure] round {} ok, children {}",
                round, total_children
            );
        }
    }

    0
}

fn remap_child(base: usize, round: usize) -> ! {
    for page in 0..REMAP_PAGES {
        let value = (round as u8).wrapping_add((page as u8).wrapping_mul(19));
        write_page_byte(base, page, value);
    }
    exit(REMAP_CHILD_OK);
}

fn run_mmap_recycle_pressure() -> i32 {
    const REMAP_LEN: usize = REMAP_PAGES * PAGE_SIZE;

    for round in 0..REMAP_ROUNDS {
        let addr = mmap(
            0,
            REMAP_LEN,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS,
            -1,
            0,
        );
        if addr < 0 {
            println!(
                "[fork_cow_pressure] remap mmap failed: round {}, ret {}",
                round, addr
            );
            return 1;
        }
        let base = addr as usize;

        for page in 0..REMAP_PAGES {
            write_page_byte(base, page, parent_pattern(page));
        }

        let pid = fork();
        if pid == 0 {
            remap_child(base, round);
        }
        if pid < 0 {
            let _ = munmap(base, REMAP_LEN);
            println!(
                "[fork_cow_pressure] remap fork failed: round {}, ret {}",
                round, pid
            );
            return 1;
        }
        if !wait_for_child(pid, REMAP_CHILD_OK, "remap") {
            let _ = munmap(base, REMAP_LEN);
            return 1;
        }

        for page in 0..REMAP_PAGES {
            let expected = parent_pattern(page);
            let got = read_page_byte(base, page);
            if got != expected {
                println!(
                    "[fork_cow_pressure] remap COW FAIL: round {}, page {}, expected {}, got {}",
                    round, page, expected, got
                );
                let _ = munmap(base, REMAP_LEN);
                return 1;
            }
        }

        let unmap_ret = munmap(base, REMAP_LEN);
        if unmap_ret < 0 {
            println!(
                "[fork_cow_pressure] munmap failed: round {}, ret {}",
                round, unmap_ret
            );
            return 1;
        }

        if round % 32 == 0 {
            println!("[fork_cow_pressure] remap round {} ok", round);
        }
    }

    0
}

fn run_zombie_stack_pressure() -> i32 {
    let mut pids = [-1isize; ZOMBIE_BURST];
    let mut created = 0usize;

    for slot in 0..ZOMBIE_BURST {
        let pid = fork();
        if pid == 0 {
            let _ = yield_();
            exit(ZOMBIE_CHILD_OK);
        }
        if pid < 0 {
            println!(
                "[fork_cow_pressure] zombie burst fork failed: slot {}, ret {}",
                slot, pid
            );
            break;
        }
        pids[slot] = pid;
        created += 1;
    }

    for _ in 0..64 {
        let _ = yield_();
    }

    for slot in 0..created {
        if !wait_for_child(pids[slot], ZOMBIE_CHILD_OK, "zombie burst") {
            return 1;
        }
    }

    if created != ZOMBIE_BURST {
        return 1;
    }

    println!(
        "[fork_cow_pressure] zombie burst ok, children {}",
        ZOMBIE_BURST
    );
    0
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[fork_cow_pressure] start");

    let addr = mmap(
        0,
        MAP_LEN,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0,
    );
    if addr < 0 {
        println!("[fork_cow_pressure] mmap failed: ret {}", addr);
        return 1;
    }
    let base = addr as usize;
    fill_parent_mapping(base);

    let fork_result = run_fork_cow_pipe_pressure(base);
    let unmap_ret = munmap(base, MAP_LEN);
    if unmap_ret < 0 {
        println!("[fork_cow_pressure] final munmap failed: ret {}", unmap_ret);
        return 1;
    }
    if fork_result != 0 {
        return fork_result;
    }

    let zombie_result = run_zombie_stack_pressure();
    if zombie_result != 0 {
        return zombie_result;
    }

    let remap_result = run_mmap_recycle_pressure();
    if remap_result != 0 {
        return remap_result;
    }

    println!(
        "[fork_cow_pressure] PASS: {} fork children + {} zombie burst children + {} remap forks",
        ROUNDS * BATCH,
        ZOMBIE_BURST,
        REMAP_ROUNDS
    );
    0
}
