use crate::error::{SysError, SyscallResult};
use crate::fs::File;
use crate::mm::exception::SetPageFaultException;
use crate::task::current_task;
use fatfs::warn;
// use crate::config::PAGE_SIZE;
use crate::fs::page::pagecache::PAGE_CACHE;
use crate::fs::tmpfs::inode::F_SEAL_WRITE;
use crate::mm::frame_alloc;
use crate::mm::vm_area::MapArea;
use crate::mm::vm_area::{LazyAlloc, cow_mapping_flags};
use crate::mm::vm_set::VMSpace;
use crate::mm::{COW, MapPermission, MmapType, UserMapAreaType, UserVMSet};
use crate::mm::{UserMapArea, vm_set};
use crate::syscall::shm::release_shm_attaches;
use crate::task::current_process;
use crate::task::perf_stats::{PerfTimerKind, scope_timer};
use crate::vm_set::AccessType;
use alloc::sync::Arc;
use alloc::vec::Vec;
use log::info;
use log::log;
use polyhal::common::FrameTracker;
use polyhal::consts::{PAGE_SIZE, USER_MEMORY_SPACE};
use polyhal::pagetable::*;
use polyhal::utils::addr::{VPNRange, VirtAddr, VirtPageNum};

fn area_needs_mprotect_cow(area: &UserMapArea, new_perm: MapPermission) -> bool {
    if !new_perm.contains(MapPermission::W) {
        return false;
    }
    if area.areatype() == UserMapAreaType::Shm {
        return false;
    }
    if area.areatype() == UserMapAreaType::Mmap && area.flags == MmapType::MapShared {
        return false;
    }
    area.data_frames
        .values()
        .any(|frame| Arc::strong_count(frame) > 1)
}

fn area_pte_flags(area: &UserMapArea) -> PTEFlags {
    let mapping_flags = if area.cow_flag() && area.perm().contains(MapPermission::W) {
        cow_mapping_flags(*area.perm())
    } else {
        area.initial_mapping_flags()
    };
    PTEFlags::from(mapping_flags) | PTEFlags::V
}

fn mprotect_area_state_needs_update(area: &UserMapArea, new_perm: MapPermission) -> bool {
    if area.perm().bits() != new_perm.bits() {
        return true;
    }
    if !new_perm.contains(MapPermission::W) {
        return area.cow_flag();
    }
    !area.cow_flag() && area_needs_mprotect_cow(area, new_perm)
}

fn apply_mprotect_area_state(area: &mut UserMapArea, new_perm: MapPermission) {
    *area.perm_mut() = new_perm;
    if !new_perm.contains(MapPermission::W) {
        area.clear_cow_flag();
    } else if area_needs_mprotect_cow(area, new_perm) {
        area.set_cow_flag();
    }
}

/// Split one overlapping VMA by moving, rather than cloning, resident frames.
fn split_mprotect_area(
    mut area: UserMapArea,
    overlap_start: VirtPageNum,
    overlap_end: VirtPageNum,
    new_perm: MapPermission,
) -> (Option<UserMapArea>, UserMapArea, Option<UserMapArea>) {
    debug_assert!(area.start_vpn() <= overlap_start);
    debug_assert!(overlap_start < overlap_end);
    debug_assert!(overlap_end <= area.end_vpn());

    let original_start = area.start_vpn();
    let original_end = area.end_vpn();
    let overlap_start_va = VirtAddr::from(overlap_start.0 * PAGE_SIZE);
    let overlap_end_va = VirtAddr::from(overlap_end.0 * PAGE_SIZE);

    let mut left = (original_start < overlap_start).then(|| {
        let mut left = UserMapArea::metadata_from_another(&area);
        left.range_va_mut().end = overlap_start_va;
        left
    });
    let mut right = (overlap_end < original_end).then(|| {
        let mut right = UserMapArea::metadata_from_another(&area);
        right.trim_start(overlap_end_va);
        right
    });

    let mut middle_frames = area.data_frames.split_off(&overlap_start);
    let right_frames = middle_frames.split_off(&overlap_end);
    if let Some(left) = &mut left {
        left.data_frames = core::mem::take(&mut area.data_frames);
    }
    if let Some(right) = &mut right {
        right.data_frames = right_frames;
    }

    area.trim_start(overlap_start_va);
    area.range_va_mut().end = overlap_end_va;
    area.data_frames = middle_frames;
    apply_mprotect_area_state(&mut area, new_perm);
    (left, area, right)
}

fn same_file_backing(left: &UserMapArea, right: &UserMapArea) -> bool {
    match (&left.map_file, &right.map_file) {
        (None, None) => true,
        (Some(left_file), Some(right_file)) => {
            Arc::ptr_eq(left_file, right_file)
                && left
                    .file_offset
                    .checked_add(left.end_va().0 - left.start_va().0)
                    == Some(right.file_offset)
        }
        _ => false,
    }
}

fn same_shared_anonymous_backing(left: &UserMapArea, right: &UserMapArea) -> bool {
    match (&left.shared_anonymous, &right.shared_anonymous) {
        (None, None) => true,
        (Some(left_backing), Some(right_backing)) => {
            Arc::ptr_eq(left_backing, right_backing)
                && left
                    .shared_anonymous_offset
                    .checked_add(left.end_vpn().0 - left.start_vpn().0)
                    == Some(right.shared_anonymous_offset)
        }
        _ => false,
    }
}

fn mprotect_area_metadata_mergeable(left: &UserMapArea, right: &UserMapArea) -> bool {
    // SysV SHM attachment boundaries are observed by shmdt; do not merge two
    // independently attached segments even if their visible attributes match.
    left.areatype() == UserMapAreaType::Mmap
        && right.areatype() == UserMapAreaType::Mmap
        && left.end_va() == right.start_va()
        && left.map_type == right.map_type
        && left.lazy_flag == right.lazy_flag
        && left.growdown_flag == right.growdown_flag
        && left.flags == right.flags
        && left.shmid == right.shmid
        && left.file_zero_start == right.file_zero_start
        && left.mapping_path.as_deref() == right.mapping_path.as_deref()
        && same_file_backing(left, right)
        && same_shared_anonymous_backing(left, right)
}

fn mprotect_areas_mergeable_with_state(
    left: &UserMapArea,
    right: &UserMapArea,
    right_perm: MapPermission,
    right_cow: bool,
) -> bool {
    mprotect_area_metadata_mergeable(left, right)
        && left.perm().bits() == right_perm.bits()
        && left.cow_flag() == right_cow
}

fn mprotect_areas_state_mergeable(left: &UserMapArea, right: &UserMapArea) -> bool {
    mprotect_areas_mergeable_with_state(left, right, *right.perm(), right.cow_flag())
}

fn merge_mprotect_areas(
    areas: &mut Vec<UserMapArea>,
    mut index: usize,
    end_vpn: VirtPageNum,
) -> usize {
    let mut merged = 0;
    while index + 1 < areas.len() && areas[index].start_vpn() <= end_vpn {
        if !mprotect_areas_state_mergeable(&areas[index], &areas[index + 1]) {
            index += 1;
            continue;
        }
        let mut right = areas.remove(index + 1);
        areas[index].range_va_mut().end = right.end_va();
        areas[index].data_frames.append(&mut right.data_frames);
        merged += 1;
    }
    merged
}

fn mprotect_range_needs_cow(
    area: &UserMapArea,
    start_vpn: VirtPageNum,
    end_vpn: VirtPageNum,
    new_perm: MapPermission,
) -> bool {
    if !new_perm.contains(MapPermission::W)
        || area.areatype() == UserMapAreaType::Shm
        || (area.areatype() == UserMapAreaType::Mmap && area.flags == MmapType::MapShared)
    {
        return false;
    }
    area.data_frames
        .range(start_vpn..end_vpn)
        .any(|(_, frame)| Arc::strong_count(frame) > 1)
}

