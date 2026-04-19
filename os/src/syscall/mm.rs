use crate::config::PAGE_SIZE;
use crate::mm::vm_set::VMSpace;
use crate::mm::{MapPermission, UserMapAreaType, VirtAddr};
use crate::task::current_process;
use crate::task::current_task;

pub fn sys_mmap(
    start: usize,
    len: usize,
    prot: usize,
    flags: usize,
    fd: usize,
    offset: usize,
) -> isize {
    // println!("enter mmap");
    if len == 0 {
        return -1;
    }
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    let page_aligned_len = (len + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let target_start = if start == 0 {
        match inner.vm_set.find_free_area(0, page_aligned_len) {
            Some(addr) => addr,
            None => return -1,
        }
    } else {
        start
    };
    let start_va = VirtAddr::from(target_start);
    let end_va = VirtAddr::from(target_start + page_aligned_len);
    // println!("start:{:?} end:{:?}", start_va, end_va);
    let map_perm = MapPermission::from_prot(prot);
    const MAP_ANONYMOUS: usize = 0x20;
    if (flags & MAP_ANONYMOUS) != 0 {
        //匿名映射
        inner
            .vm_set
            .insert_framed_area(start_va, end_va, map_perm, UserMapAreaType::Mmap, None);
    } else {
        //文件映射
        if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
            return -1;
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

pub fn sys_munmap(start: usize, _len: usize) -> isize {
    if start % PAGE_SIZE != 0 {
        return -1;
    }
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    let start_vpn = VirtAddr::from(start).floor();
    inner.vm_set.remove_area_with_start_vpn(start_vpn);
    0
}

pub fn sys_madvice(_advice: usize) -> isize {
    0
}

pub fn sys_mprotect(_start: usize, _len: usize, _prot: usize) -> isize {
    // // 参数检查
    // if len == 0 {
    //     return -1;
    // }

    // // 检查地址是否页对齐
    // if start & (PAGE_SIZE - 1) != 0 {
    //     return -1;
    // }

    // let process = current_process();
    // let mut inner = process.inner_exclusive_access();

    // let start_va = VirtAddr::from(start);
    // let end_va = VirtAddr::from(start + len);

    // // 查找对应的内存区域
    // let areas = inner.vm_set.get_areas_in_range(start_va, end_va);

    // if areas.is_empty() {
    //     return -1; // 没有找到对应的内存区域
    // }

    // // 检查权限转换是否合法
    // let new_perm = MapPermission::from_prot(prot);

    // for area in areas {
    //     // 检查新权限是否与映射类型兼容
    //     // 例如：不能将文件映射的只读区域改为可写
    //     if !check_permission_compatible(&area, &new_perm) {
    //         return -1;
    //     }

    //     // 更新内存区域的权限
    //     if let Err(_) = inner.vm_set.mprotect_area(area, start_va, end_va, new_perm) {
    //         return -1;
    //     }
    // }

    0 // 成功返回 0
}