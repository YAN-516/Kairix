//! Task management implementation (stackless version)

mod context_async;
mod manager_async;
mod pid_async;
mod processor_async;
mod task_async;

use crate::fs::{OpenFlags, open_file};
use crate::sbi::shutdown;
use alloc::sync::Arc;
use lazy_static::*;

pub use context_async::{TaskContext, TaskStatus};
pub use manager_async::{TaskManager, add_task, fetch_task, task_num};
pub use pid_async::{PidAllocator, PidHandle, pid_alloc};
pub use processor_async::{
    Processor,
    current_task,
    current_trap_cx,
    current_user_token,
    exit_current_and_run_next, // 注意：添加了 exit_current_and_run_next
    init,
    run_tasks,
    suspend_current,
    switch_to_executor_and_run,
    take_current_task,
};
pub use task_async::{TaskControlBlock, create_user_task};

lazy_static! {
    #[allow(missing_docs)]
    pub static ref INIT_TASK: Arc<TaskControlBlock> = {
        let inode = open_file("initproc", OpenFlags::RDONLY).unwrap();
        let data = inode.read_all();
        create_user_task(data)
    };
}

/// 添加 init 任务到调度器
pub fn add_init_task() {
    println!("start add initproc···");
    add_task(INIT_TASK.clone());
}

#[allow(missing_docs)]
pub const IDLE_PID: usize = 0;

// 删除原有的 exit_current_and_run_next 实现，因为已由 processor_async 提供