/// Extend an already compatible left VMA across a changed prefix without
/// constructing a temporary middle VMA and immediately merging it again.
fn try_expand_mprotect_prefix(
    areas: &mut [UserMapArea],
    index: usize,
    overlap_start: VirtPageNum,
    overlap_end: VirtPageNum,
    new_perm: MapPermission,
) -> bool {
    if index == 0
        || overlap_start != areas[index].start_vpn()
        || overlap_end >= areas[index].end_vpn()
    {
        return false;
    }

    let prefix_cow = mprotect_range_needs_cow(&areas[index], overlap_start, overlap_end, new_perm);
    if !mprotect_areas_mergeable_with_state(&areas[index - 1], &areas[index], new_perm, prefix_cow)
    {
        return false;
    }

    let new_boundary = VirtAddr::from(overlap_end.0 * PAGE_SIZE);
    let (left_areas, right_areas) = areas.split_at_mut(index);
    let left = &mut left_areas[index - 1];
    let right = &mut right_areas[0];
    let right_frames = right.data_frames.split_off(&overlap_end);
    let mut prefix_frames = core::mem::replace(&mut right.data_frames, right_frames);
    left.data_frames.append(&mut prefix_frames);
    left.range_va_mut().end = new_boundary;
    right.trim_start(new_boundary);
    true
}

const MPROTECT_LOCAL_TLB_PAGE_THRESHOLD: usize = 32;
const MPROTECT_GAP_SLOW_NS: usize = 5_000_000;

fn flush_mprotect_local_range(start_vpn: VirtPageNum, end_vpn: VirtPageNum) -> &'static str {
    let page_count = end_vpn.0.saturating_sub(start_vpn.0);
    if page_count > MPROTECT_LOCAL_TLB_PAGE_THRESHOLD {
        TLB::flush_all();
        return "local_all";
    }

    #[cfg(target_arch = "riscv64")]
    for vpn in VPNRange::new(start_vpn, end_vpn) {
        TLB::flush_vaddr(VirtAddr::from(vpn.0 * PAGE_SIZE));
    }

    #[cfg(target_arch = "loongarch64")]
    {
        // One LoongArch base-page TLB entry covers an aligned even/odd pair.
        // Flush each pair once instead of issuing duplicate INVTLB operations.
        let pair_size = PAGE_SIZE << 1;
        let mut address = (start_vpn.0 * PAGE_SIZE) & !(pair_size - 1);
        let end = end_vpn.0 * PAGE_SIZE;
        while address < end {
            TLB::flush_vaddr(VirtAddr::from(address));
            address += pair_size;
        }
    }

    "local_page"
}

struct MprotectDetail {
    vm_lock_ns: usize,
    preflight_ns: usize,
    vma_update_ns: usize,
    pte_walk_ns: usize,
    tlb_ns: usize,
    areas_scanned: usize,
    areas_changed: usize,
    vma_splits: usize,
    vma_merges: usize,
    prefix_extensions: usize,
    ptes_walked: usize,
    ptes_present: usize,
    ptes_changed: usize,
    pte_changed: bool,
    tlb_kind: &'static str,
    no_op: bool,
}

impl Default for MprotectDetail {
    fn default() -> Self {
        Self {
            vm_lock_ns: 0,
            preflight_ns: 0,
            vma_update_ns: 0,
            pte_walk_ns: 0,
            tlb_ns: 0,
            areas_scanned: 0,
            areas_changed: 0,
            vma_splits: 0,
            vma_merges: 0,
            prefix_extensions: 0,
            ptes_walked: 0,
            ptes_present: 0,
            ptes_changed: 0,
            pte_changed: false,
            tlb_kind: "none",
            no_op: false,
        }
    }
}

fn pte_change_requires_remote_shootdown(old: PTEFlags, new: PTEFlags) -> bool {
    let access_mask = PTEFlags::leaf_access_mask().bits();
    let non_access_changed = (old.bits() & !access_mask) != (new.bits() & !access_mask);
    non_access_changed
        || (old.readable() && !new.readable())
        || (old.writable() && !new.writable())
        || (old.executable() && !new.executable())
        || (old.plv_user() && !new.plv_user())
}

fn trim_user_range(vm_set: &mut UserVMSet, start: usize, end: usize) -> Vec<Arc<FrameTracker>> {
    let mut cleared_pte = false;
    // Keep removed frames alive until every CPU has discarded translations
    // for all PTEs in this batch. Dropping a frame before the shootdown would
    // allow a stale writable TLB entry to access a recycled physical page.
    let mut retired_frames: Vec<Arc<FrameTracker>> = Vec::new();
    let mut idx = 0;
    while idx < vm_set.areas.len() {
        let area_type = vm_set.areas[idx].areatype();
        if !matches!(
            area_type,
            UserMapAreaType::Elf
                | UserMapAreaType::Stack
                | UserMapAreaType::Heap
                | UserMapAreaType::Mmap
                | UserMapAreaType::Shm
        ) {
            idx += 1;
            continue;
        }

        let area_start = vm_set.areas[idx].start_va().0;
        let area_end = vm_set.areas[idx].end_va().0;
        let overlap_start = start.max(area_start);
        let overlap_end = end.min(area_end);
        if overlap_start >= overlap_end {
            idx += 1;
            continue;
        }
        let unmap_start_vpn = VirtAddr::from(overlap_start).floor();
        let unmap_end_vpn = VirtAddr::from(overlap_end).ceil();
        {
            let area = &mut vm_set.areas[idx];
            for vpn in VPNRange::new(unmap_start_vpn, unmap_end_vpn) {
                if let Some(frame) = area.data_frames.remove(&vpn) {
                    vm_set.page_table.unmap_page_no_flush(vpn);
                    cleared_pte = true;
                    retired_frames.push(frame);
                }
            }
        }

        if overlap_start == area_start && overlap_end == area_end {
            let removed = vm_set.areas.remove(idx);
            if removed.areatype() == UserMapAreaType::Shm {
                release_shm_attaches(core::slice::from_ref(&removed));
            }
            continue;
        }

        if overlap_start == area_start {
            let area = &mut vm_set.areas[idx];
            area.trim_start(VirtAddr::from(overlap_end));
            let keep_start = area.start_vpn();
            let keep_end = area.end_vpn();
            area.data_frames
                .retain(|vpn, _| *vpn >= keep_start && *vpn < keep_end);
            idx += 1;
            continue;
        }

        if overlap_end == area_end {
            let area = &mut vm_set.areas[idx];
            area.range_va_mut().end = VirtAddr::from(overlap_start);
            let keep_start = area.start_vpn();
            let keep_end = area.end_vpn();
            area.data_frames
                .retain(|vpn, _| *vpn >= keep_start && *vpn < keep_end);
            idx += 1;
            continue;
        }

        let old_end = area_end;
        let mut right = {
            let area = &vm_set.areas[idx];
            UserMapArea::from_another(area)
        };
        {
            let area = &mut vm_set.areas[idx];
            area.range_va_mut().end = VirtAddr::from(overlap_start);
            let keep_start = area.start_vpn();
            let keep_end = area.end_vpn();
            area.data_frames
                .retain(|vpn, _| *vpn >= keep_start && *vpn < keep_end);
        }
        right.trim_start(VirtAddr::from(overlap_end));
        right.range_va_mut().end = VirtAddr::from(old_end);
        let right_start = right.start_vpn();
        let right_end = right.end_vpn();
        right
            .data_frames
            .retain(|vpn, _| *vpn >= right_start && *vpn < right_end);
        vm_set.areas.insert(idx + 1, right);
        idx += 2;
    }

    if cleared_pte {
        polyhal::multicore::shootdown_tlb_all(vm_set.token());
    }
    retired_frames
}

// fn trim_mmap_range(vm_set: &mut UserVMSet, start: usize, end: usize) -> bool {
//     trim_user_range(vm_set, start, end, false)
// }

