use crate::error::{SysError, SyscallResult};
use crate::task::{
    add_task, alloc_pid_raw, current_task, insert_into_tid2task, kstack_alloc, TaskControlBlock,
};
use alloc::sync::Arc;
use core::mem::size_of;
use log::error;
use polyhal::println;
use polyhal_trap::trapframe::TrapFrame;
use polyhal_trap::trapframe::TrapFrameArgs;

pub fn sys_thread_create(entry: usize, arg: usize) -> SyscallResult {
    let task = current_task().unwrap();
    let process = task.process.upgrade().unwrap();

    // create a new thread
    let global_tid = alloc_pid_raw();
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
        global_tid,
    ));
    insert_into_tid2task(global_tid, Arc::clone(&new_task));
    // add new task to scheduler
    add_task(Arc::clone(&new_task));
    let mut new_task_inner = new_task.inner_exclusive_access();
    new_task_inner.blocked_signals = task.inner_exclusive_access().blocked_signals.clone();
    let new_task_res = new_task_inner.res.as_ref().unwrap();
    let new_task_tid = new_task_res.tid;
    let new_task_global_tid = new_task_res.global_tid;
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
    Ok(new_task_global_tid)
}

#[allow(unused)]
pub fn sys_gettid() -> SyscallResult {
    let task = current_task().unwrap();
    let global_tid = task.inner_exclusive_access().global_tid;
    error!("[DEBUG gettid] global_tid={}", global_tid);
    Ok(global_tid)
}

/// thread does not exist, return Err(SysError::ECHILD)
/// thread has not exited yet, return Err(SysError::EAGAIN)
/// otherwise, return thread's exit code
pub fn sys_waittid(tid: usize) -> SyscallResult {
    let task = current_task().unwrap();
    let process = task.process.upgrade().unwrap();
    let task_inner = task.inner_exclusive_access();
    // a thread cannot wait for itself
    if task_inner.global_tid == tid {
        return Err(SysError::ECHILD);
    }
    drop(task_inner);

    let target_task = match crate::task::tid2task(tid) {
        Some(t) => t,
        None => return Err(SysError::ECHILD),
    };
    // verify the target thread belongs to the same process
    let target_process = target_task.process.upgrade().unwrap();
    if target_process.getpid() != process.getpid() {
        return Err(SysError::ECHILD);
    }

    let (exit_code, global_tid) = {
        let t_inner = target_task.inner_exclusive_access();
        (t_inner.exit_code, t_inner.global_tid)
    };
    if let Some(code) = exit_code {
        // remove the exited thread from process.tasks
        let mut process_inner = process.inner_exclusive_access();
        for t_opt in process_inner.tasks.iter_mut() {
            if let Some(t) = t_opt {
                if Arc::ptr_eq(t, &target_task) {
                    *t_opt = None;
                    break;
                }
            }
        }
        drop(process_inner);
        // 回收全局 TID
        crate::task::remove_from_tid2task(global_tid);
        crate::task::dealloc_pid(global_tid);
        Ok(code as usize)
    } else {
        // waited thread has not exited
        Err(SysError::EAGAIN)
    }
}

pub fn sys_set_tid_address(tidptr: usize) -> SyscallResult {
    let task = crate::task::current_task().unwrap();
    let mut inner = task.inner_exclusive_access();
    inner.clear_child_tid = tidptr;
    let global_tid = inner.global_tid;
    drop(inner);
    error!("[DEBUG set_tid_address] global_tid={}", global_tid);
    Ok(global_tid)
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
        match crate::task::tid2task(pid) {
            Some(t) => t,
            None => return Err(SysError::ESRCH),
        }
    };

    let token = crate::task::current_user_token();
    let (head, len) = {
        let inner = task.inner_exclusive_access();
        (inner.robust_list_head, inner.robust_list_len)
    };
    let mut head_buf =
        crate::mm::translated_byte_buffer(token, head_ptr as *const u8, size_of::<usize>())?;
    if !head_buf.is_empty() && head_buf[0].len() >= size_of::<usize>() {
        head_buf[0][..size_of::<usize>()].copy_from_slice(&head.to_ne_bytes());
    }
    let mut len_buf =
        crate::mm::translated_byte_buffer(token, len_ptr as *const u8, size_of::<usize>())?;
    if !len_buf.is_empty() && len_buf[0].len() >= size_of::<usize>() {
        len_buf[0][..size_of::<usize>()].copy_from_slice(&len.to_ne_bytes());
    }
    Ok(0)
}

pub fn sys_exit_group(exit_code: i32) -> ! {
    let task = crate::task::current_task().unwrap();
    let process = task.process.upgrade().unwrap();

    // 1. 在持有 process 锁的情况下，标记进程状态并收集其他线程
    //    注意：不能在持有 process.inner 锁的同时获取 task.inner 锁，
    //    因为 exit_current_and_run_next 中会先获取 task.inner 再获取 process.inner，
    //    两个相反的锁顺序会导致死锁（AB-BA deadlock）。
    let other_tasks: alloc::vec::Vec<Arc<TaskControlBlock>> = {
        let mut inner = process.inner_exclusive_access();
        inner.is_zombie = true;
        inner.exit_code = exit_code;
        inner.term_status = crate::task::TermStatus::Exited(exit_code);
        inner
            .zombie_flag
            .store(true, core::sync::atomic::Ordering::SeqCst);

        inner
            .tasks
            .iter()
            .filter_map(|t| t.as_ref().map(Arc::clone))
            .filter(|t| !Arc::ptr_eq(t, &task))
            .collect()
    };

    // 2. 释放 process 锁后，再处理每个线程的 zombie_flag 和唤醒
    for t in other_tasks {
        let should_wake = {
            let t_inner = t.inner_exclusive_access();
            t_inner
                .zombie_flag
                .store(true, core::sync::atomic::Ordering::SeqCst);
            let is_blocked = t_inner.task_status == crate::task::TaskStatus::Blocked;
            drop(t_inner);
            is_blocked
        };
        if should_wake {
            crate::task::wakeup_task(t);
        }
    }

    crate::task::exit_current_and_run_next(exit_code);
    panic!("Unreachable in sys_exit_group!");
}
