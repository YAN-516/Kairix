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
use polyhal::common::FrameTracker;
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
pub mod reclaim;
///
pub mod vm_area;
///
pub mod vm_set;
use exception::SetPageFaultException;
pub use frame_allocator::frame_alloc_contiguous;
use vm_set::{AccessType, PageFaultError};
// pub use address::{PhysAddr, PhysPageNum, StepByOne, VirtAddr, VirtPageNum};
// use address::{VARange, VPNRange};
pub use frame_allocator::{
    frame_alloc, frame_alloc_hal, frame_dealloc, get_free_memory, get_total_memory,
    print_frame_stats,
};
pub use polyhal::utils::addr::*;
//pub use memory_set::remap_test;
//pub use memory_set::{KERNEL_SPACE, MemorySet, kernel_token};
use crate::error::{SysError, SysResult};
#[cfg(target_arch = "riscv64")]
use crate::sbi::get_tp;
#[cfg(target_arch = "loongarch64")]
use crate::sbi_la::get_tp;
use crate::sync::mutex::*;
use alloc::sync::Arc;
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

struct FileBackedFault {
    file: Arc<dyn crate::fs::File>,
    fault_vpn: VirtPageNum,
    file_offset: usize,
    page_id: usize,
    flags: MmapType,
}

fn fault_access_allowed(
    area: &UserMapArea,
    access: AccessType,
    allow_execute_as_read: bool,
) -> bool {
    match access {
        AccessType::Read => {
            area.perm().contains(MapPermission::R)
                || (allow_execute_as_read && area.perm().contains(MapPermission::X))
        }
        AccessType::Write => area.perm().contains(MapPermission::W) || area.cow_flag,
        AccessType::Execute => area.perm().contains(MapPermission::X),
        AccessType::None => false,
    }
}

fn file_backed_fault_snapshot(
    va: VirtAddr,
    access: AccessType,
    allow_execute_as_read: bool,
) -> Option<Option<FileBackedFault>> {
    let task = crate::task::current_task()?;
    let process = task.process.upgrade()?;
    let mut inner = process.inner_exclusive_access();
    let vm_set = &mut inner.vm_set;
    let fault_vpn = va.floor();

    if vm_set.translate(fault_vpn).is_some() {
        return None;
    }

    let area = vm_set.find_area(va)?;
    if area.areatype() != UserMapAreaType::Mmap || area.map_file.is_none() {
        return None;
    }
    if area.data_frames.contains_key(&fault_vpn) {
        return None;
    }
    if !fault_access_allowed(area, access, allow_execute_as_read) {
        return Some(None);
    }

    let offset_in_area = (fault_vpn.0 - area.start_vpn().0) * PageTable::PAGE_SIZE;
    let file_offset = area.file_offset + offset_in_area;
    Some(Some(FileBackedFault {
        file: area.map_file.as_ref().unwrap().clone(),
        fault_vpn,
        file_offset,
        page_id: file_offset / PageTable::PAGE_SIZE,
        flags: area.flags,
    }))
}

fn install_file_backed_fault_page(
    va: VirtAddr,
    fault: &FileBackedFault,
    frame: Arc<FrameTracker>,
    access: AccessType,
    allow_execute_as_read: bool,
) -> Option<PageFaultError> {
    let task = crate::task::current_task()?;
    let process = task.process.upgrade()?;
    let mut inner = process.inner_exclusive_access();
    let vm_set = &mut inner.vm_set;

    if vm_set.translate(fault.fault_vpn).is_some() {
        return Some(PageFaultError::Normal);
    }

    let (target_ppn, mut mapping_flags) = {
        let area = vm_set.find_area(va)?;
        if area.areatype() != UserMapAreaType::Mmap {
            return None;
        }
        let Some(current_file) = area.map_file.as_ref() else {
            return None;
        };
        if !Arc::ptr_eq(&fault.file, current_file) {
            return None;
        }
        if area.flags != fault.flags {
            return None;
        }
        let current_offset =
            area.file_offset + (fault.fault_vpn.0 - area.start_vpn().0) * PageTable::PAGE_SIZE;
        if current_offset != fault.file_offset {
            return None;
        }
        if !fault_access_allowed(area, access, allow_execute_as_read) {
            return None;
        }

        let mut new_private_cow_page = false;
        let target = match area.data_frames.get(&fault.fault_vpn) {
            Some(frame) => frame.clone(),
            None => {
                area.data_frames.insert(fault.fault_vpn, frame.clone());
                if area.data_frames.len() >= area.vpn_range().count() {
                    area.clear_lazy_flag();
                }
                new_private_cow_page = area.cow_flag && fault.flags == MmapType::MapPrivate;
                frame
            }
        };
        let mut flags = MappingFlags::from(*area.perm());
        if new_private_cow_page && matches!(access, AccessType::Write) {
            flags |= MappingFlags::W;
        }
        (target.ppn, flags)
    };

    if mapping_flags.contains(MappingFlags::X) && !mapping_flags.contains(MappingFlags::R) {
        mapping_flags |= MappingFlags::R;
    }
    vm_set.page_table.map_page(
        fault.fault_vpn,
        target_ppn,
        mapping_flags,
        MappingSize::Page4KB,
    );
    TLB::flush_vaddr(va);
    Some(PageFaultError::Normal)
}

