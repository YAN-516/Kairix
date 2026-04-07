//! Memory management implementation
//!
//! SV39 page-based virtual-memory architecture for RV64 systems, and
//! everything about memory management, like frame allocator, page table,
//! map area and memory set, is implemented here.
//!
//! Every task or process has a memory_set to control its virtual memory.
// pub mod address;
pub mod frame_allocator;
use polyhal::{println,print};

pub mod heap_allocator;
//mod memory_set;
///
pub mod exception;
// mod page_table;
mod vm_area;
///
pub mod vm_set;
// pub use address::{PhysAddr, PhysPageNum, StepByOne, VirtAddr, VirtPageNum};
// use address::{VARange, VPNRange};
pub use polyhal::utils::addr::*;
pub use frame_allocator::{frame_alloc, frame_dealloc, frame_alloc_hal};
//pub use memory_set::remap_test;
//pub use memory_set::{KERNEL_SPACE, MemorySet, kernel_token};
#[cfg(target_arch = "loongarch64")]
use crate::sbi_la::get_tp;
#[cfg(target_arch = "riscv64")]
use crate::sbi::get_tp;
use crate::sync::mutex::*;
use alloc::vec::Vec;
// use page_table::PTEFlags;
// pub use page_table::{
//     PageTable, PageTableEntry, UserBuffer, UserBufferIterator, translated_byte_buffer,
//     translated_ref, translated_refmut, translated_str,
// };
pub use vm_area::*;
pub use vm_set::{KERNEL_VMSET, UserVMSet, VMSet, VMSpace, remap_test};
use alloc::string::String;
pub use heap_allocator::{heap_test, init_heap};

pub use polyhal::pagetable::*;

#[allow(missing_docs)]
pub unsafe fn sfence_vma_all() {
    unsafe {
        core::arch::asm!("sfence.vma");
    }
}
/// initiate heap allocator, frame allocator and kernel space
pub fn init() {

    println!("init Kernel_space");
    KERNEL_VMSET.exclusive_access().activate();
    let id = get_tp();
    println!("activate over, cpu {}", id);
}
#[allow(missing_docs)]
pub fn start_kvm() {
    KERNEL_VMSET.exclusive_access().activate();
    let id = get_tp();
    println!("activate over, cpu {}", id);
}

///Array of u8 slice that user communicate with os
pub struct UserBuffer {
    ///U8 vec
    pub buffers: Vec<&'static mut [u8]>,
}

impl UserBuffer {
    ///Create a `UserBuffer` by parameter
    pub fn new(buffers: Vec<&'static mut [u8]>) -> Self {
        Self { buffers }
    }
    ///Length of `UserBuffer`
    pub fn len(&self) -> usize {
        let mut total: usize = 0;
        for b in self.buffers.iter() {
            total += b.len();
        }
        total
    }
}

impl IntoIterator for UserBuffer {
    type Item = *mut u8;
    type IntoIter = UserBufferIterator;
    fn into_iter(self) -> Self::IntoIter {
        UserBufferIterator {
            buffers: self.buffers,
            current_buffer: 0,
            current_idx: 0,
        }
    }
}
/// Iterator of `UserBuffer`
pub struct UserBufferIterator {
    buffers: Vec<&'static mut [u8]>,
    current_buffer: usize,
    current_idx: usize,
}

impl Iterator for UserBufferIterator {
    type Item = *mut u8;
    fn next(&mut self) -> Option<Self::Item> {
        if self.current_buffer >= self.buffers.len() {
            None
        } else {
            let r = &mut self.buffers[self.current_buffer][self.current_idx] as *mut _;
            if self.current_idx + 1 == self.buffers[self.current_buffer].len() {
                self.current_idx = 0;
                self.current_buffer += 1;
            } else {
                self.current_idx += 1;
            }
            Some(r)
        }
    }
}

/// Translate a pointer to a mutable u8 Vec through page table
pub fn translated_byte_buffer(token: usize, ptr: *const u8, len: usize) -> Vec<&'static mut [u8]> {
    let page_table = PageTable::from_token(token);
    let mut start = ptr as usize;
    let end = start + len;
    let mut v = Vec::new();
    while start < end {
        let start_va = VirtAddr::from(start);
        let mut vpn = start_va.floor();
        let ppn = page_table.translate(vpn).unwrap().ppn();
        vpn.step();
        let mut end_va: VirtAddr = vpn.into();
        end_va = end_va.min(VirtAddr::from(end));
        if end_va.page_offset() == 0 {
            v.push(&mut ppn.get_bytes_array()[start_va.page_offset()..]);
        } else {
            v.push(&mut ppn.get_bytes_array()[start_va.page_offset()..end_va.page_offset()]);
        }
        start = end_va.into();
    }
    v
}

/// Translate a pointer to a mutable u8 Vec end with `\0` through page table to a `String`
pub fn translated_str(token: usize, ptr: *const u8) -> String {
    let page_table = PageTable::from_token(token);
    let mut string = String::new();
    let mut va = ptr as usize;
    loop {
        let ch: u8 = *(page_table
            .translate_va(VirtAddr::from(va))
            .unwrap()
            .get_mut());
        if ch == 0 {
            break;
        }
        string.push(ch as char);
        va += 1;
    }
    string
}

#[allow(unused)]
///Translate a generic through page table and return a reference
pub fn translated_ref<T>(token: usize, ptr: *const T) -> &'static T {
    let page_table = PageTable::from_token(token);
    page_table
        .translate_va(VirtAddr::from(ptr as usize))
        .unwrap()
        .get_ref()
}
///Translate a generic through page table and return a mutable reference
pub fn translated_refmut<T>(token: usize, ptr: *mut T) -> &'static mut T {
    let page_table = PageTable::from_token(token);
    let va = ptr as usize;
    page_table
        .translate_va(VirtAddr::from(va))
        .unwrap()
        .get_mut()
}