// fn trim_fixed_mapping_range(vm_set: &mut UserVMSet, start: usize, end: usize) -> bool {
//     trim_user_range(vm_set, start, end, true)
// }

fn populate_mmap_range(vm_set: &mut UserVMSet, start: usize, len: usize) -> Result<(), SysError> {
    let end = start.checked_add(len).ok_or(SysError::ENOMEM)?;
    let start_vpn = VirtAddr::from(start).floor();
    let end_vpn = VirtAddr::from(end).ceil();
    for vpn in VPNRange::new(start_vpn, end_vpn) {
        match vm_set.handle_unalloc_page_fault(VirtAddr::from(vpn.0 * PAGE_SIZE), AccessType::Read)
        {
            Some(vm_set::PageFaultError::Normal) => {}
            Some(vm_set::PageFaultError::OutOfMemory) => return Err(SysError::ENOMEM),
            Some(vm_set::PageFaultError::BeyondFileSize) => return Err(SysError::ENXIO),
            Some(vm_set::PageFaultError::InvalidMapping)
            | Some(vm_set::PageFaultError::InvalidAddress)
            | None => {
                return Err(SysError::EFAULT);
            }
        }
    }
    Ok(())
}

pub fn sys_mmap(
    start: usize,
    len: usize,
    prot: usize,
    flags: usize,
    fd: usize,
    offset: usize,
) -> SyscallResult {
    let _perf_timer = scope_timer(PerfTimerKind::Mmap);
    const MAP_SHARED: usize = 0x01;
    const MAP_PRIVATE: usize = 0x02;
    const MAP_FIXED: usize = 0x10;
    const MAP_ANONYMOUS: usize = 0x20;
    const MAP_FIXED_NOREPLACE: usize = 0x100000;
    const MAP_GROWSDOWN: usize = 0x00100;
    const MAP_POPULATE: usize = 0x2000;
    const PROT_WRITE: usize = 0x02;
    warn!(
        "sys_mmap: start: {}, len: {}, prot: {}, flags: {}, fd: {}, offset: {}",
        start, len, prot, flags, fd, offset
    );
    // 先检查 fd 是否有效
    let process = current_process();

    // Declared before the process guard so recycled frames are dropped only
    // after the address-space lock is released on every return path.
    let mut retired_frames = Vec::new();
    let map_file = if (flags & MAP_ANONYMOUS) == 0 {
        let inner = process.inner_exclusive_access();
        let Some(file) = inner
            .fd_table
            .get(fd)
            .and_then(|file| file.as_ref())
            .cloned()
        else {
            info!("[DEBUG] sys_mmap: invalid fd={}", fd);
            return Err(SysError::EBADF);
        };
        Some(file)
    } else {
        None
    };
    let map_file_path = map_file
        .as_ref()
        .map(|file| Arc::<str>::from(file.get_dentry().path()));
    let mut vm_set = process.vm_exclusive_access();

    if len == 0 {
        return Err(SysError::EINVAL);
    }
    if (flags & (MAP_SHARED | MAP_PRIVATE)) == 0
    // || (flags & (MAP_SHARED | MAP_PRIVATE)) == (MAP_SHARED | MAP_PRIVATE)
    {
        return Err(SysError::EINVAL);
    }
    if (flags & (MAP_FIXED | MAP_FIXED_NOREPLACE)) != 0 && (start & (PAGE_SIZE - 1)) != 0 {
        return Err(SysError::EINVAL);
    }
    if (flags & MAP_ANONYMOUS) == 0 && (offset & (PAGE_SIZE - 1)) != 0 {
        return Err(SysError::EINVAL);
    }

    let page_aligned_len =
        len.checked_add(PAGE_SIZE - 1).ok_or(SysError::ENOMEM)? & !(PAGE_SIZE - 1);
    if (flags & MAP_ANONYMOUS) == 0 && offset.checked_add(page_aligned_len).is_none() {
        return Err(SysError::EOVERFLOW);
    }
    let end_req = match start.checked_add(page_aligned_len) {
        Some(v) => v,
        None => return Err(SysError::ENOMEM),
    };
    if end_req == 0 {
        return Err(SysError::ENOMEM);
    }

    let target_start = if (flags & (MAP_FIXED | MAP_FIXED_NOREPLACE)) != 0 {
        start
    } else {
        let hint = if start == 0 {
            0
        } else {
            start & !(PAGE_SIZE - 1)
        };
        match vm_set.find_free_area(hint, page_aligned_len) {
            Some(addr) => addr,
            None => return Err(SysError::ENOMEM),
        }
    };
    let target_end = target_start
        .checked_add(page_aligned_len)
        .ok_or(SysError::ENOMEM)?;
    // User page tables inherit the kernel half by sharing the kernel's
    // intermediate page-table pages.  Letting MAP_FIXED publish a VMA above
    // TASK_SIZE would therefore allow a later fault/munmap to overwrite or
    // clear a kernel direct-map PTE.  Validate the final address (not merely
    // the original hint) before constructing any VMA or touching any PTE.
    if !valid_user_range(target_start, target_end) {
        return Err(SysError::ENOMEM);
    }
    let start_va = VirtAddr::from(target_start);
    let end_va = VirtAddr::from(target_end);
    let map_perm = MapPermission::from_prot(prot);

    // 检查 MAP_FIXED_NOREPLACE：如果地址范围已被占用，返回 EEXIST
    if (flags & MAP_FIXED_NOREPLACE) != 0 {
        for area in vm_set.areas.iter() {
            let area_start = area.start_va().0;
            let area_end = area.end_va().0;
            if target_start < area_end && target_end > area_start {
                // 地址范围重叠
                return Err(SysError::EEXIST);
            }
        }
    } else if (flags & MAP_FIXED) != 0 {
        retired_frames.extend(trim_user_range(&mut vm_set, start_va.0, end_va.0));
    }

    if (flags & MAP_ANONYMOUS) != 0 {
        vm_set.insert_framed_area(
            start_va,
            end_va,
            map_perm,
            UserMapAreaType::Mmap,
            Some((None, offset, flags)),
        );
        if let Some(area) = vm_set.find_area(start_va) {
            if (flags & MAP_GROWSDOWN) != 0 {
                area.growdown_flag = true;
            }
            if (flags & MAP_SHARED) != 0 {
                area.flags = crate::mm::vm_area::MmapType::MapShared;
            }
        }
    } else {
        let file = map_file.expect("non-anonymous mmap lost its file snapshot");

        // 添加文件类型检查：只有常规文件和设备文件才能被 mmap
        use crate::fs::vfs::inode::InodeMode;
        let inode = file.get_inode().ok_or(SysError::ENODEV)?;
        let mode = inode.get_mode();
        let file_type = mode.bits() & InodeMode::TYPE_MASK.bits();
        // 如果设置了 MAP_POPULATE，只有常规文件支持
        if (flags & MAP_POPULATE) != 0 && file_type != InodeMode::FILE.bits() {
            info!("[DEBUG] sys_mmap: MAP_POPULATE not supported for this file type");
            return Err(SysError::ENOENT);
        }
        // 如果设置了 MAP_NONBLOCK，只有常规文件支持
        const MAP_NONBLOCK: usize = 0x400;
        if (flags & MAP_NONBLOCK) != 0 && file_type != InodeMode::FILE.bits() {
            info!("[DEBUG] sys_mmap: MAP_NONBLOCK not supported for this file type");
            return Err(SysError::ENOENT);
        }
        if file_type != InodeMode::FILE.bits()
            && file_type != InodeMode::CHAR.bits()
            && file_type != InodeMode::BLOCK.bits()
        {
            info!(
                "[DEBUG] sys_mmap: cannot mmap this file type, mode={:o}",
                mode.bits()
            );
            return Err(SysError::ENODEV);
        }
        if file_type == InodeMode::FILE.bits() && file.cache_inode_id().is_none() {
            return Err(SysError::ENODEV);
        }

        // 检查文件打开模式：mmap 需要读取文件内容，所以文件必须可读
        if !file.readable() {
            info!("[DEBUG] sys_mmap: file is not readable (O_WRONLY), cannot mmap");
            return Err(SysError::EACCES);
        }

        // MAP_PRIVATE | PROT_WRITE is copy-on-write and does not require a writable fd.
        if (prot & PROT_WRITE) != 0 && (flags & MAP_SHARED) != 0 && !file.writable() {
            info!("[DEBUG] sys_mmap: file is not writable, cannot create shared write mapping");
            return Err(SysError::EACCES);
        }
        // 新增：检查 memfd seal: F_SEAL_WRITE 禁止写映射
        if (prot & PROT_WRITE) != 0 && (flags & MAP_SHARED) != 0 {
            if let Some(inode) = file.get_inode() {
                if (inode.get_seals() & F_SEAL_WRITE) != 0 {
                    return Err(SysError::EPERM);
                }
            }
        }
        vm_set.insert_framed_area(
            start_va,
            end_va,
            map_perm,
            UserMapAreaType::Mmap,
            Some((Some(file), offset, flags)),
        );
        if let Some(area) = vm_set.find_area(start_va) {
            area.mapping_path = map_file_path;
            if (flags & MAP_GROWSDOWN) != 0 {
                area.growdown_flag = true;
            }
        }
    }

    if (flags & MAP_POPULATE) != 0 {
        populate_mmap_range(&mut vm_set, target_start, page_aligned_len)?;
    }

    Ok(target_start)
}