#[allow(missing_docs)]
pub fn handle_file_backed_page_fault_current(
    va: VirtAddr,
    access: AccessType,
    allow_execute_as_read: bool,
) -> Option<Option<PageFaultError>> {
    let fault = match file_backed_fault_snapshot(va, access, allow_execute_as_read) {
        Some(Some(fault)) => fault,
        Some(None) => return Some(None),
        None => return None,
    };

    let file_size = fault
        .file
        .get_inode()
        .map(|inode| inode.get_size())
        .unwrap_or(0);
    if fault.file_offset >= file_size {
        return Some(Some(PageFaultError::BeyondFileSize));
    }

    let Some(file_frame) = fault.file.get_cache_frame(fault.page_id) else {
        return Some(Some(PageFaultError::InvalidMapping));
    };
    let frame = if fault.flags == MmapType::MapPrivate {
        let Some(private_frame) = frame_alloc().map(Arc::new) else {
            return Some(Some(PageFaultError::OutOfMemory));
        };
        let copy_size = (file_size - fault.file_offset).min(PageTable::PAGE_SIZE);
        private_frame.ppn.get_bytes_array()[..copy_size]
            .copy_from_slice(&file_frame.ppn.get_bytes_array()[..copy_size]);
        if copy_size < PageTable::PAGE_SIZE {
            private_frame.ppn.get_bytes_array()[copy_size..].fill(0);
        }
        private_frame
    } else {
        file_frame
    };

    Some(install_file_backed_fault_page(
        va,
        &fault,
        frame,
        access,
        allow_execute_as_read,
    ))
}

fn fault_current_user_page(va: VirtAddr, access: AccessType) -> Option<PageFaultError> {
    if let Some(result) = handle_file_backed_page_fault_current(va, access, false) {
        return result;
    }

    let task = crate::task::current_task()?;
    let process = task.process.upgrade()?;
    let mut inner = process.inner_exclusive_access();
    inner.vm_set.handle_store_page_fault_set(va, access)
}

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
    // let id = get_tp();
    // println!("activate over, cpu {}", id);
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
pub fn copy_to_user(token: usize, dst_va: *mut u8, src: &[u8]) -> SysResult<usize> {
    info!("copy to user {:#x}", dst_va as usize);
    let user_buffers = translated_byte_buffer_for_write(token, dst_va, src.len())?;
    let mut copied = 0usize;
    for user_buf in user_buffers {
        let copy_len = user_buf.len();
        user_buf.copy_from_slice(&src[copied..copied + copy_len]);
        copied += copy_len;
    }
    Ok(src.len())
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
pub fn translated_byte_buffer(
    token: usize,
    ptr: *const u8,
    len: usize,
) -> SysResult<Vec<&'static mut [u8]>> {
    translated_byte_buffer_inner(token, ptr, len, true, AccessType::Read)
}

/// Translate a user byte buffer that the kernel will write to.
pub fn translated_byte_buffer_for_write(
    token: usize,
    ptr: *mut u8,
    len: usize,
) -> SysResult<Vec<&'static mut [u8]>> {
    translated_byte_buffer_inner(token, ptr as *const u8, len, true, AccessType::Write)
}

/// Translate a user byte range only when it is contained in one mapped page.
/// Returns `Ok(None)` for cross-page buffers so callers can fall back to the
/// generic vector path without treating that as a user memory error.
pub fn translated_single_byte_buffer(
    token: usize,
    ptr: *const u8,
    len: usize,
) -> SysResult<Option<&'static mut [u8]>> {
    translated_single_byte_buffer_inner(token, ptr, len, AccessType::Read)
}

/// Translate a writable user byte range only when it is contained in one page.
pub fn translated_single_byte_buffer_for_write(
    token: usize,
    ptr: *mut u8,
    len: usize,
) -> SysResult<Option<&'static mut [u8]>> {
    translated_single_byte_buffer_inner(token, ptr as *const u8, len, AccessType::Write)
}

/// 与 `translated_byte_buffer` 类似，但当页面未映射时不会触发缺页处理（lazy allocation），
/// 而是直接返回错误。用于当前线程已不在处理器上、无法调用 `current_process()` 的场景。
pub fn translated_byte_buffer_no_fault(
    token: usize,
    ptr: *const u8,
    len: usize,
) -> SysResult<Vec<&'static mut [u8]>> {
    translated_byte_buffer_inner(token, ptr, len, false, AccessType::Read)
}

