//! Implementation of Processor and executor for stackless coroutines

use super::{TaskControlBlock, TaskStatus, add_task, fetch_task};
use crate::config::{KERNEL_MEMORY_SPACE, PAGE_SIZE, PROCESSOR_STACK_SIZE};
use crate::mm::KERNEL_VMSET;
use crate::sync::UPSafeCell;
use crate::task_async::INIT_TASK;
use crate::task_async::{CpuContext, task_num};
use crate::trap::{TrapContext, trap_return};
use alloc::sync::Arc;
use core::arch::asm;
use core::task::{Poll, RawWaker, RawWakerVTable, Waker};
use lazy_static::*;
use log::info;
use xmas_elf::P32;
/// 处理器结构（每个 CPU 一个）
pub struct Processor {
    current: Option<Arc<TaskControlBlock>>,
    executor_top: usize,                   // 调度器专用栈顶
    executor_ctx: Option<*mut CpuContext>, // 调度器上下文指针
}

impl Processor {
    pub fn new() -> Self {
        let mut proc = Self {
            current: None,
            executor_top: 0,
            executor_ctx: None,
        };
        proc.init_executor_stack();
        proc
    }

    fn init_executor_stack(&mut self) {
        // 在内存空间顶部预留调度器栈
        self.executor_top = KERNEL_MEMORY_SPACE.1 - PAGE_SIZE; // 留一页 gap
        let executor_bottom = self.executor_top - PROCESSOR_STACK_SIZE;

        // 确保区域已映射（假设 KERNEL_VMSET 已包含该区域）
        // 实际使用时应调用映射函数，此处简化为直接信任
        info!(
            "Executor stack at {:#x} - {:#x}",
            executor_bottom, self.executor_top
        );
    }

    pub fn set_executor_ctx(&mut self, ctx: *mut CpuContext) {
        self.executor_ctx = Some(ctx);
    }

    pub fn executor_ctx(&self) -> Option<*mut CpuContext> {
        self.executor_ctx
    }

    pub fn current(&self) -> Option<Arc<TaskControlBlock>> {
        self.current.as_ref().map(Arc::clone)
    }

    pub fn take_current(&mut self) -> Option<Arc<TaskControlBlock>> {
        self.current.take()
    }

    /// 创建唤醒器
    fn create_waker(&self, task: Arc<TaskControlBlock>) -> Waker {
        unsafe fn wake_task(ptr: *const ()) {
            unsafe {
                let task = Arc::from_raw(ptr as *const TaskControlBlock);
                add_task(task);
            }
        }

        unsafe fn clone_task(ptr: *const ()) -> RawWaker {
            unsafe {
                let task = Arc::from_raw(ptr as *const TaskControlBlock);
                let cloned = task.clone();
                let _ = Arc::into_raw(cloned);
                RawWaker::new(ptr, &VTABLE)
            }
        }

        unsafe fn drop_task(ptr: *const ()) {
            unsafe {
                let _ = Arc::from_raw(ptr as *const TaskControlBlock);
            }
        }

        const VTABLE: RawWakerVTable =
            RawWakerVTable::new(clone_task, wake_task, wake_task, drop_task);

        let raw_waker = RawWaker::new(Arc::into_raw(task) as *const (), &VTABLE);
        unsafe { Waker::from_raw(raw_waker) }
    }

    /// 准备进入用户态（用于首次进入或从调度器返回）
    pub fn prepare_usermode(&self, task: &Arc<TaskControlBlock>) {
        // 切换页表到用户空间
        let token = task.inner_exclusive_access().get_user_token();
        unsafe {
            asm!("csrw satp, {}", in(reg) token);
            asm!("sfence.vma");
        }
        // 注意：此时内核栈已由 Trap 上下文中的 kernel_sp 指定
    }

    /// 空闲等待
    fn idle(&self) {
        unsafe {
            asm!("wfi");
        }
    }
}

lazy_static! {
    pub static ref PROCESSOR: UPSafeCell<Processor> = unsafe { UPSafeCell::new(Processor::new()) };
}

/// 初始化（可空）
pub fn init() {}

/// 获取当前任务
pub fn current_task() -> Option<Arc<TaskControlBlock>> {
    PROCESSOR.exclusive_access().current()
}

/// 取出当前任务
pub fn take_current_task() -> Option<Arc<TaskControlBlock>> {
    PROCESSOR.exclusive_access().take_current()
}

/// 获取当前用户 Token
pub fn current_user_token() -> usize {
    let task = current_task().unwrap();
    task.inner_exclusive_access().get_user_token()
}

/// 获取当前 Trap 上下文
pub fn current_trap_cx() -> &'static mut TrapContext {
    current_task()
        .unwrap()
        .inner_exclusive_access()
        .get_trap_cx()
}

// ================== 上下文切换汇编 ==================

/// 切换上下文：保存当前寄存器到 prev，加载 next 的寄存器并跳转
pub unsafe extern "C" fn switch_context(prev: *mut CpuContext, next: *const CpuContext) {
    unsafe {
        asm!(
            "sd ra,  0({0})",
            "sd sp,  8({0})",
            "sd s0, 16({0})",
            "sd s1, 24({0})",
            "sd s2, 32({0})",
            "sd s3, 40({0})",
            "sd s4, 48({0})",
            "sd s5, 56({0})",
            "sd s6, 64({0})",
            "sd s7, 72({0})",
            "sd s8, 80({0})",
            "sd s9, 88({0})",
            "sd s10, 96({0})",
            "sd s11, 104({0})",
            "ld ra,  0({1})",
            "ld sp,  8({1})",
            "ld s0, 16({1})",
            "ld s1, 24({1})",
            "ld s2, 32({1})",
            "ld s3, 40({1})",
            "ld s4, 48({1})",
            "ld s5, 56({1})",
            "ld s6, 64({1})",
            "ld s7, 72({1})",
            "ld s8, 80({1})",
            "ld s9, 88({1})",
            "ld s10, 96({1})",
            "ld s11, 104({1})",
            "ret",
            in(reg) prev,
            in(reg) next,
            options(noreturn)
        )
    }
}

