use crate::error::{SysError, SyscallResult};
use crate::task::{
    TaskControlBlock, add_task, alloc_pid_raw, current_task, insert_into_tid2task, kstack_alloc,
};
use alloc::sync::Arc;
use core::mem::size_of;
use log::debug;
use polyhal_trap::trapframe::TrapFrame;
use polyhal_trap::trapframe::TrapFrameArgs;

pub fn sys_thread_create(entry: usize, arg: usize) -> SyscallResult {
    let task = current_task().unwrap();
    let process = task.process.upgrade().unwrap();
    let (ustack_base, blocked_signals, comm) = {
        let inner = task.inner_exclusive_access();
        (
            inner.res.as_ref().unwrap().ustack_base,
            inner.blocked_signals,
            inner.comm,
        )
    };

    // create a new thread
    let global_tid = alloc_pid_raw();
    let kstack = kstack_alloc();
    let new_task = Arc::new(TaskControlBlock::new(
        Arc::clone(&process),
        ustack_base,
        true,
        kstack,
        global_tid,
    ));
    if task.sched_reset_on_fork() {
        new_task.set_sched(0, 0);
    } else {
        new_task.set_sched(task.sched_policy(), task.sched_priority());
    }
    new_task.set_sched_reset_on_fork(false);
    new_task.set_affinity_mask(task.affinity_mask());
    insert_into_tid2task(global_tid, Arc::clone(&new_task));
    // add new task to scheduler
    let (new_task_tid, new_task_global_tid) = {
        let mut new_task_inner = new_task.inner_exclusive_access();
        new_task_inner.comm = comm;
        new_task_inner.blocked_signals = blocked_signals;
        let new_task_res = new_task_inner.res.as_ref().unwrap();
        let new_task_tid = new_task_res.tid;
        let new_task_global_tid = new_task_res.global_tid;
        let new_task_ustack_top = new_task_res.ustack_top();
        let new_task_trap_cx = new_task_inner.get_trap_cx();
        *new_task_trap_cx = TrapFrame::new();
        new_task_trap_cx[TrapFrameArgs::SEPC] = entry;
        log::error!("set sp {:#x}", new_task_ustack_top);
        new_task_trap_cx[TrapFrameArgs::SP] = new_task_ustack_top;
        // (*new_task_trap_cx).x[10] = arg;
        new_task_trap_cx[TrapFrameArgs::ARG0] = arg;
        (new_task_tid, new_task_global_tid)
    };
    {
        let mut process_inner = process.inner_exclusive_access();
        // add new thread to current process
        let tasks = &mut process_inner.tasks;
        while tasks.len() < new_task_tid + 1 {
            tasks.push(None);
        }
        tasks[new_task_tid] = Some(Arc::clone(&new_task));
        process_inner.alive_thread_count += 1;
    }
    add_task(Arc::clone(&new_task));
    Ok(new_task_global_tid)
}

#[allow(unused)]
pub fn sys_gettid() -> SyscallResult {
    let task = current_task().unwrap();
    let global_tid = task.inner_exclusive_access().global_tid;
    debug!("[DEBUG gettid] global_tid={}", global_tid);
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
        crate::task::manager::remove_from_tid2task_if_present(global_tid);
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
    debug!("[DEBUG set_tid_address] global_tid={}", global_tid);
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
    if head_ptr.is_null() || len_ptr.is_null() {
        return Err(SysError::EFAULT);
    }
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
    crate::mm::copy_to_user(token, head_ptr as *mut u8, &head.to_ne_bytes())?;
    crate::mm::copy_to_user(token, len_ptr as *mut u8, &len.to_ne_bytes())?;
    Ok(0)
}

pub fn sys_exit_group(exit_code: i32) -> ! {
    let task = crate::task::current_task().unwrap();
    let process = task.process.upgrade().unwrap();
    #[cfg(target_arch = "loongarch64")]
    debug!(
        "[la64 exit] exit_group enter pid={} code={}",
        process.getpid(),
        exit_code
    );

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

    #[cfg(target_arch = "loongarch64")]
    debug!("[la64 exit] exit_group close files pid={}", process.getpid());
    process.close_all_files_on_exit();
    #[cfg(target_arch = "loongarch64")]
    debug!(
        "[la64 exit] exit_group close files done pid={}",
        process.getpid()
    );

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

    drop(process);
    drop(task);
    #[cfg(target_arch = "loongarch64")]
    debug!("[la64 exit] exit_group call exit_current code={}", exit_code);
    crate::task::exit_current_and_run_next(exit_code);
    panic!("Unreachable in sys_exit_group!");
}
