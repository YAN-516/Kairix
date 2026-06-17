//! The global allocator
// use crate::config::KERNEL_HEAP_SIZE;
use polyhal::consts::KERNEL_HEAP_SIZE;

use buddy_system_allocator::LockedHeap;
use core::ptr::addr_of_mut;
use log::*;
use log::*;
use polyhal::{print, println};

/// 打印当前内核堆的使用统计信息（user / actual / total）
pub fn print_heap_stats() {
    let heap = HEAP_ALLOCATOR.lock();
    let user = heap.stats_alloc_user();
    let actual = heap.stats_alloc_actual();
    let total = heap.stats_total_bytes();
    debug!(
        "[MEMDEBUG] heap: user={} actual={} total={} free={}",
        user,
        actual,
        total,
        total.saturating_sub(actual)
    );
}

#[global_allocator]
/// heap allocator instance
static HEAP_ALLOCATOR: LockedHeap<32> = LockedHeap::empty();

#[alloc_error_handler]
/// panic when heap allocation error occurs
pub fn handle_alloc_error(layout: core::alloc::Layout) -> ! {
    print_heap_stats();
    panic!("Heap allocation error, layout = {:?}", layout);
}
/// heap space ([u8; KERNEL_HEAP_SIZE])
static mut HEAP_SPACE: [u8; KERNEL_HEAP_SIZE] = [0; KERNEL_HEAP_SIZE];
/// initiate heap allocator
pub fn init_heap() {
    unsafe {
        HEAP_ALLOCATOR
            .lock()
            .init(addr_of_mut!(HEAP_SPACE) as usize, KERNEL_HEAP_SIZE);
    }
}

#[allow(unused)]
#[allow(missing_docs)]
pub fn heap_test() {
    use alloc::boxed::Box;
    use alloc::vec::Vec;
    unsafe extern "C" {
        safe fn sbss();
        safe fn ebss();
    }
    let bss_range = sbss as usize..ebss as usize;
    let a = Box::new(5);
    assert_eq!(*a, 5);
    assert!(bss_range.contains(&(a.as_ref() as *const _ as usize)));
    drop(a);
    let mut v: Vec<usize> = Vec::new();
    for i in 0..500 {
        v.push(i);
    }
    for (i, val) in v.iter().take(500).enumerate() {
        assert_eq!(*val, i);
    }
    assert!(bss_range.contains(&(v.as_ptr() as usize)));
    drop(v);
    println!("heap_test passed!");
}