pub fn sys_munmap(start: usize, len: usize) -> SyscallResult {
    let _perf_timer = scope_timer(PerfTimerKind::Munmap);
    if len == 0 || (start & (PAGE_SIZE - 1)) != 0 {
        return Err(SysError::EINVAL);
    }
    let page_aligned_len = page_align_len(len)?;
    let end = match start.checked_add(page_aligned_len) {
        Some(v) => v,
        None => return Err(SysError::EINVAL),
    };
    if !valid_user_range(start, end) {
        return Err(SysError::EINVAL);
    }
    let process = current_process();
    let (retired_frames, shared_file_pages) = {
        let mut vm_set = process.vm_exclusive_access();
        let shared_file_pages = crate::mm::snapshot_shared_file_pages(&vm_set.areas, start, end);
        (trim_user_range(&mut vm_set, start, end), shared_file_pages)
    };
    crate::mm::queue_shared_file_pages_for_writeback(shared_file_pages);
    drop(retired_frames);
    Ok(0)
}

fn page_align_len(len: usize) -> Result<usize, SysError> {
    len.checked_add(PAGE_SIZE - 1)
        .map(|len| len & !(PAGE_SIZE - 1))
        .filter(|len| *len != 0)
        .ok_or(SysError::EINVAL)
}

fn valid_user_range(start: usize, end: usize) -> bool {
    start >= USER_MEMORY_SPACE.0
        && start < end
        && end
            .checked_sub(1)
            .is_some_and(|last| last <= USER_MEMORY_SPACE.1)
}

fn vm_range_is_free(vm_set: &UserVMSet, start: usize, end: usize) -> bool {
    vm_set
        .areas
        .iter()
        .all(|area| end <= area.start_va().0 || start >= area.end_va().0)
}

fn mremap_source_index(vm_set: &UserVMSet, start: usize, end: usize) -> Option<usize> {
    vm_set.areas.iter().position(|area| {
        area.areatype() == UserMapAreaType::Mmap
            && start >= area.start_va().0
            && end <= area.end_va().0
    })
}

fn take_mremap_range(
    vm_set: &mut UserVMSet,
    start: usize,
    end: usize,
) -> Result<UserMapArea, SysError> {
    let idx = mremap_source_index(vm_set, start, end).ok_or(SysError::EFAULT)?;
    let original_start = vm_set.areas[idx].start_va().0;
    let original_end = vm_set.areas[idx].end_va().0;
    let middle_file_offset = vm_set.areas[idx]
        .file_offset
        .checked_add(start - original_start)
        .ok_or(SysError::EOVERFLOW)?;
    let right_file_offset = vm_set.areas[idx]
        .file_offset
        .checked_add(end - original_start)
        .ok_or(SysError::EOVERFLOW)?;

    let mut area = vm_set.areas.remove(idx);
    let left = if start > original_start {
        let mut left = UserMapArea::from_another(&area);
        left.range_va_mut().end = VirtAddr::from(start);
        let keep_start = left.start_vpn();
        let keep_end = left.end_vpn();
        left.data_frames
            .retain(|vpn, _| *vpn >= keep_start && *vpn < keep_end);
        Some(left)
    } else {
        None
    };
    let right = if end < original_end {
        let mut right = UserMapArea::from_another(&area);
        right.trim_start(VirtAddr::from(end));
        right.file_offset = right_file_offset;
        let keep_start = right.start_vpn();
        let keep_end = right.end_vpn();
        right
            .data_frames
            .retain(|vpn, _| *vpn >= keep_start && *vpn < keep_end);
        Some(right)
    } else {
        None
    };

    if start > original_start {
        area.trim_start(VirtAddr::from(start));
        area.file_offset = middle_file_offset;
    }
    area.range_va_mut().end = VirtAddr::from(end);
    let keep_start = area.start_vpn();
    let keep_end = area.end_vpn();
    area.data_frames
        .retain(|vpn, _| *vpn >= keep_start && *vpn < keep_end);

    if let Some(left) = left {
        vm_set.insert_area_sorted(left);
    }
    if let Some(right) = right {
        vm_set.insert_area_sorted(right);
    }
    Ok(area)
}

fn relocate_mremap_area(
    vm_set: &mut UserVMSet,
    area: &mut UserMapArea,
    old_start: usize,
    new_start: usize,
    new_len: usize,
    retired_frames: &mut Vec<Arc<FrameTracker>>,
) {
    let old_start_vpn = VirtAddr::from(old_start).floor();
    let new_start_vpn = VirtAddr::from(new_start).floor();
    let new_page_count = new_len / PAGE_SIZE;
    let mapping_flags = if area.cow_flag() && area.perm().contains(MapPermission::W) {
        cow_mapping_flags(*area.perm())
    } else {
        MappingFlags::from(*area.perm())
    };
    let old_frames = core::mem::take(&mut area.data_frames);
    let mut new_frames: alloc::collections::BTreeMap<VirtPageNum, Arc<FrameTracker>> =
        Default::default();

    let mut cleared_old_mapping = false;
    for &old_vpn in old_frames.keys() {
        if vm_set.page_table.translate(old_vpn).is_some() {
            vm_set.page_table.unmap_page_no_flush(old_vpn);
            cleared_old_mapping = true;
        }
    }
    if cleared_old_mapping {
        // old_frames retains every backing frame until stale translations on
        // all CPUs are gone. The frames can then be installed at their new
        // virtual addresses or safely released below.
        polyhal::multicore::shootdown_tlb_all(vm_set.token());
    }

    for (old_vpn, frame) in old_frames {
        let relative_page = old_vpn.0.saturating_sub(old_start_vpn.0);
        if relative_page >= new_page_count {
            retired_frames.push(frame);
            continue;
        }
        let new_vpn = VirtPageNum(new_start_vpn.0 + relative_page);
        vm_set
            .page_table
            .map_page(new_vpn, frame.ppn, mapping_flags, MappingSize::Page4KB);
        new_frames.insert(new_vpn, frame);
    }

    area.data_frames = new_frames;
    area.range_va_mut().start = VirtAddr::from(new_start);
    area.range_va_mut().end = VirtAddr::from(new_start + new_len);
    if area.data_frames.len() < new_page_count {
        area.set_lazy_flag();
    } else {
        area.clear_lazy_flag();
    }
}