fn translated_byte_buffer_inner(
    token: usize,
    ptr: *const u8,
    len: usize,
    _do_fault: bool,
    access: AccessType,
) -> SysResult<Vec<&'static mut [u8]>> {
    let page_table = PageTable::from_token(token);
    let mut start = ptr as usize;
    let end = start + len;
    let mut v = Vec::new();
    while start < end {
        let start_va = VirtAddr::from(start);
        let mut vpn = start_va.floor();

        let pte_opt = page_table.translate(vpn);
        let page_accessible = pte_opt.map_or(false, |pte| match access {
            AccessType::Read => pte.readable(),
            AccessType::Write => pte.writable(),
            AccessType::Execute => pte.executable(),
            AccessType::None => false,
        });
        if !page_accessible {
            if _do_fault {
                if fault_current_user_page(start_va, access).is_none() {
                    return Err(SysError::EFAULT);
                }
            } else {
                // no_fault 模式：页面未映射时直接返回错误
                return Err(SysError::EFAULT);
            }
        }

        let Some(pte) = page_table.translate(vpn) else {
            return Err(SysError::EFAULT);
        };
        let page_accessible = match access {
            AccessType::Read => pte.readable(),
            AccessType::Write => pte.writable(),
            AccessType::Execute => pte.executable(),
            AccessType::None => false,
        };
        if !page_accessible {
            return Err(SysError::EFAULT);
        }
        let ppn = pte.ppn();
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
    Ok(v)
}

fn translated_single_byte_buffer_inner(
    token: usize,
    ptr: *const u8,
    len: usize,
    access: AccessType,
) -> SysResult<Option<&'static mut [u8]>> {
    if len == 0 {
        return Ok(None);
    }

    let start = ptr as usize;
    let end = start.checked_add(len).ok_or(SysError::EFAULT)?;
    let last = end.checked_sub(1).ok_or(SysError::EFAULT)?;
    let start_va = VirtAddr::from(start);
    let start_vpn = start_va.floor();
    if start_vpn != VirtAddr::from(last).floor() {
        return Ok(None);
    }

    let page_table = PageTable::from_token(token);
    let pte_opt = page_table.translate(start_vpn);
    let page_accessible = pte_opt.map_or(false, |pte| match access {
        AccessType::Read => pte.readable(),
        AccessType::Write => pte.writable(),
        AccessType::Execute => pte.executable(),
        AccessType::None => false,
    });
    if !page_accessible {
        if fault_current_user_page(start_va, access).is_none() {
            return Err(SysError::EFAULT);
        }
    }

    let Some(pte) = page_table.translate(start_vpn) else {
        return Err(SysError::EFAULT);
    };
    let page_accessible = match access {
        AccessType::Read => pte.readable(),
        AccessType::Write => pte.writable(),
        AccessType::Execute => pte.executable(),
        AccessType::None => false,
    };
    if !page_accessible {
        return Err(SysError::EFAULT);
    }

    let offset = start_va.page_offset();
    Ok(Some(&mut pte.ppn().get_bytes_array()[offset..offset + len]))
}

/// Translate a pointer to a mutable u8 Vec end with `\0` through page table to a `String`
pub fn translated_str(token: usize, ptr: *const u8) -> SysResult<String> {
    let page_table = PageTable::from_token(token);
    let mut string = String::new();
    let mut va = ptr as usize;
    loop {
        // 如果页面未映射，触发缺页处理（lazy 区域需要分配）
        let vpn = VirtAddr::from(va).floor();
        if page_table.translate(vpn).is_none() {
            if fault_current_user_page(VirtAddr::from(va), AccessType::Read).is_none() {
                return Err(SysError::EFAULT);
            }
        }
        let Some(pa) = page_table.translate_va(VirtAddr::from(va)) else {
            return Err(SysError::EFAULT);
        };
        let ch: u8 = *(pa.get_mut());
        if ch == 0 {
            break;
        }
        string.push(ch as char);
        va += 1;
    }
    Ok(string)
}

#[allow(unused)]
///Translate a generic through page table and return a reference
pub fn translated_ref<T>(token: usize, ptr: *const T) -> SysResult<&'static T> {
    let page_table = PageTable::from_token(token);
    let va = ptr as usize;
    // 检查页面是否映射且可读（防止访问 PROT_NONE 等不可读页面）
    let vpn = VirtAddr::from(va).floor();
    let pte_opt = page_table.translate(vpn);
    let page_readable = pte_opt.map_or(false, |pte| pte.readable());
    if !page_readable {
        if fault_current_user_page(VirtAddr::from(va), AccessType::Read).is_none() {
            return Err(SysError::EFAULT);
        }
    }
    let Some(pa) = page_table.translate_va(VirtAddr::from(va)) else {
        return Err(SysError::EFAULT);
    };
    Ok(pa.get_ref())
}
///Translate a generic through page table and return a mutable reference
pub fn translated_refmut<T>(token: usize, ptr: *mut T) -> SysResult<&'static mut T> {
    let page_table = PageTable::from_token(token);
    let va = ptr as usize;
    // 检查页面是否映射且可写（防止访问 PROT_NONE 等不可写页面）
    let vpn = VirtAddr::from(va).floor();
    let pte_opt = page_table.translate(vpn);
    let page_writable = pte_opt.map_or(false, |pte| pte.writable());
    if !page_writable {
        if fault_current_user_page(VirtAddr::from(va), AccessType::Write).is_none() {
            return Err(SysError::EFAULT);
        }
    }
    let Some(pa) = page_table.translate_va(VirtAddr::from(va)) else {
        return Err(SysError::EFAULT);
    };
    Ok(pa.get_mut())
}