/// 直接切换到执行器栈并跳转到 run_tasks（用于任务退出或初始调度）
pub unsafe extern "C" fn switch_to_executor() -> ! {
    // 注意：此函数必须在没有有效任务上下文时调用
    // 它直接设置 sp 为执行器栈顶并调用 run_tasks
    let executor_top: usize;
    unsafe {
        executor_top = PROCESSOR.exclusive_access().executor_top;

        asm!(
            "mv sp, {0}",
            "tail {1}",
            in(reg) executor_top,
            sym run_tasks,
            options(noreturn)
        )
    }
}

// ================== 执行器主循环 ==================

/// 任务入口函数（每个任务第一次被调度时从此处开始执行）
extern "C" fn task_entry() {
    loop {
        let task = current_task().expect("task_entry: no current task");
        let waker = task
            .inner_exclusive_access()
            .task_cx
            .waker()
            .expect("task_entry: waker not set");
        let poll_result = task.poll(&waker);
        match poll_result {
            Poll::Ready(exit_code) => {
                // 任务完成，退出
                exit_current_and_run_next(exit_code);
            }
            Poll::Pending => {
                // 任务主动让出 CPU
                suspend_current();
            }
        }
    }
}

/// 执行器主循环
pub fn run_tasks() {
    // 初始化执行器上下文（保存执行器自身的上下文位置）
    let mut executor_ctx = CpuContext::default();
    let executor_ctx_ptr = &mut executor_ctx as *mut CpuContext;
    PROCESSOR
        .exclusive_access()
        .set_executor_ctx(executor_ctx_ptr);

    loop {
        while let Some(task) = fetch_task() {
            // 设置为当前任务
            PROCESSOR.exclusive_access().current = Some(task.clone());

            // 为任务设置 waker（执行器创建，用于唤醒）
            let waker = PROCESSOR.exclusive_access().create_waker(task.clone());
            task.set_waker(waker);

            // 获取任务上下文指针，若第一次运行则初始化
            let task_ctx_ptr = {
                let mut inner = task.inner_exclusive_access();
                if inner.saved_context.is_none() {
                    // 首次运行：设置入口为 task_entry，栈为内核栈顶
                    let mut ctx = CpuContext::default();
                    ctx.ra = task_entry as usize;
                    ctx.sp = inner.kernel_stack.top();
                    inner.saved_context = Some(ctx);
                }
                inner.saved_context.as_mut().unwrap() as *mut CpuContext
            };

            unsafe {
                // 切换到任务
                switch_context(executor_ctx_ptr, task_ctx_ptr);
            }
            // 任务让出 CPU 后回到这里，继续取下一个任务
        }
        // 无任务可运行，进入低功耗等待
        PROCESSOR.exclusive_access().idle();
    }
}

/// 挂起当前任务（由系统调用调用）
pub fn suspend_current() {
    let task = take_current_task().expect("suspend_current: no current task");
    task.set_status(TaskStatus::Ready);
    add_task(task.clone());

    // 准备保存当前上下文到任务
    let task_ctx_ptr = {
        let mut inner = task.inner_exclusive_access();
        inner.saved_context = Some(CpuContext::default());
        inner.saved_context.as_mut().unwrap() as *mut CpuContext
    };

    // 获取执行器上下文指针
    let executor_ctx_ptr = PROCESSOR
        .exclusive_access()
        .executor_ctx()
        .expect("suspend_current: executor context not set");

    unsafe {
        // 切换到执行器
        switch_context(task_ctx_ptr, executor_ctx_ptr);
    }
    // 当任务再次被调度时，会从 task_entry 循环中恢复执行
    // 即返回到 task_entry 中调用 suspend_current 之后的指令
    // 但 task_entry 中调用 suspend_current 后不应有后续指令，所以这里不会执行到
}

/// 退出当前任务并切换到下一个（由 exit 系统调用调用）
pub fn exit_current_and_run_next(exit_code: i32) {
    let task = take_current_task().expect("exit: no current task");
    let pid = task.getpid();
    println!("[kernel] process {} exit with code {}", pid, exit_code);

    // 清理资源（原有逻辑）
    let mut inner = task.inner_exclusive_access();
    inner.task_status = TaskStatus::Zombie;
    inner.exit_code = exit_code;

    // 将子进程移交给 INIT_TASK（需要 INIT_TASK 全局变量）
    // 这里简化，假设存在 INIT_TASK
    {
        let init_task = INIT_TASK.clone();
        let mut init_inner = init_task.inner_exclusive_access();
        for child in inner.children.iter() {
            child.inner_exclusive_access().parent = Some(Arc::downgrade(&init_task));
            init_inner.children.push(child.clone());
        }
    }
    inner.children.clear();

    // 回收用户空间
    inner.vm_set.recycle_data_pages();

    // 释放 inner 锁和任务本身
    drop(inner);
    drop(task); // 释放 Arc

    // 切换到执行器（不会返回）
    unsafe {
        switch_to_executor();
    }
}
