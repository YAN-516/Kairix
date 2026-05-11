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
// use crate::fs::open_file;
use crate::fs::vfs::file::open_file;
use crate::fs::vfs::inode::InodeMode;
use crate::mm::vm_set::VMSpace;
use crate::timer::set_next_trigger;
use crate::trap::disable_timer_interrupt;
use polyhal::VirtAddr;
// #[cfg(target_arch = "riscv64")]
// use crate::sbi::shutdown;
// #[cfg(target_arch = "loongarch64")]
// use crate::sbi_la::shutdown;
use crate::fs::vfs::OpenFlags;
use crate::fs::vfs::dcache::GLOBAL_DCACHE;
use crate::socket::SOCKET_MANAGER;
use crate::syscall::shm::release_shm_attaches;
use alloc::{sync::Arc, vec::Vec};
use polyhal::instruction::shutdown;
// pub use context::TaskContext;
use crate::handle_signals;
pub use id::{IDLE_PID, KernelStack, PidHandle, kstack_alloc, pid_alloc};
use lazy_static::*;
use log::error;
use manager::fetch_task;
pub use manager::{
    add_task, num_processes, pid2process, remove_from_pid2process, remove_task, wakeup_task,
};
pub use process::{ProcessControlBlock, RLIMIT_NOFILE, Rlimit64, Tms};
pub use processor::{
    current_kstack_top, current_process, current_task, current_trap_cx, current_trap_cx_user_va,
    current_user_token, init_processors, run_tasks, schedule, take_current_task,
};
// use switch::__switch;
use polyhal::kcontext::*;
use polyhal_trap::trap::*;
use polyhal_trap::trapframe::*;
pub use task::{TaskControlBlock, TaskStatus};
use polyhal::timer::current_time;
use spin::Mutex;
use alloc::collections::BTreeMap;
static TIMER_QUEUE: Mutex<BTreeMap<u128, Vec<Arc<TaskControlBlock>>>> = Mutex::new(BTreeMap::new());


pub fn add_timer(task: Arc<TaskControlBlock>, wakeup_time: u128) {
    // info!("add_timer {}", wakeup_time);
    TIMER_QUEUE.lock().entry(wakeup_time).or_insert_with(Vec::new).push(task);
}

pub fn check_timers() {
    // info!("check_timers");
    
    let now = current_time().as_nanos();
    let mut queue = TIMER_QUEUE.lock();
    // log::info!("check_timers: now = {} ns", now);
    // log::info!("check_timers: queue has {} entries", queue.len());
        // 打印队列中的所有唤醒时间
        // for (&time, tasks) in queue.iter() {
        //     log::info!("check_timers: queue entry - time = {} ns, tasks = {}", time, tasks.len());
        //     log::info!("check_timers: time <= now? {} <= {} = {}", time, now, time <= now);
        // }
    
    // 找到所有需要唤醒的任务
    let expired: Vec<_> = queue.range(..=now).map(|(&time, _)| time).collect();
    
    for time in expired {
        // info!("time {}", time);
        if let Some(tasks) = queue.remove(&time) {
            for task in tasks {
                wakeup_task(task.clone());
                // let inner = task.inner_exclusive_access();
                // if inner.task_status == TaskStatus::Sleep {
                //     wakeup_task(task.clone());
                //     // inner.task_status = TaskStatus::Ready;
                //     // add_task(task.clone());
                // }
            }
        }
    }
}


fn handle_pending_signals(ctx: &mut TrapFrame) {
    handle_signals(ctx);
}

fn task_entry() {
    // log::trace!("os::task::task_entry");
    info!("task_entry");
    let current_task = current_task().unwrap();
    // current_task
    //     .process
    //     .upgrade()
    //     .unwrap()
    //     .inner_exclusive_access()
    //     .vm_set
    //     .activate();
    let task = current_task.inner_exclusive_access().get_trap_cx() as *mut TrapFrame;
    // run_user_task_forever(unsafe { task.as_mut().unwrap() })
    let ctx_mut = unsafe { task.as_mut().unwrap() };

    loop {
        run_user_task(ctx_mut);
        handle_pending_signals(ctx_mut);
    }
}

