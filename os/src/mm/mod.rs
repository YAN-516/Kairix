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
use polyhal::print;
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
/// Swapfile-backed page reclaim support.
pub mod swap;
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
    frame_alloc, frame_alloc_hal, frame_dealloc, frame_dealloc_with_site, frame_stats,
    get_free_memory, get_total_memory, print_frame_stats, try_frame_stats,
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
//     translated_ref, translated_str, write_user_value,
// };
use alloc::string::String;
use core::mem::MaybeUninit;
use core::ops::Deref;
use core::sync::atomic::{AtomicUsize, Ordering};
pub use heap_allocator::{enable_heap_growth, heap_test, init_heap, print_heap_stats};
pub use vm_area::*;
pub(crate) use vm_set::activate_kernel_page_table;
pub use vm_set::{KERNEL_VMSET, UserVMSet, VMSet, VMSpace, remap_test};

pub use polyhal::pagetable::*;

// Executable faults are expected for every demand-paged code page. Keep a
// small startup sample and periodic checkpoints, but always retain a suspicious
// zero-filled cache page because it can indicate stale or corrupted backing.
static EXEC_FAULT_LOG_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Print the VMA and backing-page identity for a fatal user PC.
///
/// This is intentionally called only from fatal-trap diagnostics. It answers
/// whether an illegal instruction landed in file data or in the ELF zero/BSS
/// tail, and whether the PTE, VMA frame, and page-cache frame agree.
pub(crate) fn print_user_crash_vma(pc: usize) {
    let Some(task) = crate::task::current_task() else {
        error!("[USER_CRASH_VMA] pc={:#x} task=none", pc);
        return;
    };
    let pid = task.process_id();
    let Some(process) = task.process.upgrade() else {
        error!("[USER_CRASH_VMA] pid={} pc={:#x} process=none", pid, pc);
        return;
    };
    let executable_path = process
        .try_inner_exclusive_access()
        .map(|inner| inner.executable_path.clone());
    let Some(mut vm_set) = process.try_vm_exclusive_access() else {
        error!(
            "[USER_CRASH_VMA] pid={} pc={:#x} vm_lock=busy path={:?}",
            pid, pc, executable_path,
        );
        return;
    };
    let va = VirtAddr::from(pc);
    let vpn = va.floor();
    let pte_ppn = vm_set.translate(vpn).map(|pte| pte.ppn().0);
    let area = vm_set.find_area(va).map(|area| {
        let file_offset = area
            .file_offset
            .saturating_add((vpn.0.saturating_sub(area.start_vpn().0)) * PageTable::PAGE_SIZE);
        (
            area.areatype(),
            area.perm().bits(),
            area.start_va().0,
            area.end_va().0,
            area.file_zero_start,
            file_offset,
            area.data_frames.get(&vpn).map(|frame| frame.ppn.0),
            area.map_file.clone(),
        )
    });
    drop(vm_set);

    let mut cache_inode = None;
    let mut cache_page_id = None;
    let mut cache_ppn = None;
    let mut file_size = None;
    let mut elf_header_integrity = None;
    if let Some((_, _, _, _, _, file_offset, _, Some(file))) = area.as_ref() {
        if let Some(inode) = file.get_inode() {
            let inode_id = inode.cache_inode_id();
            cache_inode = inode_id;
            file_size = Some(inode.get_size());
            let page_id = *file_offset / PageTable::PAGE_SIZE;
            cache_page_id = Some(page_id);
            if let Some(cache_id) = inode_id {
                cache_ppn = crate::fs::page::pagecache::PAGE_CACHE
                    .get_page(cache_id, page_id)
                    .and_then(|page| page.try_read().and_then(|page| page.resident_frame()))
                    .map(|frame| frame.ppn.0);
                if let Some(header_frame) = crate::fs::page::pagecache::PAGE_CACHE
                    .get_page(cache_id, 0)
                    .and_then(|page| page.try_read().and_then(|page| page.resident_frame()))
                {
                    let header = header_frame.ppn.get_bytes_array();
                    if header.len() >= 64 && header[..4] == [0x7f, b'E', b'L', b'F'] {
                        let section_offset = u64::from_le_bytes(
                            header[0x28..0x30].try_into().expect("ELF64 e_shoff width"),
                        ) as usize;
                        let section_entry_size = u16::from_le_bytes(
                            header[0x3a..0x3c]
                                .try_into()
                                .expect("ELF64 e_shentsize width"),
                        ) as usize;
                        let section_count = u16::from_le_bytes(
                            header[0x3c..0x3e].try_into().expect("ELF64 e_shnum width"),
                        ) as usize;
                        let section_end = section_entry_size
                            .checked_mul(section_count)
                            .and_then(|size| section_offset.checked_add(size));
                        let truncated = section_end.is_some_and(|end| end > inode.get_size());
                        elf_header_integrity = Some((
                            header_frame.ppn.0,
                            section_offset,
                            section_entry_size,
                            section_count,
                            section_end,
                            truncated,
                        ));
                    }
                }
            }
        }
    }
    let (area_type, perm, start, end, zero_start, file_offset, area_ppn) = area
        .as_ref()
        .map(
            |(area_type, perm, start, end, zero_start, file_offset, area_ppn, _)| {
                (
                    Some(*area_type),
                    Some(*perm),
                    Some(*start),
                    Some(*end),
                    *zero_start,
                    Some(*file_offset),
                    *area_ppn,
                )
            },
        )
        .unwrap_or((None, None, None, None, None, None, None));
    let pc_in_zero_tail = zero_start.map(|zero| pc >= zero);
    error!(
        "[USER_CRASH_VMA] pid={} pc={:#x} vpn={:#x} path={:?} area={:?} perm={:?} range={:#x}..{:#x} file_offset={:?} file_size={:?} zero_start={:?} pc_in_zero_tail={:?} pte_ppn={:?} area_ppn={:?} cache_inode={:?} cache_page={:?} cache_ppn={:?}",
        pid,
        pc,
        vpn.0,
        executable_path,
        area_type,
        perm,
        start.unwrap_or(0),
        end.unwrap_or(0),
        file_offset,
        file_size,
        zero_start,
        pc_in_zero_tail,
        pte_ppn,
        area_ppn,
        cache_inode,
        cache_page_id,
        cache_ppn,
    );
    if let Some((
        header_ppn,
        section_offset,
        section_entry_size,
        section_count,
        section_end,
        truncated,
    )) = elf_header_integrity
    {
        error!(
            "[USER_CRASH_ELF_INTEGRITY] pid={} pc={:#x} path={:?} file_size={:?} pc_file_offset={:?} header_ppn={:#x} section_offset={:#x} section_entry_size={} section_count={} section_end={:?} truncated={}",
            pid,
            pc,
            executable_path,
            file_size,
            file_offset,
            header_ppn,
            section_offset,
            section_entry_size,
            section_count,
            section_end,
            truncated,
        );
    } else {
        error!(
            "[USER_CRASH_ELF_INTEGRITY] pid={} pc={:#x} path={:?} file_size={:?} pc_file_offset={:?} header_available=false",
            pid, pc, executable_path, file_size, file_offset,
        );
    }
}