pub fn sys_mremap(
    old_address: usize,
    old_size: usize,
    new_size: usize,
    flags: usize,
    new_address: usize,
) -> SyscallResult {
    const MREMAP_MAYMOVE: usize = 1;
    const MREMAP_FIXED: usize = 2;
    const MREMAP_DONTUNMAP: usize = 4;
    const VALID_FLAGS: usize = MREMAP_MAYMOVE | MREMAP_FIXED | MREMAP_DONTUNMAP;

    info!(
        "[mremap] old={:#x} old_size={:#x} new_size={:#x} flags={:#x} new={:#x}",
        old_address, old_size, new_size, flags, new_address
    );

    if old_address & (PAGE_SIZE - 1) != 0
        || old_size == 0
        || new_size == 0
        || flags & !VALID_FLAGS != 0
        || (flags & MREMAP_FIXED != 0 && flags & MREMAP_MAYMOVE == 0)
        || (flags & MREMAP_DONTUNMAP != 0 && flags & MREMAP_MAYMOVE == 0)
    {
        return Err(SysError::EINVAL);
    }

    let old_len = page_align_len(old_size)?;
    let new_len = page_align_len(new_size)?;
    if flags & MREMAP_DONTUNMAP != 0 && old_len != new_len {
        return Err(SysError::EINVAL);
    }
    let old_end = old_address.checked_add(old_len).ok_or(SysError::EINVAL)?;
    if !valid_user_range(old_address, old_end) {
        return Err(SysError::EFAULT);
    }

    let process = current_process();
    // Keep this before the VM guard: reverse drop order releases the address
    // space before any last FrameTracker reference reaches the allocator.
    let mut retired_frames = Vec::new();
    let mut vm_set = process.vm_exclusive_access();
    let source_idx = mremap_source_index(&vm_set, old_address, old_end).ok_or(SysError::EFAULT)?;

    if flags & (MREMAP_FIXED | MREMAP_DONTUNMAP) == 0 {
        if new_len == old_len {
            return Ok(old_address);
        }
        if new_len < old_len {
            let new_end = old_address + new_len;
            retired_frames.extend(trim_user_range(&mut vm_set, new_end, old_end));
            return Ok(old_address);
        }

        let source_area_end = vm_set.areas[source_idx].end_va().0;
        let new_end = old_address.checked_add(new_len).ok_or(SysError::ENOMEM)?;
        if source_area_end == old_end
            && valid_user_range(old_address, new_end)
            && vm_range_is_free(&vm_set, old_end, new_end)
        {
            vm_set.areas[source_idx].expand(VirtAddr::from(new_end));
            vm_set.areas[source_idx].set_lazy_flag();
            return Ok(old_address);
        }
    }

    if flags & MREMAP_MAYMOVE == 0 {
        return Err(SysError::ENOMEM);
    }

    let target_start = if flags & MREMAP_FIXED != 0 {
        if new_address & (PAGE_SIZE - 1) != 0 {
            return Err(SysError::EINVAL);
        }
        let target_end = new_address.checked_add(new_len).ok_or(SysError::ENOMEM)?;
        if !valid_user_range(new_address, target_end)
            || (new_address < old_end && target_end > old_address)
        {
            return Err(SysError::EINVAL);
        }
        retired_frames.extend(trim_user_range(&mut vm_set, new_address, target_end));
        if !vm_range_is_free(&vm_set, new_address, target_end) {
            return Err(SysError::EINVAL);
        }
        new_address
    } else {
        vm_set.find_free_area(0, new_len).ok_or(SysError::ENOMEM)?
    };

    let mut area = take_mremap_range(&mut vm_set, old_address, old_end)?;
    let old_replacement = if flags & MREMAP_DONTUNMAP != 0 {
        let mut replacement = UserMapArea::from_another(&area);
        replacement.data_frames.clear();
        replacement.set_lazy_flag();
        Some(replacement)
    } else {
        None
    };
    relocate_mremap_area(
        &mut vm_set,
        &mut area,
        old_address,
        target_start,
        new_len,
        &mut retired_frames,
    );
    vm_set.insert_area_sorted(area);
    if let Some(replacement) = old_replacement {
        vm_set.insert_area_sorted(replacement);
    }
    Ok(target_start)
}

pub fn sys_madvice(addr: usize, len: usize, advice: usize) -> SyscallResult {
    // POSIX standard madvise advice values
    const MADV_NORMAL: usize = 0;
    const MADV_RANDOM: usize = 1;
    const MADV_SEQUENTIAL: usize = 2;
    const MADV_WILLNEED: usize = 3;
    const MADV_DONTNEED: usize = 4;

    // Linux-specific madvise advice values
    const MADV_FREE: usize = 8;
    const MADV_REMOVE: usize = 9;
    const MADV_DONTFORK: usize = 10;
    const MADV_DOFORK: usize = 11;
    const MADV_MERGEABLE: usize = 12;
    const MADV_UNMERGEABLE: usize = 13;
    const MADV_HUGEPAGE: usize = 14;
    const MADV_NOHUGEPAGE: usize = 15;
    const MADV_DONTDUMP: usize = 16;
    const MADV_DODUMP: usize = 17;
    const MADV_WIPEONFORK: usize = 18;
    const MADV_KEEPONFORK: usize = 19;
    const MADV_COLLAPSE: usize = 20;
    const MADV_PAGEOUT: usize = 21;
    const MADV_HWPOISON: usize = 100;

    // Check for zero length
    if len == 0 {
        info!("[DEBUG] sys_madvice: len is zero");
        return Err(SysError::EINVAL);
    }

    // Check address alignment
    if (addr & (PAGE_SIZE - 1)) != 0 {
        info!("[DEBUG] sys_madvice: addr not page aligned: {:#x}", addr);
        return Err(SysError::EINVAL);
    }

    // Check for overflow
    let aligned_len = match len.checked_add(PAGE_SIZE - 1) {
        Some(value) => value & !(PAGE_SIZE - 1),
        None => {
            info!("[DEBUG] sys_madvice: address overflow");
            return Err(SysError::EINVAL);
        }
    };
    let end = match addr.checked_add(aligned_len) {
        Some(v) => v,
        None => {
            info!("[DEBUG] sys_madvice: address overflow");
            return Err(SysError::EINVAL);
        }
    };

    // Check if address range is valid for this process
    let process = current_process();
    let mut retired_frames: Vec<Arc<FrameTracker>> = Vec::new();
    let mut vm_set = process.vm_exclusive_access();
    let start_vpn = VirtAddr::from(addr).floor();
    let end_vpn = VirtAddr::from(end).ceil();

    let mut covered_until = start_vpn;
    for area in vm_set.areas.iter() {
        if area.end_vpn() <= covered_until {
            continue;
        }
        if area.start_vpn() > covered_until {
            break;
        }
        covered_until = area.end_vpn();
        if covered_until >= end_vpn {
            break;
        }
    }
    if covered_until < end_vpn {
        info!(
            "[DEBUG] sys_madvice: address range not in any VM area: {:#x}-{:#x}",
            addr, end
        );
        return Err(SysError::ENOMEM);
    }

    match advice {
        // These values are genuine access/reclaim hints and have no required
        // immediate data or fork-visible state transition.
        MADV_NORMAL | MADV_RANDOM | MADV_SEQUENTIAL | MADV_WILLNEED | MADV_HUGEPAGE
        | MADV_NOHUGEPAGE | MADV_PAGEOUT => Ok(0),
        MADV_DONTNEED | MADV_FREE => {
            // Preflight the whole interval. Kairix can safely discard private
            // anonymous pages and file-backed PTEs; shared-anonymous backing
            // needs coordinated removal from every mm and is rejected until
            // that operation is implemented.
            for area in vm_set.areas.iter() {
                if area.end_vpn() <= start_vpn || area.start_vpn() >= end_vpn {
                    continue;
                }
                let supported = matches!(
                    area.areatype(),
                    UserMapAreaType::Heap | UserMapAreaType::Stack | UserMapAreaType::Mmap
                ) && area.shared_anonymous.is_none();
                if !supported {
                    return Err(SysError::EINVAL);
                }
                if advice == MADV_FREE
                    && (area.areatype() != UserMapAreaType::Mmap
                        || area.map_file.is_some()
                        || area.flags == MmapType::MapShared)
                {
                    return Err(SysError::EINVAL);
                }
            }

            let mut removed_vpns = Vec::new();
            for area in vm_set.areas.iter_mut() {
                let first = core::cmp::max(area.start_vpn(), start_vpn);
                let last = core::cmp::min(area.end_vpn(), end_vpn);
                if first >= last {
                    continue;
                }
                for vpn in VPNRange::new(first, last) {
                    if let Some(frame) = area.data_frames.remove(&vpn) {
                        removed_vpns.push(vpn);
                        retired_frames.push(frame);
                    }
                }
                area.set_lazy_flag();
            }
            let mut unmapped = false;
            for vpn in removed_vpns {
                if vm_set.page_table.translate(vpn).is_some() {
                    vm_set.page_table.unmap_page_no_flush(vpn);
                    unmapped = true;
                }
            }
            if unmapped {
                // Keep retired_frames alive until no CPU can use the old PTE.
                polyhal::multicore::shootdown_tlb_all(vm_set.token());
            }
            drop(vm_set);
            drop(retired_frames);
            Ok(0)
        }
        // These commands have mandatory VMA, fork, writeback, or privilege
        // semantics. Returning success without them is observably wrong.
        MADV_REMOVE | MADV_DONTFORK | MADV_DOFORK | MADV_MERGEABLE | MADV_UNMERGEABLE
        | MADV_DONTDUMP | MADV_DODUMP | MADV_WIPEONFORK | MADV_KEEPONFORK | MADV_COLLAPSE
        | MADV_HWPOISON => Err(SysError::EINVAL),
        _ => {
            info!("[DEBUG] sys_madvice: invalid advice value {}", advice);
            Err(SysError::EINVAL)
        }
    }
}

