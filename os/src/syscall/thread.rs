use crate::error::{SysError, SyscallResult};
use crate::task::{TaskControlBlock, add_task, current_task, kstack_alloc};
use alloc::sync::Arc;
use core::mem::size_of;
use polyhal::println;
use polyhal_trap::trapframe::TrapFrame;
use polyhal_trap::trapframe::TrapFrameArgs;

pub fn sys_thread_create(entry: usize, arg: usize) -> SyscallResult {
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
    let mut new_task_inner = new_task.inner_exclusive_access();
    new_task_inner.blocked_signals = task.inner_exclusive_access().blocked_signals.clone();
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
    *new_task_trap_cx = TrapFrame::new();
    new_task_trap_cx[TrapFrameArgs::SEPC] = entry;
    println!("set sp {:#x}", new_task_res.ustack_top());
    new_task_trap_cx[TrapFrameArgs::SP] = new_task_res.ustack_top();
    // TrapContext::app_init_context(entry, new_task_res.ustack_top(), new_task.kstack.0);
    // (*new_task_trap_cx).x[10] = arg;
    new_task_trap_cx[TrapFrameArgs::ARG0] = arg;
    Ok(new_task_tid)
}

#[allow(unused)]
pub fn sys_gettid() -> SyscallResult {
    let tid = current_task()
        .unwrap()
        .inner_exclusive_access()
        .res
        .as_ref()
        .unwrap()
        .tid;
    Ok(tid)
}

/// thread does not exist, return Err(SysError::ECHILD)
/// thread has not exited yet, return Err(SysError::EAGAIN)
/// otherwise, return thread's exit code
pub fn sys_waittid(tid: usize) -> SyscallResult {
    let task = current_task().unwrap();
    let process = task.process.upgrade().unwrap();
    let task_inner = task.inner_exclusive_access();
    let mut process_inner = process.inner_exclusive_access();
    // a thread cannot wait for itself
    if task_inner.res.as_ref().unwrap().tid == tid {
        return Err(SysError::ECHILD);
    }
    let mut exit_code: Option<i32> = None;
    let waited_task = process_inner.tasks[tid].as_ref();
    if let Some(waited_task) = waited_task {
        if let Some(waited_exit_code) = waited_task.inner_exclusive_access().exit_code {
            exit_code = Some(waited_exit_code);
        }
    } else {
        // waited thread does not exist
        return Err(SysError::ECHILD);
    }
    if let Some(exit_code) = exit_code {
        // dealloc the exited thread
        process_inner.tasks[tid] = None;
        Ok(exit_code as usize)
    } else {
        // waited thread has not exited
        Err(SysError::EAGAIN)
    }
}

pub fn sys_set_tid_address(tidptr: usize) -> SyscallResult {
    let task = crate::task::current_task().unwrap();
    let mut inner = task.inner_exclusive_access();
    inner.clear_child_tid = tidptr;
    let tid = inner.res.as_ref().unwrap().tid;
    let process = task.process.upgrade().unwrap();
    let pid = process.getpid();
    drop(inner);

    if tid == 0 {
        // 如果是主线程，返回进程 PID
        Ok(pid)
    } else {
        Ok(tid)
    }
}

/// set_robust_list(2)
pub fn sys_set_robust_list(head: usize, len: usize) -> SyscallResult {
    let expected_len = 3 * size_of::<usize>();
    if head == 0 || len != expected_len {
        return Err(SysError::EINVAL);
    }
    let task = crate::task::current_task().unwrap();
    let mut inner = task.inner_exclusive_access();
    inner.robust_list_head = head;
    inner.robust_list_len = len;
    Ok(0)
}

/// get_robust_list(2)
pub fn sys_get_robust_list(pid: usize, head_ptr: *mut usize, len_ptr: *mut usize) -> SyscallResult {
    let task = if pid == 0 {
        crate::task::current_task().unwrap()
    } else {
        // 查找指定 tid 的线程
        let current = crate::task::current_task().unwrap();
        let process = current.process.upgrade().unwrap();
        let inner = process.inner_exclusive_access();
        let target = inner.tasks.iter().find(|t| {
            if let Some(t) = t {
                t.inner_exclusive_access().res.as_ref().map(|r| r.tid) == Some(pid)
            } else {
                false
            }
        });
        match target {
            Some(Some(t)) => t.clone(),
            _ => return Err(SysError::ESRCH),
        }
    };

    let token = crate::task::current_user_token();
    let (head, len) = {
        let inner = task.inner_exclusive_access();
        (inner.robust_list_head, inner.robust_list_len)
    };
    let mut head_buf = crate::mm::translated_byte_buffer(token, head_ptr as *const u8, size_of::<usize>());
    if !head_buf.is_empty() && head_buf[0].len() >= size_of::<usize>() {
        head_buf[0][..size_of::<usize>()].copy_from_slice(&head.to_ne_bytes());
    }
    let mut len_buf = crate::mm::translated_byte_buffer(token, len_ptr as *const u8, size_of::<usize>());
    if !len_buf.is_empty() && len_buf[0].len() >= size_of::<usize>() {
        len_buf[0][..size_of::<usize>()].copy_from_slice(&len.to_ne_bytes());
    }
    Ok(0)
}

pub fn sys_exit_group(exit_code: i32) -> ! {
    let task = crate::task::current_task().unwrap();
    let process = task.process.upgrade().unwrap();
    // let pid = process.getpid();
    // let tid = task.inner_exclusive_access().res.as_ref().unwrap().tid;
    // println!("[DEBUG] sys_exit_group pid={} tid={} exit_code={}", pid, tid, exit_code);
    let mut inner = process.inner_exclusive_access();
    inner.is_zombie = true;
    inner.exit_code = exit_code;
    // 不要在这里清空 fd_table，exit_current_and_run_next 会负责回收
    drop(inner);
    crate::task::exit_current_and_run_next(exit_code);
    panic!("Unreachable in sys_exit_group!");
}
