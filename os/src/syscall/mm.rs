use crate::task::current_task;
// use crate::config::PAGE_SIZE;
use polyhal::consts::PAGE_SIZE;

use crate::mm::UserMapArea;
use crate::mm::vm_area::MapArea;
use crate::mm::vm_set::VMSpace;
use crate::mm::{MapPermission, UserMapAreaType, UserVMSet};
use crate::task::current_process;
use polyhal::pagetable::*;
use polyhal::utils::addr::{VPNRange, VirtAddr};

fn trim_mmap_range(vm_set: &mut UserVMSet, start: usize, end: usize) {
    let mut idx = 0;
    while idx < vm_set.areas.len() {
        if vm_set.areas[idx].areatype() != UserMapAreaType::Mmap {
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
            vm_set.areas.remove(idx);
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
) -> isize {
    const EINVAL: isize = -22;
    const EBADF: isize = -9;
    const ENOMEM: isize = -12;
    const MAP_SHARED: usize = 0x01;
    const MAP_PRIVATE: usize = 0x02;
    const MAP_FIXED: usize = 0x10;
    const MAP_ANONYMOUS: usize = 0x20;

    if len == 0 {
        return EINVAL;
    }
    if (flags & (MAP_SHARED | MAP_PRIVATE)) == 0
        || (flags & (MAP_SHARED | MAP_PRIVATE)) == (MAP_SHARED | MAP_PRIVATE)
    {
        return EINVAL;
    }
    if (flags & MAP_FIXED) != 0 && (start & (PAGE_SIZE - 1)) != 0 {
        return EINVAL;
    }
    if (flags & MAP_ANONYMOUS) == 0 && (offset & (PAGE_SIZE - 1)) != 0 {
        return EINVAL;
    }

    let page_aligned_len = (len + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let end_req = match start.checked_add(page_aligned_len) {
        Some(v) => v,
        None => return ENOMEM,
    };
    if end_req == 0 {
        return ENOMEM;
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
            None => return ENOMEM,
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
    } else {
        if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
            return EBADF;
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
    target_start as isize
}

pub fn sys_munmap(start: usize, len: usize) -> isize {
    const EINVAL: isize = -22;
    if len == 0 || (start & (PAGE_SIZE - 1)) != 0 {
        return EINVAL;
    }
    let page_aligned_len = (len + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let end = match start.checked_add(page_aligned_len) {
        Some(v) => v,
        None => return EINVAL,
    };
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    trim_mmap_range(&mut inner.vm_set, start, end);
    0
}

pub fn sys_madvice(_advice: usize) -> isize {
    0
}

pub fn sys_mprotect(start: usize, len: usize, prot: usize) -> isize {
    const EINVAL: isize = -22;
    if len == 0 {
        return 0;
    }
    if (start & (PAGE_SIZE - 1)) != 0 {
        return EINVAL;
    }
    let end = match start.checked_add(len) {
        Some(v) => v,
        None => return EINVAL,
    };
    if end <= start {
        return EINVAL;
    }

    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    let start_va = VirtAddr::from(start);
    let end_va = VirtAddr::from(end);
    let new_perm = MapPermission::from_prot(prot);
    let start_vpn = start_va.floor();
    let end_vpn = end_va.ceil();

    // 仅在完全覆盖 area 时更新元数据，避免部分覆盖时把整个 area 都改掉。
    for area in inner.vm_set.areas.iter_mut() {
        let area_start_vpn = area.start_vpn();
        let area_end_vpn = area.end_vpn();
        if start_vpn <= area_start_vpn && end_vpn >= area_end_vpn {
            *area.perm_mut() = new_perm;
        }
    }

    for vpn in VPNRange::new(start_vpn, end_vpn) {
        if let Some(pte) = inner.vm_set.page_table.find_pte(vpn) {
            if pte.is_valid() {
                let new_flags = PTEFlags::from(MappingFlags::from(new_perm)) | PTEFlags::V;
                *pte = PTE::new(pte.ppn(), new_flags);
            }
        }
    }
    TLB::flush_all();
    0
}
