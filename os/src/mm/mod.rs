//! Memory management implementation
//!
//! SV39 page-based virtual-memory architecture for RV64 systems, and
//! everything about memory management, like frame allocator, page table,
//! map area and memory set, is implemented here.
//!
//! Every task or process has a memory_set to control its virtual memory.
// pub mod address;
pub mod frame_allocator;
use log::*;
use polyhal::{print, println};
///
pub mod heap;
pub mod heap_allocator;
//mod memory_set;
///
pub mod exception;
// pub mod page_table;
// mod page_table;
///
pub mod vm_area;
///
pub mod vm_set;
use vm_set::AccessType;
use exception::SetPageFaultException;
// pub use address::{PhysAddr, PhysPageNum, StepByOne, VirtAddr, VirtPageNum};
// use address::{VARange, VPNRange};
pub use frame_allocator::{
    frame_alloc, frame_alloc_hal, frame_dealloc, get_free_memory, get_total_memory, print_frame_stats,
};
pub use polyhal::utils::addr::*;
//pub use memory_set::remap_test;
//pub use memory_set::{KERNEL_SPACE, MemorySet, kernel_token};
#[cfg(target_arch = "riscv64")]
use crate::sbi::get_tp;
#[cfg(target_arch = "loongarch64")]
use crate::sbi_la::get_tp;
use crate::sync::mutex::*;
use alloc::vec::Vec;
// use page_table::PTEFlags;
// pub use page_table::{
//     PageTable, PageTableEntry, UserBuffer, UserBufferIterator, translated_byte_buffer,
//     translated_ref, translated_refmut, translated_str,
// };
use alloc::string::String;
pub use heap_allocator::{heap_test, init_heap, print_heap_stats};
pub use vm_area::*;
pub use vm_set::{KERNEL_VMSET, UserVMSet, VMSet, VMSpace, remap_test};

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
    KERNEL_VMSET.lock().activate();
    let id = get_tp();
    println!("activate over, cpu {}", id);
}
#[allow(missing_docs)]
pub fn start_kvm() {
    KERNEL_VMSET.lock().activate();
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
///
pub fn copy_to_user(_token: usize, dst_va: *const u8, src: &[u8]) -> usize {
    info!("copy to user {:#x}", dst_va as usize);
    unsafe {
        core::ptr::copy_nonoverlapping(src.as_ptr(), dst_va as *mut u8, src.len());
    }
    // let user_buffers = translated_byte_buffer(token, dst_va, src.len());
    // let mut current_src = src;
    // for user_buf in user_buffers.into_iter() {
    //     let copy_len = user_buf.len();
    //     user_buf.copy_from_slice(&current_src[..copy_len]);
    //     current_src = &current_src[copy_len..];
    // }
    src.len()
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
    translated_byte_buffer_inner(token, ptr, len, true)
}

/// 与 `translated_byte_buffer` 类似，但当页面未映射时不会触发缺页处理（lazy allocation），
/// 而是直接返回空 Vec。用于当前线程已不在处理器上、无法调用 `current_process()` 的场景。
pub fn translated_byte_buffer_no_fault(token: usize, ptr: *const u8, len: usize) -> Vec<&'static mut [u8]> {
    translated_byte_buffer_inner(token, ptr, len, false)
}

fn translated_byte_buffer_inner(token: usize, ptr: *const u8, len: usize, _do_fault: bool) -> Vec<&'static mut [u8]> {
    let page_table = PageTable::from_token(token);
    let mut start = ptr as usize;
    let end = start + len;
    let mut v = Vec::new();
    while start < end {
        let start_va = VirtAddr::from(start);
        let mut vpn = start_va.floor();

        // 如果页面未映射，尝试触发缺页处理（lazy 区域需要分配）
        if page_table.translate(vpn).is_none() {
            if _do_fault {
                if let Some(task) = crate::task::current_task() {
                    if let Some(process) = task.process.upgrade() {
                        let mut inner = process.inner_exclusive_access();
                        if inner.vm_set.handle_store_page_fault_set(start_va, AccessType::Write).is_none() {
                            panic!(
                                "translated_byte_buffer: page fault handler failed for va {:#x}",
                                start_va.0
                            );
                        }
                    } else {
                        // 进程已被回收，无法分配页面，返回空 Vec
                        return Vec::new();
                    }
                } else {
                    // 无当前任务，返回空 Vec
                    return Vec::new();
                }
            } else {
                // no_fault 模式：页面未映射时直接返回空 Vec
                return Vec::new();
            }
        }

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
        // 如果页面未映射，触发缺页处理（lazy 区域需要分配）
        let vpn = VirtAddr::from(va).floor();
        if page_table.translate(vpn).is_none() {
            if let Some(task) = crate::task::current_task() {
                if let Some(process) = task.process.upgrade() {
                    let mut inner = process.inner_exclusive_access();
                    if inner.vm_set.handle_store_page_fault_set(VirtAddr::from(va), AccessType::Read).is_none() {
                        panic!(
                            "translated_str: page fault handler failed for va {:#x}",
                            va
                        );
                    }
                } else {
                    return String::new();
                }
            } else {
                return String::new();
            }
        }
        let Some(pa) = page_table.translate_va(VirtAddr::from(va)) else {
            return String::new();
        };
        let ch: u8 = *(pa.get_mut());
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
    let va = ptr as usize;
    // 如果页面未映射，触发缺页处理（lazy 区域需要分配）
    let vpn = VirtAddr::from(va).floor();
    if page_table.translate(vpn).is_none() {
        if let Some(task) = crate::task::current_task() {
            if let Some(process) = task.process.upgrade() {
                let mut inner = process.inner_exclusive_access();
                if inner.vm_set.handle_store_page_fault_set(VirtAddr::from(va), AccessType::Read).is_none() {
                    panic!(
                        "translated_ref: page fault handler failed for va {:#x}",
                        va
                    );
                }
            } else {
                return unsafe { core::ptr::NonNull::<T>::dangling().as_ref() };
            }
        } else {
            return unsafe { core::ptr::NonNull::<T>::dangling().as_ref() };
        }
    }
    let Some(pa) = page_table.translate_va(VirtAddr::from(va)) else {
        return unsafe { core::ptr::NonNull::<T>::dangling().as_ref() };
    };
    pa.get_ref()
}
///Translate a generic through page table and return a mutable reference
pub fn translated_refmut<T>(token: usize, ptr: *mut T) -> &'static mut T {
    let page_table = PageTable::from_token(token);
    let va = ptr as usize;
    // 如果页面未映射，触发缺页处理（lazy 区域需要分配）
    let vpn = VirtAddr::from(va).floor();
    if page_table.translate(vpn).is_none() {
        if let Some(task) = crate::task::current_task() {
            if let Some(process) = task.process.upgrade() {
                let mut inner = process.inner_exclusive_access();
                if inner.vm_set.handle_store_page_fault_set(VirtAddr::from(va), AccessType::Write).is_none() {
                    panic!(
                        "translated_refmut: page fault handler failed for va {:#x}",
                        va
                    );
                }
            } else {
                return unsafe { core::ptr::NonNull::<T>::dangling().as_mut() };
            }
        } else {
            return unsafe { core::ptr::NonNull::<T>::dangling().as_mut() };
        }
    }
    let Some(pa) = page_table.translate_va(VirtAddr::from(va)) else {
        return unsafe { core::ptr::NonNull::<T>::dangling().as_mut() };
    };
    pa.get_mut()
}
