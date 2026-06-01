use crate::error::{SysError, SyscallResult};
use crate::mm::exception::SetPageFaultException;
use crate::task::current_task;
use fatfs::warn;
// use crate::config::PAGE_SIZE;
use crate::fs::page::pagecache::PAGE_CACHE;
use crate::fs::tmpfs::inode::F_SEAL_WRITE;
use crate::mm::frame_alloc;
use crate::mm::vm_area::LazyAlloc;
use crate::mm::vm_area::MapArea;
use crate::mm::vm_set::VMSpace;
use crate::mm::{COW, MapPermission, MmapType, UserMapAreaType, UserVMSet};
use crate::mm::{UserMapArea, vm_set};
use crate::syscall::shm::release_shm_attaches;
use crate::task::current_process;
use alloc::sync::Arc;
use alloc::vec::Vec;
use log::info;
use log::log;
use polyhal::consts::PAGE_SIZE;
use polyhal::pagetable::*;
use polyhal::utils::addr::{VPNRange, VirtAddr, VirtPageNum};

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
    const MAP_FIXED_NOREPLACE: usize = 0x100000;
    const MAP_GROWSDOWN: usize = 0x00100;
    const MAP_POPULATE: usize = 0x2000;
    warn!(
        "sys_mmap: start: {}, len: {}, prot: {}, flags: {}, fd: {}, offset: {}",
        start, len, prot, flags, fd, offset
    );
    // 先检查 fd 是否有效
    let process = current_process();

    let mut inner = process.inner_exclusive_access();

    if (flags & MAP_ANONYMOUS) == 0 {
        // 需要文件描述符，但 fd 无效
        if fd == usize::MAX || fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
            info!("[DEBUG] sys_mmap: invalid fd={}", fd);
            return Err(SysError::EBADF);
        }
    }

    if len == 0 {
        return Err(SysError::EINVAL);
    }
    if (flags & (MAP_SHARED | MAP_PRIVATE)) == 0
    // || (flags & (MAP_SHARED | MAP_PRIVATE)) == (MAP_SHARED | MAP_PRIVATE)
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

    let target_start = if (flags & (MAP_FIXED | MAP_FIXED_NOREPLACE)) != 0 {
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

    // 检查 MAP_FIXED_NOREPLACE：如果地址范围已被占用，返回 EEXIST
    if (flags & MAP_FIXED_NOREPLACE) != 0 {
        for area in inner.vm_set.areas.iter() {
            let area_start = area.start_va().0;
            let area_end = area.end_va().0;
            if target_start < area_end && (target_start + page_aligned_len) > area_start {
                // 地址范围重叠
                return Err(SysError::EEXIST);
            }
        }
    } else if (flags & MAP_FIXED) != 0 {
        trim_mmap_range(&mut inner.vm_set, start_va.0, end_va.0);
    }

    if (flags & MAP_ANONYMOUS) != 0 {
        inner.vm_set.insert_framed_area(
            start_va,
            end_va,
            map_perm,
            UserMapAreaType::Mmap,
            Some((None, offset, flags)),
        );
        // 设置 MAP_GROWSDOWN 标志
        if let Some(area) = inner.vm_set.areas.last_mut() {
            if (flags & MAP_GROWSDOWN) != 0 {
                area.growdown_flag = true;
            }
        }

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

        // 添加文件类型检查：只有常规文件和设备文件才能被 mmap
        use crate::fs::vfs::inode::InodeMode;
        if let Some(inode) = file.get_inode() {
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
            if file_type == InodeMode::FILE.bits()
                || file_type == InodeMode::CHAR.bits()
                || file_type == InodeMode::BLOCK.bits()
            {
                // 普通文件或设备文件，允许 mmap
            } else {
                info!(
                    "[DEBUG] sys_mmap: cannot mmap this file type, mode={:o}",
                    mode.bits()
                );
                return Err(SysError::ENODEV);
            }
        }

        // 检查文件打开模式：mmap 需要读取文件内容，所以文件必须可读
        if !file.readable() {
            info!("[DEBUG] sys_mmap: file is not readable (O_WRONLY), cannot mmap");
            return Err(SysError::EACCES);
        }

        // 检查文件打开模式：如果文件只读打开，禁止写映射
        if (prot & PROT_WRITE) != 0 && !file.writable() {
            info!("[DEBUG] sys_mmap: file is not writable, cannot create write mapping");
            return Err(SysError::EACCES);
        }
        // 新增：检查 memfd seal: F_SEAL_WRITE 禁止写映射
        const PROT_WRITE: usize = 0x02;
        if (prot & PROT_WRITE) != 0 && (flags & MAP_SHARED) != 0 {
            if let Some(inode) = file.get_inode() {
                if (inode.get_seals() & F_SEAL_WRITE) != 0 {
                    return Err(SysError::EPERM);
                }
            }
        }
        inner.vm_set.insert_framed_area(
            start_va,
            end_va,
            map_perm,
            UserMapAreaType::Mmap,
            Some((Some(file), offset, flags)),
        );
        // 设置 MAP_GROWSDOWN 标志
        if let Some(area) = inner.vm_set.areas.last_mut() {
            if (flags & MAP_GROWSDOWN) != 0 {
                area.growdown_flag = true;
            }
        }
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
    let end = match addr.checked_add(len) {
        Some(v) => v,
        None => {
            info!("[DEBUG] sys_madvice: address overflow");
            return Err(SysError::EINVAL);
        }
    };

    // Check if address range is valid for this process
    let process = current_process();
    let inner = process.inner_exclusive_access();
    let start_va = VirtAddr::from(addr);
    let end_va = VirtAddr::from(end);

    let mut valid = false;
    for area in inner.vm_set.areas.iter() {
        if start_va >= area.start_va() && end_va <= area.end_va() {
            valid = true;
            break;
        }
    }

    if !valid {
        info!(
            "[DEBUG] sys_madvice: address range not in any VM area: {:#x}-{:#x}",
            addr, end
        );
        return Err(SysError::ENOMEM);
    }

    // Check for valid advice value
    // Note: madvise is advisory, so we accept all known advice values as no-op.
    // Only return EINVAL for truly unknown/invalid advice values.
    match advice {
        // POSIX standard values - supported (no-op for now)
        MADV_NORMAL | MADV_RANDOM | MADV_SEQUENTIAL | MADV_WILLNEED | MADV_DONTNEED |
        // Linux-specific values - accept as no-op (madvise is advisory)
        MADV_FREE | MADV_REMOVE | MADV_DONTFORK | MADV_DOFORK |
        MADV_MERGEABLE | MADV_UNMERGEABLE | MADV_HUGEPAGE | MADV_NOHUGEPAGE |
        MADV_DONTDUMP | MADV_DODUMP | MADV_WIPEONFORK | MADV_KEEPONFORK |
        MADV_COLLAPSE | MADV_PAGEOUT | MADV_HWPOISON => {
            // Accept all known advice values as no-op
            Ok(0)
        }
        // Unknown/invalid values - return EINVAL
        _ => {
            info!("[DEBUG] sys_madvice: invalid advice value {}", advice);
            Err(SysError::EINVAL)
        }
    }
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
                // 新增：检查 memfd seal
                if new_perm.contains(MapPermission::W) {
                    if let Some(file) = &inner.vm_set.areas[i].map_file {
                        if inner.vm_set.areas[i].flags == MmapType::MapShared {
                            if let Some(inode) = file.get_inode() {
                                if (inode.get_seals() & F_SEAL_WRITE) != 0 {
                                    return Err(SysError::EPERM);
                                }
                            }
                        }
                    }
                }
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
                    left_area
                        .data_frames
                        .retain(|vpn, _| *vpn >= left_keep_start && *vpn < left_keep_end);
                    // 保留左侧的原始权限
                    inner.vm_set.areas.insert(i, left_area);
                    i += 1;
                    // 更新当前 area 的起始地址
                    inner.vm_set.areas[i].range_va_mut().start =
                        VirtAddr::from(start_vpn.0 * PAGE_SIZE);
                }
                // 处理右侧未覆盖部分（如果存在）
                if area_end_vpn > end_vpn {
                    let mut right_area = UserMapArea::from_another(&inner.vm_set.areas[i]);
                    right_area.range_va_mut().start = VirtAddr::from(end_vpn.0 * PAGE_SIZE);
                    // 清理右侧超出范围的 data_frames
                    let right_keep_start = right_area.start_vpn();
                    let right_keep_end = right_area.end_vpn();
                    right_area
                        .data_frames
                        .retain(|vpn, _| *vpn >= right_keep_start && *vpn < right_keep_end);
                    // 保留右侧的原始权限
                    inner.vm_set.areas.insert(i + 1, right_area);
                    // 更新当前 area 的结束地址
                    inner.vm_set.areas[i].range_va_mut().end =
                        VirtAddr::from(end_vpn.0 * PAGE_SIZE);
                }
                // 清理当前 area 超出范围的 data_frames
                let mid_keep_start = inner.vm_set.areas[i].start_vpn();
                let mid_keep_end = inner.vm_set.areas[i].end_vpn();
                inner.vm_set.areas[i]
                    .data_frames
                    .retain(|vpn, _| *vpn >= mid_keep_start && *vpn < mid_keep_end);
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
    let mut files_to_flush = Vec::new();

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
            if let Some(ino) = file.cache_inode_id() {
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
                files_to_flush.push(file.clone());
            }
        }
    }

    drop(inner);
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
    let mut mut_inner = process.inner_exclusive_access();
    let vm_set = &mut mut_inner.vm_set;
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
    for area in vm_set.areas.iter_mut() {
        if start_va >= area.start_va() && end_va <= area.end_va() {
            if area.lazy_flag {
                for vpn in area.vpn_range() {
                    let frame = frame_alloc().unwrap();
                    area.data_frames.insert(vpn, Arc::new(frame));
                }
                area.clear_lazy_flag();

                let frames = area.data_frames.clone();

                for (vpn, frame) in frames {
                    vm_set.page_table.map_page(
                        vpn,
                        frame.ppn,
                        MappingFlags::from(*area.perm()),
                        MappingSize::Page4KB,
                    );
                }
            }
        }
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
    let vm_set = &inner.vm_set;
    // Check permissions: only root (euid=0) can munlock
    if inner.euid != 0 {
        return Err(SysError::EPERM);
    }
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