pub fn sys_mprotect(start: usize, len: usize, prot: usize) -> SyscallResult {
    let _perf_timer = scope_timer(PerfTimerKind::Mprotect);
    let started_ns = crate::task::perf_stats::now_ns();
    let caller = current_task();
    let context_switches_before = caller
        .as_ref()
        .map_or(0, |task| task.runtime_diagnostic().context_switches);
    let mut detail = MprotectDetail::default();
    let aligned_end = len
        .checked_add(PAGE_SIZE - 1)
        .map(|value| value & !(PAGE_SIZE - 1))
        .and_then(|aligned_len| start.checked_add(aligned_len));
    log::error!(
        "[MPROTECT_TRACE] event=enter cpu={} pid={} tid={} start={:#x} len={} prot={:#x} aligned_end={:?}",
        polyhal::arch::hart_id(),
        caller.as_ref().map_or(usize::MAX, |task| task.process_id()),
        caller.as_ref().map_or(usize::MAX, |task| task.global_tid()),
        start,
        len,
        prot,
        aligned_end,
    );
    // Reuse the task snapshot already needed by diagnostics instead of
    // reacquiring the current processor/task to obtain the same process.
    let process = caller
        .as_ref()
        .and_then(|task| task.process.upgrade())
        .expect("mprotect caller process disappeared");
    let inner_started_ns = crate::task::perf_stats::now_ns();
    let result = sys_mprotect_inner(start, len, prot, &process, &mut detail);
    let inner_ns = crate::task::perf_stats::elapsed_since(inner_started_ns);
    let elapsed_ns = crate::task::perf_stats::now_ns().saturating_sub(started_ns);
    let context_switches = caller.as_ref().map_or(0, |task| {
        task.runtime_diagnostic()
            .context_switches
            .saturating_sub(context_switches_before)
    });
    let accounted_ns = detail
        .vm_lock_ns
        .saturating_add(detail.preflight_ns)
        .saturating_add(detail.vma_update_ns)
        .saturating_add(detail.pte_walk_ns)
        .saturating_add(detail.tlb_ns);
    let unaccounted_ns = elapsed_ns.saturating_sub(accounted_ns);
    crate::task::perf_stats::record_mprotect_phase(crate::task::perf_stats::MprotectPhaseSample {
        elapsed_ns,
        inner_ns,
        context_switches,
        vm_lock_ns: detail.vm_lock_ns,
        preflight_ns: detail.preflight_ns,
        vma_update_ns: detail.vma_update_ns,
        pte_walk_ns: detail.pte_walk_ns,
        tlb_ns: detail.tlb_ns,
        prefix_extensions: detail.prefix_extensions,
        vma_splits: detail.vma_splits,
        vma_merges: detail.vma_merges,
        ptes_walked: detail.ptes_walked,
        ptes_present: detail.ptes_present,
        ptes_changed: detail.ptes_changed,
        tlb_kind: detail.tlb_kind,
    });
    if unaccounted_ns >= MPROTECT_GAP_SLOW_NS || context_switches != 0 {
        log::error!(
            "[MPROTECT_GAP_DETAIL] cpu={} pid={} tid={} start={:#x} len={} prot={:#x} elapsed_ns={} accounted_ns={} unaccounted_ns={} context_switches={} prefix_extensions={} ptes_walked={} ptes_present={} ptes_changed={} tlb_kind={} result={:?}",
            polyhal::arch::hart_id(),
            caller.as_ref().map_or(usize::MAX, |task| task.process_id()),
            caller.as_ref().map_or(usize::MAX, |task| task.global_tid()),
            start,
            len,
            prot,
            elapsed_ns,
            accounted_ns,
            unaccounted_ns,
            context_switches,
            detail.prefix_extensions,
            detail.ptes_walked,
            detail.ptes_present,
            detail.ptes_changed,
            detail.tlb_kind,
            result,
        );
    }
    log::error!(
        "[MPROTECT_TRACE] event=exit cpu={} pid={} tid={} start={:#x} len={} prot={:#x} aligned_end={:?} elapsed_ns={} result={:?}",
        polyhal::arch::hart_id(),
        caller.as_ref().map_or(usize::MAX, |task| task.process_id()),
        caller.as_ref().map_or(usize::MAX, |task| task.global_tid()),
        start,
        len,
        prot,
        aligned_end,
        crate::task::perf_stats::now_ns().saturating_sub(started_ns),
        result,
    );
    log::error!(
        "[MPROTECT_DETAIL] cpu={} pid={} tid={} start={:#x} len={} prot={:#x} vm_lock_ns={} preflight_ns={} vma_update_ns={} pte_walk_ns={} tlb_ns={} areas_scanned={} areas_changed={} vma_splits={} vma_merges={} prefix_extensions={} ptes_walked={} ptes_present={} ptes_changed={} pte_changed={} tlb_kind={} no_op={}",
        polyhal::arch::hart_id(),
        caller.as_ref().map_or(usize::MAX, |task| task.process_id()),
        caller.as_ref().map_or(usize::MAX, |task| task.global_tid()),
        start,
        len,
        prot,
        detail.vm_lock_ns,
        detail.preflight_ns,
        detail.vma_update_ns,
        detail.pte_walk_ns,
        detail.tlb_ns,
        detail.areas_scanned,
        detail.areas_changed,
        detail.vma_splits,
        detail.vma_merges,
        detail.prefix_extensions,
        detail.ptes_walked,
        detail.ptes_present,
        detail.ptes_changed,
        detail.pte_changed,
        detail.tlb_kind,
        detail.no_op,
    );
    result
}

