// mod context;
mod id;
pub mod manager;
pub mod process;
pub mod processor;
use log::{info, log};
use polyhal::consts::VIRT_ADDR_START;
use polyhal::{print, println};
// mod switch;
pub mod signal;
// mod switch;
#[allow(clippy::module_inception)]
#[allow(rustdoc::private_intra_doc_links)]
pub mod task;
use self::id::TaskUserRes;
use crate::mm::vm_set::VMSpace;
use crate::timer::set_next_trigger;
use crate::trap::disable_timer_interrupt;
use polyhal::VirtAddr;
// #[cfg(target_arch = "riscv64")]
// use crate::sbi::shutdown;
// #[cfg(target_arch = "loongarch64")]
// use crate::sbi_la::shutdown;
use crate::socket::SOCKET_MANAGER;
use crate::syscall::shm::release_shm_attaches;
use alloc::{
    sync::{Arc, Weak},
    vec::Vec,
};
use polyhal::instruction::shutdown;
// pub use context::TaskContext;
use crate::handle_signals;
#[cfg(target_arch = "riscv64")]
use crate::sbi::get_tp;
#[cfg(target_arch = "loongarch64")]
use crate::sbi_la::get_tp;
pub(crate) use id::print_oom_snapshot;
pub use id::{
    IDLE_PID, KernelStack, PidHandle, alloc_pid_raw, dealloc_pid, kstack_alloc, pid_alloc,
};
use lazy_static::*;
use log::error;
use manager::fetch_task;
pub use manager::{
    add_task, add_task_to_cpu, add_task_to_cpu_front, all_processes, insert_into_tid2task,
    num_processes, pid2process, processes_in_pgrp, remove_from_pid2process, remove_from_tid2task,
    remove_task, tid2task, wakeup_task,
};
pub use process::{
    CLONE_FS, CLONE_INTO_CGROUP, CLONE_NEWNET, CLONE_NEWNS, CLONE_NEWPID, CLONE_PIDFD,
    CLONE_SIGHAND, CLONE_THREAD, CLONE_VFORK, CLONE_VM, ProcessControlBlock, RLIMIT_FSIZE,
    RLIMIT_NOFILE, Rlimit64, TermStatus, Tms,
};
pub use processor::{
    current_kstack_top, current_process, current_task, current_trap_cx, current_trap_cx_user_va,
    current_user_token, init_processors, run_tasks, schedule, take_current_task,
};
// use switch::__switch;
use alloc::collections::BTreeMap;
use polyhal::kcontext::*;
use polyhal::timer::current_time;
use polyhal_trap::trap::*;
use polyhal_trap::trapframe::*;
use spin::Mutex;
pub use task::{TaskControlBlock, TaskStatus};
static TIMER_QUEUE: Mutex<BTreeMap<u128, Vec<Arc<TaskControlBlock>>>> = Mutex::new(BTreeMap::new());

lazy_static! {
    static ref DEFERRED_EXITED_TASKS: Mutex<Vec<usize>> = Mutex::new(Vec::new());
}

fn defer_drop_exited_task(task: Arc<TaskControlBlock>) {
    DEFERRED_EXITED_TASKS
        .lock()
        .push(Arc::into_raw(task) as usize);
}

pub(crate) fn reap_deferred_exited_tasks() {
    let tasks = {
        let mut deferred = DEFERRED_EXITED_TASKS.lock();
        core::mem::take(&mut *deferred)
    };
    for task in tasks {
        unsafe {
            drop(Arc::from_raw(task as *const TaskControlBlock));
        }
    }
}

/// Return the number of exited task handles waiting for deferred drop.
pub(crate) fn deferred_exited_task_count() -> usize {
    DEFERRED_EXITED_TASKS.lock().len()
}

/// Snapshot used by OOM diagnostics to locate retained task/kernel-stack owners.
pub(crate) struct TaskRetentionStats {
    pub processes: usize,
    pub locked_processes: usize,
    pub zombie_processes: usize,
    pub child_refs: usize,
    pub max_child_refs: usize,
    pub max_child_refs_pid: usize,
    pub task_slots: usize,
    pub zombie_task_slots: usize,
    pub max_task_slots: usize,
    pub max_task_slots_pid: usize,
    pub ready_queue_tasks: usize,
    pub timer_queue_tasks: usize,
    pub timer_queue_lock_busy: bool,
}

