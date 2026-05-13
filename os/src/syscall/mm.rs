use crate::error::{SysError, SyscallResult};
use crate::task::current_task;
// use crate::config::PAGE_SIZE;
use polyhal::consts::PAGE_SIZE;

use crate::mm::UserMapArea;
use crate::mm::vm_area::MapArea;
use crate::mm::vm_set::VMSpace;
use crate::mm::{COW, MapPermission, UserMapAreaType, UserVMSet, MmapType};
use crate::syscall::shm::release_shm_attaches;
use crate::task::current_process;
use polyhal::pagetable::*;
use polyhal::utils::addr::{VPNRange, VirtAddr};
use crate::fs::page::pagecache::PAGE_CACHE;

fn trim_mmap_range(vm_set: &mut UserVMSet, start: usize, end: usize) {
    let mut idx = 0;
    while idx < vm_set.areas.len() {
        let area_type = vm_set.areas[idx].areatype();
        if area_type != UserMapAreaType::Mmap && area_type != UserMapAreaType::Shm {
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
                if area.data_frames.contains_key(&vpn) {
                    area.unmap_one(&mut vm_set.page_table, vpn);
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
            area.range_va_mut().start = VirtAddr::from(overlap_end);
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
        right.range_va_mut().start = VirtAddr::from(overlap_end);
        right.range_va_mut().end = VirtAddr::from(old_end);
        let right_start = right.start_vpn();
        let right_end = right.end_vpn();
        right
            .data_frames
            .retain(|vpn, _| *vpn >= right_start && *vpn < right_end);
        vm_set.areas.insert(idx + 1, right);
        idx += 2;
    }
}

pub fn sys_mmap(
    start: usize,
    len: usize,
    prot: usize,
    flags: usize,
    fd: usize,
    offset: usize,
) -> SyscallResult {
    const MAP_SHARED: usize = 0x01;
    const MAP_PRIVATE: usize = 0x02;
    const MAP_FIXED: usize = 0x10;
    const MAP_ANONYMOUS: usize = 0x20;

    if len == 0 {
        return Err(SysError::EINVAL);
    }
    if (flags & (MAP_SHARED | MAP_PRIVATE)) == 0
        || (flags & (MAP_SHARED | MAP_PRIVATE)) == (MAP_SHARED | MAP_PRIVATE)
    {
        return Err(SysError::EINVAL);
    }
    if (flags & MAP_FIXED) != 0 && (start & (PAGE_SIZE - 1)) != 0 {
        return Err(SysError::EINVAL);
    }
    if (flags & MAP_ANONYMOUS) == 0 && (offset & (PAGE_SIZE - 1)) != 0 {
        return Err(SysError::EINVAL);
    }

    let page_aligned_len = (len + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let end_req = match start.checked_add(page_aligned_len) {
        Some(v) => v,
        None => return Err(SysError::ENOMEM),
    };
    if end_req == 0 {
        return Err(SysError::ENOMEM);
    }

    let process = current_process();
    let mut inner = process.inner_exclusive_access();

    let target_start = if (flags & MAP_FIXED) != 0 {
        start
    } else {
        let hint = if start == 0 {
            0
        } else {
            start & !(PAGE_SIZE - 1)
        };
        match inner.vm_set.find_free_area(hint, page_aligned_len) {
            Some(addr) => addr,
            None => return Err(SysError::ENOMEM),
        }
    };
    let start_va = VirtAddr::from(target_start);
    let end_va = VirtAddr::from(target_start + page_aligned_len);
    let map_perm = MapPermission::from_prot(prot);

    if (flags & MAP_FIXED) != 0 {
        trim_mmap_range(&mut inner.vm_set, start_va.0, end_va.0);
    }

    if (flags & MAP_ANONYMOUS) != 0 {
        inner
            .vm_set
            .insert_framed_area(start_va, end_va, map_perm, UserMapAreaType::Mmap, None);
        if (flags & MAP_SHARED) != 0 {
            if let Some(area) = inner.vm_set.find_area(start_va) {
                area.flags = crate::mm::vm_area::MmapType::MapShared;
            }
        }
    } else {
        if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
            return Err(SysError::EBADF);
        }
        let file = inner.fd_table[fd].as_ref().unwrap().clone();
        inner.vm_set.insert_framed_area(
            start_va,
            end_va,
            map_perm,
            UserMapAreaType::Mmap,
            Some((file, offset, flags)),
        );
    }
    Ok(target_start)
}

pub fn sys_munmap(start: usize, len: usize) -> SyscallResult {
    if len == 0 || (start & (PAGE_SIZE - 1)) != 0 {
        return Err(SysError::EINVAL);
    }
    let page_aligned_len = (len + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let end = match start.checked_add(page_aligned_len) {
        Some(v) => v,
        None => return Err(SysError::EINVAL),
    };
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    trim_mmap_range(&mut inner.vm_set, start, end);
    Ok(0)
}

pub fn sys_madvice(_advice: usize) -> SyscallResult {
    Ok(0)
}

pub fn sys_mprotect(start: usize, len: usize, prot: usize) -> SyscallResult {
    if len == 0 {
        return Ok(0);
    }
    if (start & (PAGE_SIZE - 1)) != 0 {
        return Err(SysError::EINVAL);
    }
    let end = match start.checked_add(len) {
        Some(v) => v,
        None => return Err(SysError::EINVAL),
    };
    if end <= start {
        return Err(SysError::EINVAL);
    }

    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    let start_va = VirtAddr::from(start);
    let end_va = VirtAddr::from(end);
    let new_perm = MapPermission::from_prot(prot);
    let start_vpn = start_va.floor();
    let end_vpn = end_va.ceil();

    // 遍历所有 area，对与 mprotect 范围有重叠的 area 进行处理：
    // - 如果 mprotect 范围完全覆盖 area，直接更新 area 的权限
    // - 如果部分覆盖，需要拆分 area，只更新重叠部分的权限
    let mut i = 0;
    while i < inner.vm_set.areas.len() {
        let area_start_vpn = inner.vm_set.areas[i].start_vpn();
        let area_end_vpn = inner.vm_set.areas[i].end_vpn();
        
        // 检查是否有重叠
        if start_vpn < area_end_vpn && end_vpn > area_start_vpn {
            // 完全覆盖：直接更新权限
            if start_vpn <= area_start_vpn && end_vpn >= area_end_vpn {
                *inner.vm_set.areas[i].perm_mut() = new_perm;
                if !new_perm.contains(MapPermission::W) {
                    inner.vm_set.areas[i].clear_cow_flag();
                }
            } else {
                // 部分覆盖：需要拆分 area
                // 先处理左侧未覆盖部分（如果存在）
                if area_start_vpn < start_vpn {
                    let mut left_area = UserMapArea::from_another(&inner.vm_set.areas[i]);
                    left_area.range_va_mut().end = VirtAddr::from(start_vpn.0 * PAGE_SIZE);
                    // 清理左侧超出范围的 data_frames
                    let left_keep_start = left_area.start_vpn();
                    let left_keep_end = left_area.end_vpn();
                    left_area.data_frames.retain(|vpn, _| *vpn >= left_keep_start && *vpn < left_keep_end);
                    // 保留左侧的原始权限
                    inner.vm_set.areas.insert(i, left_area);
                    i += 1;
                    // 更新当前 area 的起始地址
                    inner.vm_set.areas[i].range_va_mut().start = VirtAddr::from(start_vpn.0 * PAGE_SIZE);
                }
                // 处理右侧未覆盖部分（如果存在）
                if area_end_vpn > end_vpn {
                    let mut right_area = UserMapArea::from_another(&inner.vm_set.areas[i]);
                    right_area.range_va_mut().start = VirtAddr::from(end_vpn.0 * PAGE_SIZE);
                    // 清理右侧超出范围的 data_frames
                    let right_keep_start = right_area.start_vpn();
                    let right_keep_end = right_area.end_vpn();
                    right_area.data_frames.retain(|vpn, _| *vpn >= right_keep_start && *vpn < right_keep_end);
                    // 保留右侧的原始权限
                    inner.vm_set.areas.insert(i + 1, right_area);
                    // 更新当前 area 的结束地址
                    inner.vm_set.areas[i].range_va_mut().end = VirtAddr::from(end_vpn.0 * PAGE_SIZE);
                }
                // 清理当前 area 超出范围的 data_frames
                let mid_keep_start = inner.vm_set.areas[i].start_vpn();
                let mid_keep_end = inner.vm_set.areas[i].end_vpn();
                inner.vm_set.areas[i].data_frames.retain(|vpn, _| *vpn >= mid_keep_start && *vpn < mid_keep_end);
                // 更新重叠部分的权限
                *inner.vm_set.areas[i].perm_mut() = new_perm;
                if !new_perm.contains(MapPermission::W) {
                    inner.vm_set.areas[i].clear_cow_flag();
                }
            }
        }
        i += 1;
    }

    // 更新已存在的 PTE
    for vpn in VPNRange::new(start_vpn, end_vpn) {
        if let Some(pte) = inner.vm_set.page_table.find_pte(vpn) {
            if pte.is_valid() {
                let new_flags = PTEFlags::from(MappingFlags::from(new_perm)) | PTEFlags::V;
                *pte = PTE::new(pte.ppn(), new_flags);
            }
        }
    }
    TLB::flush_all();
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

    let process = current_process();
    let inner = process.inner_exclusive_access();

    for area in inner.vm_set.areas.iter() {
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
            if let Some(inode) = file.get_inode() {
                let ino = inode.get_ino();
                let cache = PAGE_CACHE.lock();
                for (&vpn, _) in area.data_frames.iter() {
                    let page_va = vpn.0 * PAGE_SIZE;
                    if page_va < overlap_start || page_va >= overlap_end {
                        continue;
                    }
                    let offset_in_area = page_va - area_start;
                    let file_offset = area.file_offset + offset_in_area;
                    let page_id = file_offset / PAGE_SIZE;
                    if let Some(page_lock) = cache.get_page(ino, page_id) {
                        let mut page = page_lock.write();
                        page.dirty = true;
                    }
                }
                drop(cache);
                file.flush();
            }
        }
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
    if len == 0 {
        return Err(SysError::EINVAL);
    }
    // Validate alignment (optional in our simplified implementation)
    if (start & (PAGE_SIZE - 1)) != 0 {
        return Err(SysError::EINVAL);
    }
    // Check for overflow
    let _end = start.checked_add(len).ok_or(SysError::EINVAL)?;
    
    // In our OS, all memory is already locked (no swap support)
    Ok(0)
}
