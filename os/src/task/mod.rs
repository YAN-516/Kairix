// mod context;
mod id;
mod manager;
pub mod process;
mod processor;
use fatfs::info;
use log::log;
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
use crate::mm::vm_set::VMSpace;
use polyhal::VirtAddr;
// #[cfg(target_arch = "riscv64")]
// use crate::sbi::shutdown;
// #[cfg(target_arch = "loongarch64")]
// use crate::sbi_la::shutdown;
use crate::fs::vfs::OpenFlags;
use crate::syscall::shm::release_shm_attaches;
use crate::fs::vfs::dcache::GLOBAL_DCACHE;
use alloc::{sync::Arc, vec::Vec};
use polyhal::instruction::shutdown;
// pub use context::TaskContext;
pub use id::{IDLE_PID, KernelStack, PidHandle, kstack_alloc, pid_alloc};
use lazy_static::*;
use log::error;
use manager::fetch_task;
pub use manager::{
    add_task, num_processes, pid2process, remove_from_pid2process, remove_task, wakeup_task,
};
pub use process::{ProcessControlBlock, Rlimit64, RLIMIT_NOFILE, Tms};
pub use processor::{
    current_kstack_top, current_process, current_task, current_trap_cx, current_trap_cx_user_va,
    current_user_token, init_processors, run_tasks, schedule, take_current_task,
};
// use switch::__switch;
use polyhal::kcontext::*;
use polyhal_trap::trap::*;
use polyhal_trap::trapframe::*;
pub use task::{TaskControlBlock, TaskStatus};

fn task_entry() {
    // log::trace!("os::task::task_entry");
    error!("task_entry");
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
#[allow(missing_docs)]
pub fn block_current_and_run_next() {
    let task = take_current_task().unwrap();
    let mut task_inner = task.inner_exclusive_access();
    let task_cx_ptr = &mut task_inner.task_cx as *mut KContext;
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
                let kernel_va = phys_addr + VIRT_ADDR_START;
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
                shutdown();
            } else {
                //crate::sbi::shutdown(0); //0 for success hint
                shutdown();
            }
        }
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
        let old_areas = process_inner.vm_set.recycle_data_pages();
        // flush and drop file descriptors
        let files_to_flush: Vec<_> = process_inner
            .fd_table
            .iter_mut()
            .filter_map(|fd| fd.take())
            .collect();
        drop(process_inner);
        release_shm_attaches(&old_areas);
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
        drop(process_inner);

        // 向父进程发送 SIGCHLD 并尝试唤醒被阻塞的父任务
        let parent_weak = process.inner_exclusive_access().parent.clone();
        if let Some(parent) = parent_weak.and_then(|w| w.upgrade()) {
            crate::syscall::signal::deliver_signal(&parent, crate::task::signal::Signal::SigChld);
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
    // we do not have to save task context
    let mut _unused = KContext::blank();
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