struct FileBackedFault {
    file: Arc<dyn crate::fs::File>,
    fault_vpn: VirtPageNum,
    file_offset: usize,
    page_id: usize,
    flags: MmapType,
    area_type: UserMapAreaType,
    file_zero_start: Option<usize>,
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
    crate::trap::record_page_fault_phase(20);
    let mut vm_set = process.vm_exclusive_access();
    crate::trap::record_page_fault_phase(21);
    let fault_vpn = va.floor();

    if vm_set.translate(fault_vpn).is_some() {
        return None;
    }

    let area = vm_set.find_area(va)?;
    if !matches!(
        area.areatype(),
        UserMapAreaType::Mmap | UserMapAreaType::Elf
    ) || area.map_file.is_none()
    {
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
        area_type: area.areatype(),
        file_zero_start: area.file_zero_start,
    }))
}

fn install_file_backed_fault_page(
    va: VirtAddr,
    fault: &FileBackedFault,
    frame: Arc<FrameTracker>,
    private_write: bool,
    shared_write: bool,
    access: AccessType,
    allow_execute_as_read: bool,
) -> Option<PageFaultError> {
    let task = crate::task::current_task()?;
    let process = task.process.upgrade()?;
    crate::trap::record_page_fault_phase(24);
    let mut vm_set = process.vm_exclusive_access();
    crate::trap::record_page_fault_phase(25);
    let candidate_ppn = frame.ppn;

    if vm_set.translate(fault.fault_vpn).is_some() {
        return Some(PageFaultError::Normal);
    }

    let (target_ppn, mut mapping_flags) = {
        let area = vm_set.find_area(va)?;
        if !matches!(
            area.areatype(),
            UserMapAreaType::Mmap | UserMapAreaType::Elf
        ) {
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

        let (target, installed_candidate) = match area.data_frames.get(&fault.fault_vpn) {
            Some(frame) => (frame.clone(), false),
            None => {
                area.data_frames.insert(fault.fault_vpn, frame.clone());
                if area.data_frames.len() >= area.vpn_range().count() {
                    area.clear_lazy_flag();
                }
                (frame, true)
            }
        };
        let writable_private = private_write && installed_candidate;
        let flags = if shared_write {
            MappingFlags::from(*area.perm())
        } else if area.cow_flag && !writable_private {
            cow_mapping_flags(*area.perm())
        } else {
            area.initial_mapping_flags()
        };
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
    if mapping_flags.contains(MappingFlags::X) {
        crate::trap::record_page_fault_phase(26);
        polyhal::multicore::synchronize_instruction_cache(vm_set.token());
        crate::trap::record_page_fault_phase(27);
        if target_ppn != candidate_ppn {
            polyhal::println!(
                "[USER_EXEC_RACE] pid={} va={:#x} vpn={:#x} candidate_ppn={:#x} installed_ppn={:#x}",
                process.getpid(),
                va.0,
                fault.fault_vpn.0,
                candidate_ppn.0,
                target_ppn.0,
            );
        }
    }
    TLB::flush_vaddr(va);
    Some(PageFaultError::Normal)
}

fn handle_shared_file_write_fault_current(va: VirtAddr) -> Option<Option<PageFaultError>> {
    let task = crate::task::current_task()?;
    let process = task.process.upgrade()?;
    let (file, page_id, fault_vpn, token) = {
        let mut vm_set = process.vm_exclusive_access();
        let fault_vpn = va.floor();
        let pte = vm_set.translate(fault_vpn)?;
        let area = vm_set.find_area(va)?;
        if !area.tracks_shared_file_dirty() || !area.data_frames.contains_key(&fault_vpn) {
            return None;
        }
        if pte.writable() {
            // A stale local TLB entry can still fault after another thread has
            // completed the dirty transition for this address space.
            TLB::flush_vaddr(va);
            return Some(Some(PageFaultError::Normal));
        }
        let file = area.map_file.as_ref()?.clone();
        let offset_in_area = (fault_vpn.0 - area.start_vpn().0) * PageTable::PAGE_SIZE;
        let file_offset = area.file_offset.checked_add(offset_in_area)?;
        (
            file,
            file_offset / PageTable::PAGE_SIZE,
            fault_vpn,
            vm_set.token(),
        )
    };

    if let Err(err) = file.mark_cache_page_dirty(page_id) {
        warn!(
            "[MMAP_SHARED_DIRTY] failed: pid={} va={:#x} page={} err={:?}",
            task.process_id(),
            va.0,
            page_id,
            err
        );
        return Some(Some(PageFaultError::InvalidMapping));
    }

    let mut vm_set = process.vm_exclusive_access();
    let (ppn, flags) = {
        let area = vm_set.find_area(va)?;
        if !area.tracks_shared_file_dirty()
            || !area
                .map_file
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &file))
        {
            return None;
        }
        let current_offset = area
            .file_offset
            .checked_add((fault_vpn.0 - area.start_vpn().0) * PageTable::PAGE_SIZE)?;
        if current_offset / PageTable::PAGE_SIZE != page_id {
            return None;
        }
        let frame = area.data_frames.get(&fault_vpn)?;
        (
            frame.ppn,
            PTEFlags::from(MappingFlags::from(*area.perm())) | PTEFlags::V,
        )
    };
    let pte = vm_set.page_table.find_pte(fault_vpn)?;
    if pte.ppn() != ppn {
        return None;
    }
    *pte = PTE::new(ppn, flags);
    polyhal::multicore::shootdown_tlb_all(token);
    info!(
        "[MMAP_SHARED_DIRTY] pid={} va={:#x} page={}",
        task.process_id(),
        va.0,
        page_id
    );
    Some(Some(PageFaultError::Normal))
}