/// Collect coarse ownership stats for task/kstack retention debugging.
pub(crate) fn task_retention_stats() -> TaskRetentionStats {
    let processes = all_processes();
    let mut locked_processes = 0usize;
    let mut zombie_processes = 0usize;
    let mut child_refs = 0usize;
    let mut max_child_refs = 0usize;
    let mut max_child_refs_pid = 0usize;
    let mut task_slots = 0usize;
    let mut zombie_task_slots = 0usize;
    let mut max_task_slots = 0usize;
    let mut max_task_slots_pid = 0usize;
    for process in processes.iter() {
        let Some(inner) = process.try_inner_exclusive_access() else {
            locked_processes += 1;
            continue;
        };
        let pid = process.getpid();
        if inner.is_zombie {
            zombie_processes += 1;
        }
        let process_child_refs = inner.children.len();
        child_refs += process_child_refs;
        if process_child_refs > max_child_refs {
            max_child_refs = process_child_refs;
            max_child_refs_pid = pid;
        }
        let mut process_task_slots = 0usize;
        for task in inner.tasks.iter().flatten() {
            process_task_slots += 1;
            task_slots += 1;
            if task
                .try_inner_exclusive_access()
                .is_some_and(|task| task.task_status == TaskStatus::Zombie)
            {
                zombie_task_slots += 1;
            }
        }
        if process_task_slots > max_task_slots {
            max_task_slots = process_task_slots;
            max_task_slots_pid = pid;
        }
    }
    let (timer_queue_tasks, timer_queue_lock_busy) = if let Some(queue) = TIMER_QUEUE.try_lock() {
        (
            queue.values().map(|tasks| tasks.len()).sum::<usize>(),
            false,
        )
    } else {
        (0, true)
    };
    TaskRetentionStats {
        processes: processes.len(),
        locked_processes,
        zombie_processes,
        child_refs,
        max_child_refs,
        max_child_refs_pid,
        task_slots,
        zombie_task_slots,
        max_task_slots,
        max_task_slots_pid,
        ready_queue_tasks: crate::task::manager::queuelength(),
        timer_queue_tasks,
        timer_queue_lock_busy,
    }
}

pub fn add_timer(task: Arc<TaskControlBlock>, wakeup_time: u128) {
    // info!("add_timer {}", wakeup_time);
    TIMER_QUEUE
        .lock()
        .entry(wakeup_time)
        .or_insert_with(Vec::new)
        .push(task);
}

pub fn check_timers() {
    let now = current_time().as_nanos();
    let mut queue = TIMER_QUEUE.lock();
    let expired: Vec<_> = queue.range(..=now).map(|(&time, _)| time).collect();

    // 先收集过期任务，然后释放 TIMER_QUEUE 锁
    let mut tasks_to_wake: Vec<Arc<TaskControlBlock>> = Vec::new();
    for time in expired {
        if let Some(tasks) = queue.remove(&time) {
            tasks_to_wake.extend(tasks);
        }
    }
    drop(queue); // 释放 TIMER_QUEUE 锁

    // 现在再唤醒任务，避免持有 TIMER_QUEUE 锁时获取 TASK_MANAGER 锁
    for task in tasks_to_wake {
        wakeup_task(task);
    }
}

pub(crate) fn remove_task_from_timer_queue(task: &Arc<TaskControlBlock>) {
    let mut queue = TIMER_QUEUE.lock();
    let task_ptr = Arc::as_ptr(task);
    let keys: Vec<_> = queue.keys().copied().collect();
    for key in keys {
        let should_remove = if let Some(tasks) = queue.get_mut(&key) {
            tasks.retain(|queued| Arc::as_ptr(queued) != task_ptr);
            tasks.is_empty()
        } else {
            false
        };
        if should_remove {
            queue.remove(&key);
        }
    }
}

fn current_cpu() -> usize {
    let cpu = get_tp();
    if cpu < crate::config::MAX_CPU_NUM {
        cpu
    } else {
        0
    }
}

#[allow(unused)]
fn handle_pending_signals(ctx: &mut TrapFrame) {
    handle_signals(ctx);
}

enum CurrentTaskExitState {
    Alive,
    ProcessZombie(i32),
    Orphan,
}