#[allow(missing_docs)]
pub fn suspend_current_and_run_next() {
    // error!("suspend");
    // There must be an application running.
    let task = take_current_task();
    if let Some(task) = task {
        // ---- access current TCB exclusively
        let mut task_inner = task.inner_exclusive_access();
        let task_cx_ptr = &mut task_inner.task_cx as *mut KContext;
        // Change status to Ready
        task_inner.task_status = TaskStatus::Ready;
        drop(task_inner);
        // ---- release current TCB

        // push back to ready queue.
        add_task(task);
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
        // ---- access current TCB exclusively
        let mut task_inner = task.inner_exclusive_access();
        let task_cx_ptr = &mut task_inner.task_cx as *mut KContext;
        // Change status to Ready
        task_inner.task_status = TaskStatus::Ready;
        drop(task_inner);
        // ---- release current TCB

        // push back to ready queue.
        add_task_front(task);
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
    task_inner.task_status = TaskStatus::Blocked;
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
    let tid = task_inner.res.as_ref().map(|r| r.tid).unwrap_or(0);
    // record exit code
    task_inner.exit_code = Some(exit_code);
    task_inner.res = None;
    info!(
        "exit_current_and_run_next: tid={} exit_code={}",
        tid, exit_code
    );
    // 先收集需要的信息，然后释放 task_inner，避免 task.inner -> process.inner 的锁顺序
    // 与 sys_exit_group / _clone 等 process.inner -> task.inner 的路径形成死锁。
    let (clear_child_tid, robust_list_head, robust_list_len) = {
        (
            task_inner.clear_child_tid,
            task_inner.robust_list_head,
            task_inner.robust_list_len,
        )
    };
    drop(task_inner);

    if let Some(process) = process_opt.as_ref() {
        let pid = process.getpid();
        if clear_child_tid != 0 {
            let process_inner = process.inner_exclusive_access();
            let page_table = &process_inner.vm_set.page_table;
            let vpn = VirtAddr::from(clear_child_tid).floor();
            if let Some(pte) = page_table.translate(vpn) {
                if pte.is_valid() {
                    let phys_addr = (pte.ppn().0 << 12) + (clear_child_tid % 4096);
                    let kernel_va = phys_addr + VIRT_ADDR_START;
                    unsafe {
                        *(kernel_va as *mut u32) = 0;
                    }
                }
            }
            drop(process_inner);

            // 唤醒可能正在等待 clear_child_tid 的线程
            crate::syscall::futex::futex_wake_one(clear_child_tid, pid);
        }

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

    // here we do not remove the thread since we are still using the kstack
    // it will be deallocated when sys_waittid is called
    drop(task);
    // however, if this is the main thread of current process
    // the process should terminate at once
    let mut should_wake_parent = false;
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
            info!(
                "[DEBUG] pid={} marked zombie=true exit_code={}",
                pid, exit_code
            );

            {
                let mut initproc_inner = INITPROC.inner_exclusive_access();
                for child in process_inner.children.iter() {
                    child.inner_exclusive_access().parent = Some(Arc::downgrade(&INITPROC));
                    initproc_inner.children.push(child.clone());
                }
            }

            let mut recycle_res = Vec::<TaskUserRes>::new();
            for task in process_inner.tasks.iter().filter(|t| t.is_some()) {
                let task = task.as_ref().unwrap();
                remove_inactive_task(Arc::clone(&task));
                let mut task_inner = task.inner_exclusive_access();
                if let Some(res) = task_inner.res.take() {
                    recycle_res.push(res);
                }
            }
            // 其他线程的资源已被回收，只剩当前线程（tid=0）待退出
            process_inner.alive_thread_count = 1;
            drop(process_inner);
            recycle_res.clear();

            let mut process_inner = process.inner_exclusive_access();
            process_inner.children.clear();
            let old_areas = process_inner.vm_set.recycle_data_pages();
            let files_to_flush: Vec<_> = process_inner
                .fd_table
                .iter_mut()
                .enumerate()
                .filter_map(|(fd, file)| file.take().map(|f| (fd, f)))
                .collect();
            drop(process_inner);
            {
                let mut manager = SOCKET_MANAGER.lock();
                for (fd, _) in &files_to_flush {
                    let _ = manager.close_socket_with_refcount(*fd, pid);
                }
            }
            release_shm_attaches(&old_areas);
            drop(old_areas); // 关键：释放 BTreeMap 节点和 FrameTracker，避免内核堆与物理页泄漏
            for (_, file) in &files_to_flush {
                file.flush();
            }
            let mut process_inner = process.inner_exclusive_access();
            process_inner.fd_table.clear();
            while process_inner.tasks.len() > 1 {
                process_inner.tasks.pop();
            }
            drop(process_inner);
        }

        // 减少 alive_thread_count，如果变为 0 则通知父进程
        let mut process_inner = process.inner_exclusive_access();
        process_inner.alive_thread_count -= 1;
        info!(
            "[DEBUG] pid={} tid={} exit, alive_thread_count={}",
            pid, tid, process_inner.alive_thread_count
        );
        if process_inner.is_zombie && process_inner.alive_thread_count == 0 {
            should_wake_parent = true;
        }
        drop(process_inner);

        if should_wake_parent {
            let parent_weak = process.inner_exclusive_access().parent.clone();
            if let Some(parent) = parent_weak.and_then(|w| w.upgrade()) {
                crate::syscall::signal::deliver_signal(
                    &parent,
                    crate::task::signal::Signal::SigChld,
                );
                let p_inner = parent.inner_exclusive_access();
                for task_opt in p_inner.tasks.iter() {
                    if let Some(task) = task_opt {
                        let t_inner = task.inner_exclusive_access();
                        if t_inner.task_status == crate::task::TaskStatus::Blocked {
                            drop(t_inner);
                            crate::task::wakeup_task(task.clone());
                            break;
                        }
                    }
                }
            }
        }
        drop(process);
    }
    info!("exit_current_and_run_next exit_code={}", exit_code);
    // we do not have to save task context
    let mut _unused = KContext::blank();
    set_next_trigger();
    schedule(&mut _unused as *mut _);
}