#[allow(missing_docs)]
pub fn handle_file_backed_page_fault_current(
    va: VirtAddr,
    access: AccessType,
    allow_execute_as_read: bool,
) -> Option<Option<PageFaultError>> {
    if matches!(access, AccessType::Write) {
        if let Some(result) = handle_shared_file_write_fault_current(va) {
            return Some(result);
        }
    }
    let fault = match file_backed_fault_snapshot(va, access, allow_execute_as_read) {
        Some(Some(fault)) => fault,
        Some(None) => return Some(None),
        None => return None,
    };

    let (file_size, inode_number, cache_inode_id) = fault
        .file
        .get_inode()
        .map(|inode| {
            (
                inode.get_size(),
                Some(inode.get_ino()),
                inode.cache_inode_id(),
            )
        })
        .unwrap_or((0, None, None));
    let page_start = fault.fault_vpn.0 * PageTable::PAGE_SIZE;
    let elf_zero_bytes = if fault.area_type == UserMapAreaType::Elf {
        fault.file_zero_start.map(|zero_start| {
            zero_start
                .saturating_sub(page_start)
                .min(PageTable::PAGE_SIZE)
        })
    } else {
        None
    };
    let private_write = fault.flags == MmapType::MapPrivate && matches!(access, AccessType::Write);
    let shared_write = fault.area_type == UserMapAreaType::Mmap
        && fault.flags == MmapType::MapShared
        && matches!(access, AccessType::Write);

    let frame = if elf_zero_bytes == Some(0) {
        let Some(zero_frame) = frame_alloc().map(Arc::new) else {
            return Some(Some(PageFaultError::OutOfMemory));
        };
        zero_frame.ppn.get_bytes_array().fill(0);
        crate::task::perf_stats::record_file_fault_zero_page();
        zero_frame
    } else {
        if fault.file_offset >= file_size {
            return Some(Some(PageFaultError::BeyondFileSize));
        }
        crate::trap::record_page_fault_phase(22);
        let Some(file_frame) = fault.file.get_cache_frame(fault.page_id) else {
            return Some(Some(PageFaultError::InvalidMapping));
        };
        crate::trap::record_page_fault_phase(23);
        if shared_write {
            if let Err(err) = fault.file.mark_cache_page_dirty(fault.page_id) {
                warn!(
                    "[MMAP_SHARED_DIRTY] initial fault failed: pid={} va={:#x} page={} err={:?}",
                    crate::task::current_task()
                        .map(|task| task.process_id())
                        .unwrap_or(0),
                    va.0,
                    fault.page_id,
                    err
                );
                return Some(Some(PageFaultError::InvalidMapping));
            }
        }
        let copy_size = elf_zero_bytes
            .unwrap_or_else(|| (file_size - fault.file_offset).min(PageTable::PAGE_SIZE));
        let needs_private_copy = private_write
            || (fault.area_type == UserMapAreaType::Elf && copy_size < PageTable::PAGE_SIZE);
        if needs_private_copy {
            let Some(private_frame) = frame_alloc().map(Arc::new) else {
                return Some(Some(PageFaultError::OutOfMemory));
            };
            private_frame.ppn.get_bytes_array()[..copy_size]
                .copy_from_slice(&file_frame.ppn.get_bytes_array()[..copy_size]);
            if copy_size < PageTable::PAGE_SIZE {
                private_frame.ppn.get_bytes_array()[copy_size..].fill(0);
            }
            crate::task::perf_stats::record_file_fault_private_copy();
            private_frame
        } else {
            crate::task::perf_stats::record_file_fault_shared_page();
            file_frame
        }
    };

    if matches!(access, AccessType::Execute) {
        let bytes = frame.ppn.get_bytes_array();
        let sample = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let sample_index = EXEC_FAULT_LOG_COUNT.fetch_add(1, Ordering::Relaxed);
        let suspicious_zero_cache = sample == 0 && elf_zero_bytes.is_none();
        if sample_index < 16 || sample_index % 512 == 0 || suspicious_zero_cache {
            error!(
                "[USER_EXEC_FAULT] seq={} pid={} va={:#x} vpn={:#x} inode={:?} cache_inode={:?} file_offset={:#x} page={} file_size={} zero_bytes={:?} frame_ppn={:#x} sample={:#010x} source={}",
                sample_index,
                crate::task::current_task()
                    .map(|task| task.process_id())
                    .unwrap_or(0),
                va.0,
                fault.fault_vpn.0,
                inode_number,
                cache_inode_id,
                fault.file_offset,
                fault.page_id,
                file_size,
                elf_zero_bytes,
                frame.ppn.0,
                sample,
                if elf_zero_bytes.is_some() {
                    "elf"
                } else {
                    "cache"
                },
            );
        }
    }

    Some(install_file_backed_fault_page(
        va,
        &fault,
        frame,
        private_write,
        shared_write,
        access,
        allow_execute_as_read,
    ))
}