fn current_task_exit_state(task: &Arc<TaskControlBlock>) -> CurrentTaskExitState {
    let Some(process) = task.process.upgrade() else {
        return CurrentTaskExitState::Orphan;
    };
    let inner = process.inner_exclusive_access();
    if inner.is_zombie {
        CurrentTaskExitState::ProcessZombie(inner.exit_code)
    } else {
        CurrentTaskExitState::Alive
    }
}

fn finish_current_zombie_task(task: Arc<TaskControlBlock>) {
    let (task_cx_ptr, exit_code) = {
        let mut task_inner = task.inner_exclusive_access();
        (
            &mut task_inner.task_cx as *mut KContext,
            task_inner.exit_code.unwrap_or(-1),
        )
    };
    if task.process.upgrade().is_some() {
        crate::task::processor::set_current_task(task);
        exit_current_and_run_next(exit_code);
    } else {
        defer_drop_exited_task(task);
        schedule(task_cx_ptr);
    }
}

fn task_entry() {
    // log::trace!("os::task::task_entry");
    //println!("task_entry");
    let task = {
        let current_task = current_task().unwrap();
        current_task
            .process
            .upgrade()
            .unwrap()
            .inner_exclusive_access()
            .vm_set
            .activate();
        current_task.inner_exclusive_access().get_trap_cx() as *mut TrapFrame
    };
    // run_user_task_forever(unsafe { task.as_mut().unwrap() })
    let ctx_mut = unsafe { task.as_mut().unwrap() };

    loop {
        if let Some(process) = crate::task::current_task()
            .and_then(|task| task.process.upgrade())
            .filter(|process| process.inner_exclusive_access().is_zombie)
        {
            let exit_code = process.inner_exclusive_access().exit_code;
            exit_current_and_run_next(exit_code);
        }
        run_user_task(ctx_mut);
    }
}

#[allow(missing_docs)]
pub fn suspend_current_and_run_next() {
    // error!("suspend");
    // There must be an application running.
    let task = take_current_task();
    if let Some(task) = task {
        let cpu = current_cpu();
        {
            let task_inner = task.inner_exclusive_access();
            if task_inner.task_status == TaskStatus::Zombie {
                drop(task_inner);
                finish_current_zombie_task(task);
                return;
            }
        }
        match current_task_exit_state(&task) {
            CurrentTaskExitState::ProcessZombie(exit_code) => {
                crate::task::processor::set_current_task(task);
                exit_current_and_run_next(exit_code);
                return;
            }
            CurrentTaskExitState::Orphan => {
                let mut task_inner = task.inner_exclusive_access();
                let task_cx_ptr = &mut task_inner.task_cx as *mut KContext;
                task_inner.task_status = TaskStatus::Zombie;
                drop(task_inner);
                defer_drop_exited_task(task);
                schedule(task_cx_ptr);
                return;
            }
            CurrentTaskExitState::Alive => {}
        }
        // ---- access current TCB exclusively
        let mut task_inner = task.inner_exclusive_access();
        let task_cx_ptr = &mut task_inner.task_cx as *mut KContext;
        // Change status to Ready
        task_inner.task_status = TaskStatus::Ready;
        drop(task_inner);
        // ---- release current TCB

        // push back to ready queue.
        add_task_to_cpu(task, cpu);
        // jump to scheduling cycle
        schedule(task_cx_ptr);
    } else {
        // no task is running, just fetch one from ready queue and run it.
    }
}

