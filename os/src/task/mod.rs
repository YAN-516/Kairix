mod context;
mod id;
mod manager;
pub mod process;
mod processor;
pub mod signal;
mod switch;
#[allow(clippy::module_inception)]
#[allow(rustdoc::private_intra_doc_links)]
pub mod task;
use self::id::TaskUserRes;
use crate::fs::vfs::file::open_file;
use crate::KERNEL_SPACE_OFFSET;
use crate::fs::vfs::OpenFlags;
use crate::fs::vfs::dcache::GLOBAL_DCACHE;
use crate::mm::VirtAddr;
use crate::sbi::shutdown;
use crate::timer::get_time;
use alloc::{sync::Arc, vec::Vec};
pub use context::TaskContext;
pub use id::{IDLE_PID, KernelStack, PidHandle, kstack_alloc, pid_alloc};
use lazy_static::*;
use manager::fetch_task;
pub use manager::{add_task, pid2process, remove_from_pid2process, remove_task, wakeup_task, num_processes};
pub use process::{ProcessControlBlock, Tms};
pub use processor::{
    current_kstack_top, current_process, current_task, current_trap_cx, current_trap_cx_user_va,
    current_user_token, init_processors, run_tasks, schedule, take_current_task,
};
use switch::__switch;
pub use task::{TaskControlBlock, TaskStatus};
#[allow(missing_docs)]
pub fn suspend_current_and_run_next() {
    // There must be an application running.
    let task = take_current_task();
    if let Some(task) = task {
        // ---- access current TCB exclusively
        let mut task_inner = task.inner_exclusive_access();
        let task_cx_ptr = &mut task_inner.task_cx as *mut TaskContext;
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
#[allow(missing_docs)]
pub fn block_current_and_run_next() {
    let task = take_current_task().unwrap();
    let mut task_inner = task.inner_exclusive_access();
    let task_cx_ptr = &mut task_inner.task_cx as *mut TaskContext;
    task_inner.task_status = TaskStatus::Blocked;
    drop(task_inner);
    schedule(task_cx_ptr);
}

/// Exit the current 'Running' task and run the next task in task list.
pub fn exit_current_and_run_next(exit_code: i32) {
    let task = take_current_task().unwrap();
    let mut task_inner = task.inner_exclusive_access();
    let process = task.process.upgrade().unwrap();
    let tid = task_inner.res.as_ref().unwrap().tid;
    // record exit code
    task_inner.exit_code = Some(exit_code);
    task_inner.res = None;

    let clear_child_tid = task_inner.clear_child_tid;
    if clear_child_tid != 0 {
        let process_inner = process.inner_exclusive_access();
        let page_table = &process_inner.vm_set.page_table;
        let vpn = VirtAddr::from(clear_child_tid).floor();
        if let Some(pte) = page_table.translate(vpn) {
            if pte.is_valid() {
                let phys_addr = (pte.ppn().0 << 12) + (clear_child_tid % 4096);
                let kernel_va = phys_addr + crate::config::KERNEL_SPACE_OFFSET;
                unsafe {
                    *(kernel_va as *mut u32) = 0;
                }
            }
        }
        drop(process_inner);

        // TODO: 如果实现了 futex，需要在这里唤醒等待的线程：
        // crate::syscall::futex_wake(clear_child_tid, 1);
    }

    // here we do not remove the thread since we are still using the kstack
    // it will be deallocated when sys_waittid is called
    drop(task_inner);
    drop(task);
    // however, if this is the main thread of current process
    // the process should terminate at once
    if tid == 0 {
        let pid = process.getpid();

        // let mut inner = process.inner_exclusive_access();
        // let parent = inner.parent.as_mut().unwrap().upgrade().unwrap();

        // parent.inner_exclusive_access().time.tms_cstime +=
        //     inner.time.tms_stime + get_time() - inner.kstart;

        // parent.inner_exclusive_access().time.tms_cutime += inner.time.tms_utime;
        if pid == IDLE_PID {
            println!(
                "[kernel] Idle process exit with exit_code {} ...",
                exit_code
            );
            if exit_code != 0 {
                //crate::sbi::shutdown(255); //255 == -1 for err hint
                shutdown(true);
            } else {
                //crate::sbi::shutdown(0); //0 for success hint
                shutdown(false);
            }
        }
        remove_from_pid2process(pid);
        let mut process_inner = process.inner_exclusive_access();
        // mark this process as a zombie process
        process_inner.is_zombie = true;
        // record exit code of main process
        process_inner.exit_code = exit_code;

        {
            // move all child processes under init process
            let mut initproc_inner = INITPROC.inner_exclusive_access();
            for child in process_inner.children.iter() {
                child.inner_exclusive_access().parent = Some(Arc::downgrade(&INITPROC));
                initproc_inner.children.push(child.clone());
            }
        }

        // deallocate user res (including tid/trap_cx/ustack) of all threads
        // it has to be done before we dealloc the whole memory_set
        // otherwise they will be deallocated twice
        let mut recycle_res = Vec::<TaskUserRes>::new();
        for task in process_inner.tasks.iter().filter(|t| t.is_some()) {
            let task = task.as_ref().unwrap();
            // if other tasks are Ready in TaskManager or waiting for a timer to be
            // expired, we should remove them.
            //
            // Mention that we do not need to consider Mutex/Semaphore since they
            // are limited in a single process. Therefore, the blocked tasks are
            // removed when the PCB is deallocated.
            remove_inactive_task(Arc::clone(&task));
            let mut task_inner = task.inner_exclusive_access();
            if let Some(res) = task_inner.res.take() {
                recycle_res.push(res);
            }
        }
        // dealloc_tid and dealloc_user_res require access to PCB inner, so we
        // need to collect those user res first, then release process_inner
        // for now to avoid deadlock/double borrow problem.
        drop(process_inner);
        recycle_res.clear();

        let mut process_inner = process.inner_exclusive_access();
        process_inner.children.clear();
        // deallocate other data in user space i.e. program code/data section
        process_inner.vm_set.recycle_data_pages();
        // flush and drop file descriptors
        let files_to_flush: Vec<_> = process_inner.fd_table.iter_mut().filter_map(|fd| fd.take()).collect();
        drop(process_inner);
        for file in files_to_flush {
            file.flush();
        }
        let mut process_inner = process.inner_exclusive_access();
        process_inner.fd_table.clear();
        // Remove all tasks except for the main thread itself.
        // This is because we are still using the kstack under the TCB
        // of the main thread. This TCB, including its kstack, will be
        // deallocated when the process is reaped via waitpid.
        while process_inner.tasks.len() > 1 {
            process_inner.tasks.pop();
        }
    }
    drop(process);
    // we do not have to save task context
    let mut _unused = TaskContext::zero_init();
    schedule(&mut _unused as *mut _);
}

lazy_static! {
    ///Globle process that init user shell
    pub static ref INITPROC: Arc<ProcessControlBlock> = {
        let cwd = GLOBAL_DCACHE.get("/").unwrap().clone();
        let file = open_file(cwd,"initproc", OpenFlags::RDONLY).unwrap();
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