fn sys_mprotect_inner(
    start: usize,
    len: usize,
    prot: usize,
    process: &crate::task::ProcessControlBlock,
    detail: &mut MprotectDetail,
) -> SyscallResult {
    if len == 0 {
        detail.no_op = true;
        return Ok(0);
    }
    if (start & (PAGE_SIZE - 1)) != 0 {
        return Err(SysError::EINVAL);
    }
    if prot & !0x7 != 0 {
        return Err(SysError::EINVAL);
    }
    let aligned_len = len
        .checked_add(PAGE_SIZE - 1)
        .map(|value| value & !(PAGE_SIZE - 1))
        .filter(|value| *value != 0)
        .ok_or(SysError::ENOMEM)?;
    let end = start.checked_add(aligned_len).ok_or(SysError::ENOMEM)?;
    if end <= start {
        return Err(SysError::ENOMEM);
    }
    if !valid_user_range(start, end) {
        return Err(SysError::ENOMEM);
    }

    let vm_lock_started_ns = crate::task::perf_stats::now_ns();
    let mut vm_set = process.vm_exclusive_access();
    detail.vm_lock_ns = crate::task::perf_stats::elapsed_since(vm_lock_started_ns);
    let start_va = VirtAddr::from(start);
    let end_va = VirtAddr::from(end);
    let new_perm = MapPermission::from_prot(prot);
    let start_vpn = start_va.floor();
    let end_vpn = end_va.ceil();

    // Linux requires every page in the requested interval to be mapped. Start
    // at a binary-searched lower bound and complete all fallible validation
    // before mutating a VMA, so ENOMEM/EPERM cannot leave a changed prefix.
    let preflight_started_ns = crate::task::perf_stats::now_ns();
    let mut covered_until = start_vpn;
    let first_area = vm_set.first_area_ending_after(start_vpn);
    let mut preflight_index = first_area;
    let mut preflight_error = None;
    while preflight_index < vm_set.areas.len() && covered_until < end_vpn {
        let area = &vm_set.areas[preflight_index];
        if area.start_vpn() > covered_until {
            break;
        }
        detail.areas_scanned += 1;
        if new_perm.contains(MapPermission::W) && area.flags == MmapType::MapShared {
            if let Some(file) = &area.map_file {
                if let Some(inode) = file.get_inode() {
                    if (inode.get_seals() & F_SEAL_WRITE) != 0 {
                        preflight_error = Some(SysError::EPERM);
                        break;
                    }
                }
            }
        }
        covered_until = area.end_vpn();
        preflight_index += 1;
    }
    detail.preflight_ns = crate::task::perf_stats::elapsed_since(preflight_started_ns);
    if let Some(error) = preflight_error {
        return Err(error);
    }
    if covered_until < end_vpn {
        return Err(SysError::ENOMEM);
    }

    // Only scan overlapping VMAs. A no-op VMA is deliberately left intact, so
    // repeated mprotect calls cannot fragment it merely by naming a subrange.
    let vma_update_started_ns = crate::task::perf_stats::now_ns();
    let mut i = first_area;
    let mut needs_vma_merge = false;
    while i < vm_set.areas.len() {
        let area_start_vpn = vm_set.areas[i].start_vpn();
        let area_end_vpn = vm_set.areas[i].end_vpn();
        if area_start_vpn >= end_vpn {
            break;
        }
        if !mprotect_area_state_needs_update(&vm_set.areas[i], new_perm) {
            i += 1;
            continue;
        }
        detail.areas_changed += 1;

        let overlap_start = core::cmp::max(start_vpn, area_start_vpn);
        let overlap_end = core::cmp::min(end_vpn, area_end_vpn);
        if overlap_start == area_start_vpn && overlap_end == area_end_vpn {
            apply_mprotect_area_state(&mut vm_set.areas[i], new_perm);
            needs_vma_merge = true;
            i += 1;
            continue;
        }

        if try_expand_mprotect_prefix(&mut vm_set.areas, i, overlap_start, overlap_end, new_perm) {
            detail.prefix_extensions += 1;
            continue;
        }

        let area = vm_set.areas.remove(i);
        needs_vma_merge = true;
        detail.vma_splits += 1;
        let (left, middle, right) = split_mprotect_area(area, overlap_start, overlap_end, new_perm);
        if let Some(left) = left {
            vm_set.areas.insert(i, left);
            i += 1;
        }
        vm_set.areas.insert(i, middle);
        i += 1;
        if let Some(right) = right {
            vm_set.areas.insert(i, right);
            i += 1;
        }
    }

    // Extending a compatible left VMA over a prefix only moves the boundary
    // between two VMAs whose states remain different. It cannot create a new
    // merge opportunity, so the dominant sequential single-page upgrade path
    // need not rescan the VMA vector after every call.
    if needs_vma_merge {
        detail.vma_merges =
            merge_mprotect_areas(&mut vm_set.areas, first_area.saturating_sub(1), end_vpn);
    }
    detail.vma_update_ns = crate::task::perf_stats::elapsed_since(vma_update_started_ns);

    // Update resident PTEs in one VMA/page pass. Permission upgrades can leave
    // an older, more restrictive translation on remote CPUs: a resulting page
    // fault takes the VM lock and the existing stale-TLB recovery path flushes
    // that CPU locally. Revocations and cache-attribute changes must complete a
    // synchronous shootdown before mprotect returns.
    let mut pte_changed = false;
    let mut needs_remote_shootdown = false;
    let mut needs_instruction_sync = false;
    let mut first_changed_vpn: Option<VirtPageNum> = None;
    let mut last_changed_vpn: Option<VirtPageNum> = None;
    let pte_walk_started_ns = crate::task::perf_stats::now_ns();
    // A prefix expansion can move resident pages into the preceding VMA. Start
    // one slot before the original lower bound instead of repeating a binary
    // search after the VMA update.
    let mut area_index = first_area.saturating_sub(1);
    let vm_set_ref: &mut UserVMSet = &mut vm_set;
    let (areas, page_table) = (&vm_set_ref.areas, &mut vm_set_ref.page_table);
    while area_index < areas.len() {
        let area_start = areas[area_index].start_vpn();
        if area_start >= end_vpn {
            break;
        }
        let area_end = areas[area_index].end_vpn();
        if area_end <= start_vpn {
            area_index += 1;
            continue;
        }
        let update_start = core::cmp::max(start_vpn, area_start);
        let update_end = core::cmp::min(end_vpn, area_end);
        let new_flags = area_pte_flags(&areas[area_index]);

        // UserMapArea owns a FrameTracker for every resident user PTE. Walking
        // that sparse index avoids allocating lower-level page-table nodes or
        // probing inherited LoongArch entries for untouched lazy pages. This
        // is especially important for rustc's repeated one-page mprotect calls
        // over a large, not-yet-faulted arena.
        for (&vpn, _) in areas[area_index]
            .data_frames
            .range(update_start..update_end)
        {
            detail.ptes_walked += 1;
            if let Some(pte) = page_table.find_pte(vpn) {
                if pte.is_valid() {
                    detail.ptes_present += 1;
                    let old_flags = pte.flags();
                    if old_flags != new_flags {
                        needs_instruction_sync |= !old_flags.executable() && new_flags.executable();
                        needs_remote_shootdown |=
                            pte_change_requires_remote_shootdown(old_flags, new_flags);
                        *pte = PTE::new(pte.ppn(), new_flags);
                        pte_changed = true;
                        detail.ptes_changed += 1;
                        first_changed_vpn.get_or_insert(vpn);
                        last_changed_vpn = Some(vpn);
                    }
                }
            }
        }
        area_index += 1;
    }
    detail.pte_walk_ns = crate::task::perf_stats::elapsed_since(pte_walk_started_ns);
    detail.pte_changed = pte_changed;

    let tlb_started_ns = crate::task::perf_stats::now_ns();
    if needs_instruction_sync {
        polyhal::multicore::synchronize_instruction_cache(vm_set.token());
        detail.tlb_kind = "icache";
    } else if needs_remote_shootdown {
        polyhal::multicore::shootdown_tlb_all(vm_set.token());
        detail.tlb_kind = "remote";
    } else if pte_changed {
        // A pure upgrade cannot let another CPU retain excessive access. Flush
        // locally so this syscall's CPU observes the new permissions now; a
        // remote CPU repairs its harmless restrictive translation on demand.
        let changed_start = first_changed_vpn.expect("changed PTE missing lower bound");
        let changed_end = VirtPageNum(
            last_changed_vpn
                .expect("changed PTE missing upper bound")
                .0
                .saturating_add(1),
        );
        detail.tlb_kind = flush_mprotect_local_range(changed_start, changed_end);
    }
    detail.tlb_ns = crate::task::perf_stats::elapsed_since(tlb_started_ns);
    detail.no_op = detail.areas_changed == 0 && detail.ptes_changed == 0;
    Ok(0)
}