pub fn first_current_and_run_next() {
    // error!("suspend");
    // There must be an application running.
    let task = take_current_task();
    if let Some(task) = task {
        let cpu = current_cpu();
        {
            let task_inner = task.inner_exclusive_access();
            if task_inner.task_status == TaskStatus::Zombie {
                drop(task_inner);
                finish_current_zombie_task(task);
                return;
            }
        }
        match current_task_exit_state(&task) {
            CurrentTaskExitState::ProcessZombie(exit_code) => {
                crate::task::processor::set_current_task(task);
                exit_current_and_run_next(exit_code);
                return;
            }
            CurrentTaskExitState::Orphan => {
                let mut task_inner = task.inner_exclusive_access();
                let task_cx_ptr = &mut task_inner.task_cx as *mut KContext;
                task_inner.task_status = TaskStatus::Zombie;
                drop(task_inner);
                defer_drop_exited_task(task);
                schedule(task_cx_ptr);
                return;
            }
            CurrentTaskExitState::Alive => {}
        }
        // ---- access current TCB exclusively
        let mut task_inner = task.inner_exclusive_access();
        let task_cx_ptr = &mut task_inner.task_cx as *mut KContext;
        // Change status to Ready
        task_inner.task_status = TaskStatus::Ready;
        drop(task_inner);
        // ---- release current TCB

        // push back to ready queue.
        add_task_to_cpu_front(task, cpu);
        // jump to scheduling cycle
        schedule(task_cx_ptr);
    } else {
        // no task is running, just fetch one from ready queue and run it.
    }
}
#[allow(missing_docs)]
pub fn block_current_and_run_next() {
    let task = take_current_task().unwrap();
    let mut task_inner = task.inner_exclusive_access();
    let task_cx_ptr = &mut task_inner.task_cx as *mut KContext;
    if task_inner.task_status == TaskStatus::Running {
        task_inner.task_status = TaskStatus::Blocked;
    }
    // 关键修复：在持有 task 锁时检查 zombie_flag。
    // 如果进程已被 SIGKILL 等标记为 zombie，直接返回不阻塞，
    // 避免在释放 task 锁后发生竞态导致永远阻塞。
    if task_inner
        .zombie_flag
        .load(core::sync::atomic::Ordering::SeqCst)
    {
        task_inner.task_status = TaskStatus::Running;
        drop(task_inner);
        // 将任务重新放回当前 CPU，避免后续 current_task() 返回 None
        crate::task::processor::set_current_task(task);
        return;
    }
    // 关键修复：检查是否有已到达但未处理的唤醒（lost wakeup race）。
    // 如果其他 CPU 在我们加入等待队列后、调用 schedule 前发了唤醒，
    // wakeup_task 会设置此标志。此时我们不应阻塞，而是直接返回让调用者重试。
    if task_inner.pending_wakeup {
        task_inner.pending_wakeup = false;
        task_inner.task_status = TaskStatus::Running;
        drop(task_inner);
        crate::task::processor::set_current_task(task);
        return;
    }
    drop(task_inner);
    schedule(task_cx_ptr);
}