fn fault_current_user_page(va: VirtAddr, access: AccessType) -> Option<PageFaultError> {
    if va.0 < polyhal::consts::USER_MEMORY_SPACE.0 || va.0 > polyhal::consts::USER_MEMORY_SPACE.1 {
        if let Some(task) = crate::task::current_task() {
            error!(
                "[USER_COPY_BAD_ADDRESS] pid={} syscall={:?} stage={} va={:#x} access={:?}",
                task.process_id(),
                task.active_syscall(),
                task.active_syscall_stage(),
                va.0,
                access,
            );
        } else {
            error!(
                "[USER_COPY_BAD_ADDRESS] pid=none syscall=none stage=0 va={:#x} access={:?}",
                va.0, access,
            );
        }
        return None;
    }

    if let Some(result) = handle_file_backed_page_fault_current(va, access, false) {
        return result;
    }

    let task = crate::task::current_task()?;
    let process = task.process.upgrade()?;
    let mut vm_set = process.vm_exclusive_access();
    vm_set.handle_store_page_fault_set(va, access)
}

#[allow(missing_docs)]
pub unsafe fn sfence_vma_all() {
    unsafe {
        core::arch::asm!("sfence.vma");
    }
}
/// initiate heap allocator, frame allocator and kernel space
pub fn init() {
    polyhal::println!("init Kernel_space");
    KERNEL_VMSET.lock().activate();
    // let id = get_tp();
    // println!("activate over, cpu {}", id);
}
#[allow(missing_docs)]
pub fn start_kvm() {
    KERNEL_VMSET.lock().activate();
    let id = get_tp();
    polyhal::println!("activate over, cpu {}", id);
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
    let end = start.checked_add(len).ok_or(SysError::EFAULT)?;
    validate_user_copy_range(start, end)?;
    let mut v = Vec::new();
    while start < end {
        let start_va = VirtAddr::from(start);
        let mut vpn = start_va.floor();
        let pte = resolve_user_pte(&page_table, token, start_va, end, _do_fault, access)?;
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

fn pte_allows_access(pte: PTE, access: AccessType) -> bool {
    if !pte.flags().plv_user() {
        return false;
    }
    match access {
        AccessType::Read => pte.readable(),
        AccessType::Write => pte.writable(),
        AccessType::Execute => pte.executable(),
        AccessType::None => false,
    }
}

fn validate_user_copy_range(start: usize, end: usize) -> SysResult<()> {
    if start == end {
        return Ok(());
    }
    if start < polyhal::consts::USER_MEMORY_SPACE.0
        || end <= start
        || end
            .checked_sub(1)
            .is_none_or(|last| last > polyhal::consts::USER_MEMORY_SPACE.1)
    {
        return Err(SysError::EFAULT);
    }
    Ok(())
}

fn log_user_buffer_fault(
    token: usize,
    start_va: VirtAddr,
    end: usize,
    access: AccessType,
    attempt: usize,
    fault: Option<&PageFaultError>,
    pte: Option<PTE>,
) {
    if !matches!(access, AccessType::Write) {
        return;
    }
    let Some(task) = crate::task::current_task() else {
        error!(
            "[USER_BUFFER_WRITE_FAULT] pid=none token={:#x} va={:#x} end={:#x} access={:?} attempt={} fault={:?} pte={:?}",
            token, start_va.0, end, access, attempt, fault, pte
        );
        return;
    };
    let Some(process) = task.process.upgrade() else {
        error!(
            "[USER_BUFFER_WRITE_FAULT] pid={} token={:#x} va={:#x} end={:#x} access={:?} attempt={} fault={:?} pte={:?} process=gone",
            task.process_id(),
            token,
            start_va.0,
            end,
            access,
            attempt,
            fault,
            pte
        );
        return;
    };
    let mut vm_set = process.vm_exclusive_access();
    if let Some(area) = vm_set.find_area(start_va) {
        error!(
            "[USER_BUFFER_WRITE_FAULT] pid={} syscall={:?} stage={} token={:#x} va={:#x} end={:#x} attempt={} fault={:?} pte={:?} vma=[{:#x},{:#x}) type={:?} perm={:#x} lazy={} cow={} resident_pages={}",
            task.process_id(),
            task.active_syscall(),
            task.active_syscall_stage(),
            token,
            start_va.0,
            end,
            attempt,
            fault,
            pte,
            area.start_va().0,
            area.end_va().0,
            area.areatype(),
            area.perm().bits(),
            area.get_lazy_flag(),
            area.cow_flag(),
            area.data_frames.len(),
        );
    } else {
        error!(
            "[USER_BUFFER_WRITE_FAULT] pid={} syscall={:?} stage={} token={:#x} va={:#x} end={:#x} attempt={} fault={:?} pte={:?} vma=none",
            task.process_id(),
            task.active_syscall(),
            task.active_syscall_stage(),
            token,
            start_va.0,
            end,
            attempt,
            fault,
            pte,
        );
    }
}

/// Resolve one user PTE for an in-kernel user copy.
///
/// A single Linux-visible access can legitimately require two software fault
/// transitions here: a lazy page may first be installed read-only because its
/// VMA is COW, then a write fault makes that particular page private. Hardware
/// retries the instruction after each transition; kernel user-copy paths must
/// do the same instead of turning the intermediate read-only PTE into EFAULT.
fn resolve_user_pte(
    page_table: &PageTable,
    token: usize,
    va: VirtAddr,
    end: usize,
    do_fault: bool,
    access: AccessType,
) -> SysResult<PTE> {
    const MAX_FAULT_TRANSITIONS: usize = 3;

    for attempt in 0..=MAX_FAULT_TRANSITIONS {
        if let Some(pte) = page_table.translate(va.floor()) {
            if pte_allows_access(pte, access) {
                return Ok(pte);
            }
        }
        if !do_fault {
            return Err(SysError::EFAULT);
        }
        if attempt == MAX_FAULT_TRANSITIONS {
            let pte = page_table.translate(va.floor());
            log_user_buffer_fault(token, va, end, access, attempt, None, pte);
            return Err(SysError::EFAULT);
        }

        match fault_current_user_page(va, access) {
            Some(PageFaultError::Normal) => {}
            Some(error) => {
                let pte = page_table.translate(va.floor());
                log_user_buffer_fault(token, va, end, access, attempt, Some(&error), pte);
                return Err(SysError::EFAULT);
            }
            None => {
                let pte = page_table.translate(va.floor());
                log_user_buffer_fault(token, va, end, access, attempt, None, pte);
                return Err(SysError::EFAULT);
            }
        }
    }
    Err(SysError::EFAULT)
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
    let pte = resolve_user_pte(&page_table, token, start_va, end, true, access)?;

    let offset = start_va.page_offset();
    Ok(Some(&mut pte.ppn().get_bytes_array()[offset..offset + len]))
}

/// Translate a pointer to a mutable u8 Vec end with `\0` through page table to a `String`
pub fn translated_str(token: usize, ptr: *const u8) -> SysResult<String> {
    let page_table = PageTable::from_token(token);
    let mut string = String::new();
    let mut va = ptr as usize;
    loop {
        let end = va.checked_add(1).ok_or(SysError::EFAULT)?;
        validate_user_copy_range(va, end)?;
        let user_va = VirtAddr::from(va);
        let pte = resolve_user_pte(&page_table, token, user_va, end, true, AccessType::Read)?;
        let ch = pte.ppn().get_bytes_array()[user_va.page_offset()];
        if ch == 0 {
            break;
        }
        string.push(ch as char);
        va = end;
    }
    Ok(string)
}

/// An aligned kernel copy of a fixed-size userspace value.
pub struct UserRef<T>(T);

impl<T> Deref for UserRef<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

fn copy_user_value<T: Copy>(buffers: &[&'static mut [u8]]) -> T {
    let mut value = MaybeUninit::<T>::uninit();
    let destination = unsafe {
        core::slice::from_raw_parts_mut(value.as_mut_ptr() as *mut u8, core::mem::size_of::<T>())
    };
    let mut copied = 0usize;
    for buffer in buffers {
        let len = buffer.len();
        destination[copied..copied + len].copy_from_slice(buffer);
        copied += len;
    }
    debug_assert_eq!(copied, destination.len());
    unsafe { value.assume_init() }
}

#[allow(unused)]
/// Copy a fixed-size value from userspace after validating every crossed page.
pub fn translated_ref<T: Copy>(token: usize, ptr: *const T) -> SysResult<UserRef<T>> {
    let buffers = translated_byte_buffer(token, ptr as *const u8, core::mem::size_of::<T>())?;
    Ok(UserRef(copy_user_value(&buffers)))
}

/// Copy a fixed-size kernel value to userspace, validating and faulting every
/// crossed destination page at the time of the write.
pub fn write_user_value<T: Copy>(token: usize, ptr: *mut T, value: &T) -> SysResult<()> {
    let bytes = unsafe {
        core::slice::from_raw_parts(value as *const T as *const u8, core::mem::size_of::<T>())
    };
    copy_to_user(token, ptr.cast(), bytes)?;
    Ok(())
}
