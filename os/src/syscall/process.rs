use core::task;

use crate::fs::{OpenFlags, open_file};
use crate::mm::vm_set;
use crate::mm::{translated_ref, translated_refmut, translated_str, vm_set::*, VMSpace, heap::HeapExt, address::*};
use crate::task::{
    current_process, current_task, current_user_token, exit_current_and_run_next, pid2process,
    suspend_current_and_run_next,
};
use crate::timer::get_time_ms;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use log::{error, warn};

pub fn sys_exit(exit_code: i32) -> ! {
    exit_current_and_run_next(exit_code);
    panic!("Unreachable in sys_exit!");
}

pub fn sys_yield() -> isize {
    suspend_current_and_run_next();
    0
}

pub fn sys_get_time() -> isize {
    get_time_ms() as isize
}

pub fn sys_getpid() -> isize {
    current_task().unwrap().process.upgrade().unwrap().getpid() as isize
}

pub fn sys_fork() -> isize {
    let current_process = current_process();
    let new_process = current_process.fork();
    let new_pid = new_process.getpid();
    // modify trap context of new_task, because it returns immediately after switching
    let new_process_inner = new_process.inner_exclusive_access();
    let task = new_process_inner.tasks[0].as_ref().unwrap();
    let trap_cx = task.inner_exclusive_access().get_trap_cx();
    // we do not have to move to next instruction since we have done it before
    // for child process, fork returns 0
    trap_cx.x[10] = 0;
    warn!(
        "fork a new process with pid {}, parent pid = {}",
        new_pid,
        current_process.getpid()
    );
    new_pid as isize
}

pub fn sys_exec(path: *const u8) -> isize {
    let token = current_user_token();
    let path = translated_str(token, path);

    if let Some(app_inode) = open_file(path.as_str(), OpenFlags::RDONLY) {
        let all_data = app_inode.read_all();
        let process = current_process();
        process.exec(all_data.as_slice());
        // return argc because cx.x[10] will be covered with it later
        0 as isize
    } else {
        -1
    }
}

pub fn sys_brk(ptr: *const i32) -> isize{

    let process = current_process();
    let vm_set = &mut process.inner_exclusive_access().vm_set;
    if ptr as usize == 0{
        return vm_set.heap_end_va().0 as isize   
    }
    let current_end_va = vm_set.heap_end_va();
    if current_end_va.0 == ptr as usize{
        return 0;
    }
    if current_end_va.0 < ptr as usize{
        vm_set.append_to(VirtAddr::from(ptr as usize));
    }else{
        vm_set.shrink_to(VirtAddr::from(ptr as usize));
    }
    0
}

/// If there is not a child process whose pid is same as given, return -1.
/// Else if there is a child process but it is still running, return -2.
pub fn sys_waitpid(pid: isize, exit_code_ptr: *mut i32) -> isize {
    let process = current_process();
    // find a child process

    let mut inner = process.inner_exclusive_access();
    if !inner
        .children
        .iter()
        .any(|p| pid == -1 || pid as usize == p.getpid())
    {
        return -1;
        // ---- release current PCB
    }
    let pair = inner.children.iter().enumerate().find(|(_, p)| {
        // ++++ temporarily access child PCB exclusively
        p.inner_exclusive_access().is_zombie && (pid == -1 || pid as usize == p.getpid())
        // ++++ release child PCB
    });
    if let Some((idx, _)) = pair {
        let child = inner.children.remove(idx);
        // confirm that child will be deallocated after being removed from children list
        //assert_eq!(Arc::strong_count(&child), 1);
        let found_pid = child.getpid();
        // ++++ temporarily access child PCB exclusively
        let exit_code = child.inner_exclusive_access().exit_code;
        // ++++ release child PCB
        *translated_refmut(inner.vm_set.token(), exit_code_ptr) = exit_code;
        found_pid as isize
    } else {
        -2
    }
    // ---- release current PCB automatically
}