/// Exit the current 'Running' task and run the next task in task list.
pub fn exit_current_and_run_next(exit_code: i32) {
    disable_timer_interrupt();
    let task = take_current_task().unwrap();
    let mut task_inner = task.inner_exclusive_access();
    let process_opt = task.process.upgrade();
    let pid_for_log = process_opt.as_ref().map(|process| process.getpid());
    let tid = task_inner.res.as_ref().map(|r| r.tid).unwrap_or(0);
    let global_tid = task_inner.global_tid;
    // record exit code
    task_inner.exit_code = Some(exit_code);
    task_inner.task_status = TaskStatus::Zombie;
    info!(
        "exit_current_and_run_next: tid={} exit_code={}",
        tid, exit_code
    );
    // 先收集需要的信息，然后释放 task_inner，避免 task.inner -> process.inner 的锁顺序
    // 与 sys_exit_group / _clone 等 process.inner -> task.inner 的路径形成死锁。
    let (clear_child_tid, robust_list_head, robust_list_len, auto_reap_on_exit) = {
        (
            task_inner.clear_child_tid,
            task_inner.robust_list_head,
            task_inner.robust_list_len,
            task_inner.auto_reap_on_exit,
        )
    };
    drop(task_inner);
    let auto_reap_thread = tid != 0 && (auto_reap_on_exit || clear_child_tid != 0);

    // pthread exits are reported through clear_child_tid/futex rather than waittid.
    // Remove the lookup entry early, but keep the global tid allocated until the
    // TCB can be dropped; otherwise a later thread could reuse the same id while
    // this exited task is still kept alive by process.tasks for its kernel stack.
    if tid == 0 {
        remove_from_tid2task(global_tid);
    } else if auto_reap_thread {
        crate::task::manager::remove_from_tid2task_if_present(global_tid);
    }
    remove_task_from_timer_queue(&task);
    crate::syscall::futex::remove_task_from_futex_table(&task);

    if let Some(process) = process_opt.as_ref() {
        let pid = process.getpid();
        if clear_child_tid != 0 {
            let process_inner = process.inner_exclusive_access();
            let page_table = &process_inner.vm_set.page_table;
            let vpn = VirtAddr::from(clear_child_tid).floor();
            let mut paddr = None;
            if let Some(pte) = page_table.translate(vpn) {
                if pte.is_valid() && pte.writable() {
                    let phys_addr = (pte.ppn().0 << 12) + (clear_child_tid % 4096);
                    let kernel_va = phys_addr + VIRT_ADDR_START;
                    unsafe {
                        *(kernel_va as *mut u32) = 0;
                    }
                    paddr = Some(phys_addr);
                }
            }
            drop(process_inner);

            // 唤醒可能正在等待 clear_child_tid 的线程
            // 传入物理地址，以便匹配未带 FUTEX_PRIVATE_FLAG 的 futex wait（Shared key）
            crate::syscall::futex::futex_wake_one(clear_child_tid, pid, paddr);
        }

        // // 从所有 cgroup 中移除该进程
        // {
        //     let mut table = crate::fs::cgroup2::CGROUP_TABLE.lock();
        //     for pids in table.values_mut() {
        //         pids.retain(|&p| p != pid);
        //     }
        //     table.retain(|_, pids| !pids.is_empty());
        // }

        // 处理 robust mutex list
        if robust_list_head != 0 {
            let process_inner = process.inner_exclusive_access();
            let token = process_inner.vm_set.token();
            drop(process_inner);
            crate::syscall::futex::handle_robust_list_exit(
                &task,
                tid,
                token,
                pid,
                robust_list_head,
                robust_list_len,
            );
        }
    }

    // Keep TaskUserRes alive until waittid or process cleanup removes the TCB.
    // Dropping it here would recycle the local tid before this CPU switches off
    // the exiting task's kernel stack, allowing another hart to overwrite the
    // process.tasks slot and drop the current KernelStack too early.
    {
        let mut task_inner = task.inner_exclusive_access();
        task_inner.task_status = TaskStatus::Zombie;
    }
    // however, if this is the main thread of current process
    // the process should terminate at once
    let mut should_wake_parent = false;
    let mut detach_exited_task = false;
    let mut dealloc_detached_global_tid = false;
    if let Some(process) = process_opt {
        let pid = process.getpid();
        if tid == 0 {
            if pid == IDLE_PID {
                println!(
                    "[kernel] Idle process exit with exit_code {} ...",
                    exit_code
                );
                shutdown();
            }
            let mut process_inner = process.inner_exclusive_access();
            process_inner.is_zombie = true;
            process_inner.exit_code = exit_code;
            if matches!(
                process_inner.term_status,
                crate::task::process::TermStatus::Running
            ) {
                process_inner.term_status = crate::task::process::TermStatus::Exited(exit_code);
            }
            info!(
                "[DEBUG] pid={} marked zombie=true exit_code={} term_status={:?}",
                pid, exit_code, process_inner.term_status
            );
            process_inner
                .zombie_flag
                .store(true, core::sync::atomic::Ordering::SeqCst);
            let mut should_wake_init = false;
            let children = process_inner.children.clone();
            let tasks_to_notify: Vec<Arc<TaskControlBlock>> = process_inner
                .tasks
                .iter()
                .filter_map(|task| task.as_ref().map(Arc::clone))
                .collect();
            drop(process_inner);

            process.close_all_files_on_exit();

            if pid != 1 {
                let mut adopted_children = Vec::new();
                for child in children {
                    let mut child_inner = child.inner_exclusive_access();
                    // 只重新 parent 那些 parent 确实指向当前进程的子进程
                    // (CLONE_PARENT 创建的子进程 parent 指向祖父进程，不应被修改)
                    if let Some(ref weak) = child_inner.parent {
                        if let Some(actual_parent) = weak.upgrade() {
                            if actual_parent.getpid() == pid {
                                child_inner.parent = Some(Arc::downgrade(&INITPROC));
                                if child_inner.is_zombie && child_inner.alive_thread_count == 0 {
                                    should_wake_init = true;
                                }
                                adopted_children.push(child.clone());
                            }
                        }
                    }
                }
                if !adopted_children.is_empty() {
                    INITPROC
                        .inner_exclusive_access()
                        .children
                        .extend(adopted_children);
                }
            }

            for task in tasks_to_notify {
                let (task_global_tid, should_wake) = {
                    let task_inner = task.inner_exclusive_access();
                    task_inner
                        .zombie_flag
                        .store(true, core::sync::atomic::Ordering::SeqCst);
                    (
                        task_inner.global_tid,
                        task_inner.task_status == TaskStatus::Blocked,
                    )
                };
                if task_global_tid == global_tid {
                    continue;
                }
                if should_wake {
                    wakeup_task(task);
                }
            }
            if should_wake_init {
                wake_blocked_waiter(&INITPROC);
            }
        }

        // 减少 alive_thread_count，如果变为 0 则通知父进程
        let mut process_inner = process.inner_exclusive_access();
        let detach_now = auto_reap_thread || process_inner.is_zombie;
        let alive_before = process_inner.alive_thread_count;
        let task_slots_before = process_inner.tasks.iter().flatten().count();
        let zombie_task_slots_before = process_inner
            .tasks
            .iter()
            .flatten()
            .filter(|task| task.inner_exclusive_access().task_status == TaskStatus::Zombie)
            .count();
        let child_refs = process_inner.children.len();
        if detach_now {
            if tid < process_inner.tasks.len() {
                process_inner.tasks[tid] = None;
            }
            detach_exited_task = true;
            dealloc_detached_global_tid = tid != 0 && !auto_reap_thread;
        }
        if process_inner.alive_thread_count > 0 {
            process_inner.alive_thread_count -= 1;
        }
        info!(
            "[DEBUG] pid={} tid={} exit, alive_thread_count={}",
            pid, tid, process_inner.alive_thread_count
        );
        log::debug!(
            "[TASK_RETAIN exit] pid={} tid={} global_tid={} auto_reap={} process_zombie={} detach_now={} detached={} alive_before={} alive_after={} task_slots_before={} zombie_task_slots_before={} child_refs={} task_strong_count={}",
            pid,
            tid,
            global_tid,
            auto_reap_thread,
            process_inner.is_zombie,
            detach_now,
            detach_exited_task,
            alive_before,
            process_inner.alive_thread_count,
            task_slots_before,
            zombie_task_slots_before,
            child_refs,
            Arc::strong_count(&task)
        );
        if process_inner.is_zombie && process_inner.alive_thread_count == 0 {
            should_wake_parent = true;
        }
        drop(process_inner);

        if should_wake_parent {
            process.close_all_files_on_exit();
            process.release_user_space_on_exit();
            let (parent_weak, exit_signal, vfork_parent_task) = {
                let mut process_inner = process.inner_exclusive_access();
                (
                    process_inner.parent.clone(),
                    process_inner.exit_signal,
                    process_inner.vfork_parent.take(),
                )
            };
            if let Some(parent) = parent_weak.and_then(|w| w.upgrade()) {
                if let Some(signal) = crate::task::signal::Signal::from_i32(exit_signal) {
                    crate::syscall::signal::deliver_signal(&parent, signal);
                }
                let parent_tasks: Vec<Arc<TaskControlBlock>> = {
                    let p_inner = parent.inner_exclusive_access();
                    p_inner
                        .tasks
                        .iter()
                        .filter_map(|task| task.as_ref().map(Arc::clone))
                        .collect()
                };
                let mut found_blocked = false;
                for task in parent_tasks {
                    let t_inner = task.inner_exclusive_access();
                    let status = t_inner.task_status;
                    error!(
                        "[DEBUG exit_current_and_run_next] parent task status={:?}",
                        status
                    );
                    let should_wake = status != crate::task::TaskStatus::Zombie;
                    if status == crate::task::TaskStatus::Blocked {
                        found_blocked = true;
                    }
                    drop(t_inner);
                    if should_wake {
                        crate::task::wakeup_task(task);
                    }
                }
                error!(
                    "[DEBUG exit_current_and_run_next] found_blocked={}",
                    found_blocked
                );
            } else {
                error!("[DEBUG exit_current_and_run_next] parent upgrade failed!");
            }
            // 唤醒 CLONE_VFORK 挂起的父任务
            if let Some(vfork_parent_task) = vfork_parent_task {
                let t_inner = vfork_parent_task.inner_exclusive_access();
                if t_inner.task_status == crate::task::TaskStatus::Blocked {
                    drop(t_inner);
                    crate::task::wakeup_task(vfork_parent_task);
                }
            }
        }
        drop(process);
    }

    if dealloc_detached_global_tid {
        crate::task::manager::remove_from_tid2task_if_present(global_tid);
        dealloc_pid(global_tid);
    }
    if auto_reap_thread {
        dealloc_pid(global_tid);
    }

    if detach_exited_task {
        log::debug!(
            "[TASK_RETAIN defer_drop] pid={:?} tid={} global_tid={} strong_count_before_defer={}",
            pid_for_log,
            tid,
            global_tid,
            Arc::strong_count(&task)
        );
        defer_drop_exited_task(task);
    } else {
        log::debug!(
            "[TASK_RETAIN drop_or_keep] tid={} global_tid={} detached=false strong_count_before_drop={}",
            tid,
            global_tid,
            Arc::strong_count(&task)
        );
        drop(task);
    }
    info!("exit_current_and_run_next exit_code={}", exit_code);
    // we do not have to save task context
    let mut _unused = KContext::blank();
    set_next_trigger();
    schedule(&mut _unused as *mut _);
}

