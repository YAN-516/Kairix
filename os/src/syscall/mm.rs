use crate::mm::{MapPermission, VirtAddr};
use crate::task::current_task;
// use crate::config::PAGE_SIZE;
use polyhal::consts::PAGE_SIZE;

use crate::mm::vm_set::VMSpace;
use crate::task::current_process;
use crate::mm::UserMapAreaType;

pub fn sys_mmap(start: usize, len: usize, prot: usize, flags: usize, fd: usize, offset: usize) -> isize {
    if len == 0 { return -1; }
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
    let map_perm =MapPermission::from_prot(prot);
    const MAP_ANONYMOUS: usize = 0x20;
    if (flags & MAP_ANONYMOUS) != 0 {
        //匿名映射
        inner.vm_set.insert_framed_area(
            start_va, 
            end_va, 
            map_perm, 
            UserMapAreaType::Mmap, 
            None
        );
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
            Some((file, offset, flags))
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