use crate::{
    task::{TaskControlBlock, add_task, current_task, kstack_alloc},
    trap::{TrapContext, trap_handler},
};
use alloc::sync::Arc;
use core::mem::size_of;

pub fn sys_thread_create(entry: usize, arg: usize) -> isize {
    let task = current_task().unwrap();
    let process = task.process.upgrade().unwrap();

    // create a new thread
    let kstack = kstack_alloc();
    let new_task = Arc::new(TaskControlBlock::new(
        Arc::clone(&process),
        task.inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .ustack_base,
        true,
        kstack,
    ));
    // add new task to scheduler
    add_task(Arc::clone(&new_task));
    let new_task_inner = new_task.inner_exclusive_access();
    let new_task_res = new_task_inner.res.as_ref().unwrap();
    let new_task_tid = new_task_res.tid;
    let mut process_inner = process.inner_exclusive_access();
    // add new thread to current process
    let tasks = &mut process_inner.tasks;
    while tasks.len() < new_task_tid + 1 {
        tasks.push(None);
    }
    tasks[new_task_tid] = Some(Arc::clone(&new_task));
    let new_task_trap_cx = new_task_inner.get_trap_cx();
    *new_task_trap_cx =
        TrapContext::app_init_context(entry, new_task_res.ustack_top(), new_task.kstack.0);
    (*new_task_trap_cx).x[10] = arg;
    new_task_tid as isize
}

#[allow(unused)]
pub fn sys_gettid() -> isize {
    current_task()
        .unwrap()
        .inner_exclusive_access()
        .res
        .as_ref()
        .unwrap()
        .tid as isize
}

/// thread does not exist, return -1
/// thread has not exited yet, return -2
/// otherwise, return thread's exit code
pub fn sys_waittid(tid: usize) -> i32 {
    let task = current_task().unwrap();
    let process = task.process.upgrade().unwrap();
    let task_inner = task.inner_exclusive_access();
    let mut process_inner = process.inner_exclusive_access();
    // a thread cannot wait for itself
    if task_inner.res.as_ref().unwrap().tid == tid {
        return -1;
    }
    let mut exit_code: Option<i32> = None;
    let waited_task = process_inner.tasks[tid].as_ref();
    if let Some(waited_task) = waited_task {
        if let Some(waited_exit_code) = waited_task.inner_exclusive_access().exit_code {
            exit_code = Some(waited_exit_code);
        }
    } else {
        // waited thread does not exist
        return -1;
    }
    if let Some(exit_code) = exit_code {
        // dealloc the exited thread
        process_inner.tasks[tid] = None;
        exit_code
    } else {
        // waited thread has not exited
        -2
    }
}

pub fn sys_set_tid_address(tidptr: usize) -> isize {
    let task = crate::task::current_task().unwrap();
    let mut inner = task.inner_exclusive_access();
    inner.clear_child_tid = tidptr;
    let tid = inner.res.as_ref().unwrap().tid;
    let process = task.process.upgrade().unwrap();
    let pid = process.getpid();
    drop(inner);
    
    if tid == 0 {
        // 如果是主线程，返回进程 PID
        pid as isize
    } else {
        tid as isize
    }
}

/// set_robust_list(2)
///
/// 当前内核未实现 futex robust-list 回收逻辑，先提供最小兼容：
/// - 参数基本校验
/// - 成功返回 0，避免用户态因 ENOSYS/unsupported 直接失败
pub fn sys_set_robust_list(head: usize, len: usize) -> isize {
    const EINVAL: isize = -22;
    // struct robust_list_head 在 rv64 上为 3 * usize = 24 字节。
    let expected_len = 3 * size_of::<usize>();
    if head == 0 || len != expected_len {
        return EINVAL;
    }
    0
}

pub fn sys_exit_group(exit_code: i32) -> ! {
    let task = crate::task::current_task().unwrap();
    let process = task.process.upgrade().unwrap();
    let mut inner = process.inner_exclusive_access();
    inner.is_zombie = true;
    inner.exit_code = exit_code;
    inner.fd_table.clear(); 
    drop(inner);
    crate::task::exit_current_and_run_next(exit_code);
    panic!("Unreachable in sys_exit_group!");
}