lazy_static! {
    /// Global init process (PID 1).
    /// Uses the initproc embedded into the kernel so the official build does
    /// not depend on patching a pre-existing sdcard image.
    pub static ref INITPROC: Arc<ProcessControlBlock> = {
        ProcessControlBlock::new(crate::embedded::initproc_image())
    };
}
#[allow(missing_docs)]
pub fn add_initproc() {
    let _initproc = INITPROC.clone();
}
#[allow(missing_docs)]
pub fn remove_inactive_task(task: Arc<TaskControlBlock>) {
    remove_task(Arc::clone(&task));
}

fn wake_blocked_waiter(process: &Arc<ProcessControlBlock>) -> bool {
    let tasks = {
        let inner = process.inner_exclusive_access();
        inner
            .tasks
            .iter()
            .filter_map(|task| task.as_ref().map(Arc::clone))
            .collect::<Vec<_>>()
    };
    for task in tasks {
        let task_inner = task.inner_exclusive_access();
        if task_inner.task_status == TaskStatus::Blocked {
            drop(task_inner);
            wakeup_task(task);
            return true;
        }
    }
    false
}

// 在你的任务管理模块中
use crate::task::manager::add_task_front;
use core::task::{RawWaker, RawWakerVTable, Waker};

const VTABLE_FRONT: RawWakerVTable = RawWakerVTable::new(
    clone_waker,       // clone 使用相同的
    wake_front,        // wake 放到队首
    wake_by_ref_front, // wake_by_ref 放到队首
    drop_waker,        // drop 使用相同的
);
// 假设你有这个函数
fn wake_task_to_front(task: Arc<TaskControlBlock>) {
    let mut task_inner = task.inner_exclusive_access();
    if task_inner.task_status == TaskStatus::Zombie {
        return;
    }
    if task_inner.task_status == TaskStatus::Ready || task_inner.task_status == TaskStatus::Running
    {
        task_inner.pending_wakeup = true;
        return;
    }
    task_inner.task_status = TaskStatus::Ready;
    drop(task_inner);
    add_task_front(task);
}