lazy_static! {
    /// Global init process (PID 1).
    /// Loads `initproc` from the root filesystem, which is responsible for
    /// setting up the userland environment and then exec-ing `user_shell`.
    pub static ref INITPROC: Arc<ProcessControlBlock> = {
        let cwd = GLOBAL_DCACHE.get("/").unwrap().clone();
        let file = open_file(cwd, "initproc", OpenFlags::RDONLY, InodeMode::FILE).unwrap();
        let v = file.read_all();
        ProcessControlBlock::new(v.as_slice())
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
    // 将任务放到就绪队列的队首
    // 方法1：如果有 add_task_front 函数
    add_task_front(task);
    // 方法2：先设置任务状态为 Ready，然后调度器会处理顺序
    // task.set_ready(true);
    // 但这样不保证队首，需要调度器支持
}

pub fn task_waker_front(task: Arc<TaskControlBlock>) -> Waker {
    let raw_waker = RawWaker::new(Arc::into_raw(task) as *const (), &VTABLE_FRONT);
    unsafe { Waker::from_raw(raw_waker) }
}

unsafe fn wake_front(ptr: *const ()) {
    unsafe {
        let task = Arc::from_raw(ptr as *const TaskControlBlock);
        println!("waking task to front: {:p}", Arc::as_ptr(&task));
        wake_task_to_front(task.clone()); // 放到队首

        core::mem::forget(task);
    }
}

unsafe fn wake_by_ref_front(ptr: *const ()) {
    unsafe {
        let task = Arc::from_raw(ptr as *const TaskControlBlock);
        wake_task_to_front(task.clone());
        core::mem::forget(task);
    }
}

unsafe fn clone_waker(ptr: *const ()) -> RawWaker {
    unsafe {
        let task = Arc::from_raw(ptr as *const TaskControlBlock);
        let cloned = task.clone();
        core::mem::forget(task);
        RawWaker::new(Arc::into_raw(cloned) as *const (), &VTABLE_FRONT)
    }
}
unsafe fn drop_waker(ptr: *const ()) {
    unsafe {
        drop(Arc::from_raw(ptr as *const TaskControlBlock));
    }
}
