use crate::config::PAGE_SIZE;
use crate::fs::{OpenFlags, open_file};
use crate::mm::{PageTable, PhysAddr};
pub use polyhal::utils::addr::*;

use crate::mm::{VMSpace, translated_ref, translated_refmut, translated_str};
use crate::task::{
    block_current_and_run_next, current_process, current_task, current_user_token,
    exit_current_and_run_next, pid2process, suspend_current_and_run_next,
};
use crate::timer::get_time_us;
use crate::trap::_set_sum_bit;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use log::{error, warn};

pub fn sys_exit(exit_code: i32) -> ! {
    exit_current_and_run_next(exit_code);
    panic!("Unreachable in sys_exit!");
}

pub fn sys_yield() -> isize {
    //println!("enter yield!");
    suspend_current_and_run_next();
    0
}

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

pub fn sys_get_time(_ts: *mut TimeVal, _tz: usize) -> isize {
    _set_sum_bit();
    let _ns = get_time_us();
    unsafe {
        *(_ts) = TimeVal {
            sec: _ns / 1_000_000,
            usec: _ns % 1_000_000,
        };
    }
    0
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

/// If there is not a child process whose pid is same as given, return -1.
/// Else if there is a child process but it is still running, return -2.
pub fn sys_waitpid(pid: isize, exit_code_ptr: *mut i32) -> isize {
    _set_sum_bit();
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
        let exit_code = {
            let child = &inner.children[idx];
            let child_inner = child.inner_exclusive_access();
            child_inner.exit_code
        };
        let child = inner.children.remove(idx);
        let found_pid = child.getpid();
        // confirm that child will be deallocated after being removed from children list
        //assert_eq!(Arc::strong_count(&child), 1);
        // ++++ release child PCB
        drop(inner);
        drop(process);
        unsafe {
            *exit_code_ptr = ((exit_code as i32) & 0xFF) << 8;
        }

        found_pid as isize
    } else {
        -2
    }
    // ---- release current PCB automatically
}