pub fn task_waker_front(task: Arc<TaskControlBlock>) -> Waker {
    let raw_waker = RawWaker::new(
        Weak::into_raw(Arc::downgrade(&task)) as *const (),
        &VTABLE_FRONT,
    );
    unsafe { Waker::from_raw(raw_waker) }
}

unsafe fn wake_front(ptr: *const ()) {
    unsafe {
        let task = Weak::from_raw(ptr as *const TaskControlBlock);
        if let Some(task) = task.upgrade() {
            wake_task_to_front(task); // 放到队首
        }
    }
}

unsafe fn wake_by_ref_front(ptr: *const ()) {
    unsafe {
        let task = Weak::from_raw(ptr as *const TaskControlBlock);
        if let Some(task) = task.upgrade() {
            wake_task_to_front(task);
        }
        core::mem::forget(task);
    }
}

unsafe fn clone_waker(ptr: *const ()) -> RawWaker {
    unsafe {
        let task = Weak::from_raw(ptr as *const TaskControlBlock);
        let cloned = task.clone();
        core::mem::forget(task);
        RawWaker::new(Weak::into_raw(cloned) as *const (), &VTABLE_FRONT)
    }
}
unsafe fn drop_waker(ptr: *const ()) {
    unsafe {
        drop(Weak::from_raw(ptr as *const TaskControlBlock));
    }
}
