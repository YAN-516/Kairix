#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::ptr::{read_volatile, write_volatile};
use user_lib::{mmap, mprotect, munmap};

const PAGE_SIZE: usize = 4096;
const TLB_PAIR_SIZE: usize = PAGE_SIZE * 2;
const MAP_LEN: usize = PAGE_SIZE * 4;
const PROT_NONE: usize = 0;
const PROT_READ: usize = 1;
const PROT_WRITE: usize = 2;
const MAP_PRIVATE: usize = 2;
const MAP_ANONYMOUS: usize = 0x20;

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[lazy_tlb_pair_test] start");

    // glibc reserves malloc arenas with PROT_NONE and enables the committed
    // prefix with mprotect. Keep every PTE lazy so the first reads below must
    // exercise the kernel's page-fault and TLB invalidation path.
    let mapping = mmap(0, MAP_LEN, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if mapping < 0 {
        println!("[lazy_tlb_pair_test] FAIL mmap ret={}", mapping);
        return 1;
    }

    let base = mapping as usize;
    let protect = mprotect(base, MAP_LEN, PROT_READ | PROT_WRITE);
    if protect != 0 {
        let cleanup = munmap(base, MAP_LEN);
        println!(
            "[lazy_tlb_pair_test] FAIL mprotect ret={} cleanup={}",
            protect, cleanup
        );
        return 2;
    }

    // Select a complete absolute 8 KiB pair inside the mapping. Accessing its
    // even half first installs a TLB entry whose odd half is still invalid.
    // The odd-half load then reproduces the libc-bench LA livelock if INVTLB
    // is incorrectly issued with VA[12] still set.
    let pair_base = (base + TLB_PAIR_SIZE - 1) & !(TLB_PAIR_SIZE - 1);
    if pair_base + TLB_PAIR_SIZE > base + MAP_LEN {
        let cleanup = munmap(base, MAP_LEN);
        println!(
            "[lazy_tlb_pair_test] FAIL no full pair base={:#x} pair={:#x} cleanup={}",
            base, pair_base, cleanup
        );
        return 3;
    }

    let even = (pair_base + 0xb8) as *mut u8;
    let odd = (pair_base + PAGE_SIZE + 0xb8) as *mut u8;
    let (even_initial, odd_initial, even_after, odd_after) = unsafe {
        let even_initial = read_volatile(even);
        let odd_initial = read_volatile(odd);
        write_volatile(even, 0x5a);
        write_volatile(odd, 0xa5);
        (
            even_initial,
            odd_initial,
            read_volatile(even),
            read_volatile(odd),
        )
    };

    let cleanup = munmap(base, MAP_LEN);
    if even_initial != 0
        || odd_initial != 0
        || even_after != 0x5a
        || odd_after != 0xa5
        || cleanup != 0
    {
        println!(
            "[lazy_tlb_pair_test] FAIL values={:#x}/{:#x}->{:#x}/{:#x} cleanup={}",
            even_initial, odd_initial, even_after, odd_after, cleanup
        );
        return 4;
    }

    println!(
        "[lazy_tlb_pair_test] PASS base={:#x} pair={:#x}",
        base, pair_base
    );
    0
}