pub fn sys_msync(addr: usize, len: usize, flags: usize) -> SyscallResult {
    const MS_ASYNC: usize = 1;
    const MS_INVALIDATE: usize = 2;
    const MS_SYNC: usize = 4;

    if addr & (PAGE_SIZE - 1) != 0 {
        return Err(SysError::EINVAL);
    }
    if len == 0 {
        return Ok(0);
    }
    let page_aligned_len = (len + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let end = match addr.checked_add(page_aligned_len) {
        Some(v) => v,
        None => return Err(SysError::ENOMEM),
    };

    // flags 校验
    if (flags & !(MS_ASYNC | MS_INVALIDATE | MS_SYNC)) != 0 {
        return Err(SysError::EINVAL);
    }
    if (flags & MS_ASYNC) != 0 && (flags & MS_SYNC) != 0 {
        return Err(SysError::EINVAL);
    }

    let mut pages_to_mark: Vec<(Arc<dyn File>, usize, Vec<usize>)> = Vec::new();

    {
        let process = current_process();
        let vm_set = process.vm_exclusive_access();

        for area in vm_set.areas.iter() {
            if area.areatype() != UserMapAreaType::Mmap {
                continue;
            }
            if area.flags != MmapType::MapShared {
                continue;
            }
            let area_start = area.start_va().0;
            let area_end = area.end_va().0;
            let overlap_start = addr.max(area_start);
            let overlap_end = end.min(area_end);
            if overlap_start >= overlap_end {
                continue;
            }

            if let Some(file) = &area.map_file {
                let Some(ino) = file.cache_inode_id() else {
                    continue;
                };
                let mut page_ids = Vec::new();
                for (&vpn, _) in area.data_frames.iter() {
                    let page_va = vpn.0 * PAGE_SIZE;
                    if page_va < overlap_start || page_va >= overlap_end {
                        continue;
                    }
                    let offset_in_area = page_va - area_start;
                    let file_offset = area.file_offset + offset_in_area;
                    let page_id = file_offset / PAGE_SIZE;
                    page_ids.push(page_id);
                }
                if !page_ids.is_empty() {
                    pages_to_mark.push((file.clone(), ino, page_ids));
                }
            }
        }
    }

    let mut files_to_flush = Vec::new();
    for (file, ino, page_ids) in pages_to_mark {
        for page_id in page_ids {
            if let Some(page_lock) = PAGE_CACHE.get_page(ino, page_id) {
                let mut page = page_lock.write();
                let generation = file
                    .get_inode()
                    .map(|inode| inode.page_cache_generation())
                    .unwrap_or(0);
                page.mark_dirty_with_generation(generation);
            }
        }
        files_to_flush.push(file);
    }

    for file in files_to_flush {
        file.flush();
    }

    Ok(0)
}

/// Lock the specified address range in physical memory.
///
/// This prevents the memory from being swapped out, ensuring deterministic
/// memory access latency for real-time applications.
///
/// Since our OS doesn't support swap space yet, all memory is already "locked".
/// This implementation simply validates the arguments and returns success.
pub fn sys_mlock(start: usize, len: usize) -> SyscallResult {
    warn!("sys_mlock: start = {:#x}, len = {:#x}", start, len);
    if len == 0 {
        warn!("len==0");
        return Err(SysError::EINVAL);
    }
    // Check for overflow
    let process = current_task().unwrap().process.upgrade().unwrap();
    let inner = process.inner_exclusive_access();
    // First check: permissions - only root (euid=0) can mlock
    if inner.euid != 0 {
        return Err(SysError::EPERM);
    }
    drop(inner);
    let mut vm_set = process.vm_exclusive_access();
    // Second check: validate address range is within process VM areas (returns ENOMEM if invalid)
    let end = start.checked_add(len).ok_or(SysError::EINVAL)?;
    let start_va = VirtAddr::from(start);
    let end_va = VirtAddr::from(end);
    let mut valid = false;
    for area in vm_set.areas.iter() {
        if start_va >= area.start_va() && end_va <= area.end_va() {
            valid = true;
            break;
        }
    }
    if !valid {
        return Err(SysError::ENOMEM);
    }
    let mut pages_to_map = Vec::new();
    for area in vm_set.areas.iter_mut() {
        if start_va >= area.start_va() && end_va <= area.end_va() {
            if area.lazy_flag {
                for vpn in area.vpn_range() {
                    if area.data_frames.contains_key(&vpn) {
                        continue;
                    }
                    let frame = if area.shared_anonymous.is_some() {
                        area.allocate_shared_anonymous_frame(vpn)
                            .ok_or(SysError::ENOMEM)?
                    } else {
                        Arc::new(frame_alloc().ok_or(SysError::ENOMEM)?)
                    };
                    area.data_frames.insert(vpn, frame);
                }
                area.clear_lazy_flag();

                let mapping_flags = MappingFlags::from(*area.perm());
                pages_to_map.extend(
                    area.data_frames
                        .iter()
                        .map(|(&vpn, frame)| (vpn, frame.ppn, mapping_flags)),
                );
            }
        }
    }
    for (vpn, ppn, mapping_flags) in pages_to_map {
        vm_set
            .page_table
            .map_page(vpn, ppn, mapping_flags, MappingSize::Page4KB);
    }
    // Validate alignment (optional in our simplified implementation)
    // if (start & (PAGE_SIZE - 1)) != 0 {
    //     warn!("not aligned");
    //     return Err(SysError::EINVAL);
    // }
    warn!("======");
    let _end = start.checked_add(len).ok_or(SysError::EINVAL)?;
    // In our OS, all memory is already locked (no swap support)
    Ok(0)
}

/// Unlock a range of process memory.
///
/// Since our OS doesn't support swap space yet, all memory is always "locked".
/// This implementation simply validates the arguments and returns success.
pub fn sys_munlock(start: usize, len: usize) -> SyscallResult {
    warn!("sys_munlock: start = {:#x}, len = {:#x}", start, len);
    if len == 0 {
        warn!("len==0");
        return Err(SysError::EINVAL);
    }
    let process = current_task().unwrap().process.upgrade().unwrap();
    let inner = process.inner_exclusive_access();
    // Check permissions: only root (euid=0) can munlock
    if inner.euid != 0 {
        return Err(SysError::EPERM);
    }
    drop(inner);
    let vm_set = process.vm_exclusive_access();
    // Validate address range is within process VM areas
    let end = start.checked_add(len).ok_or(SysError::EINVAL)?;
    let start_va = VirtAddr::from(start);
    let end_va = VirtAddr::from(end);
    let mut valid = false;
    for area in vm_set.areas.iter() {
        if start_va >= area.start_va() && end_va <= area.end_va() {
            valid = true;
            break;
        }
    }
    if !valid {
        return Err(SysError::ENOMEM);
    }
    // In our OS, all memory is always locked (no swap support), so munlock is a no-op
    Ok(0)
}
