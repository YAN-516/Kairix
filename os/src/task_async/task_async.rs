//! Implementation of TaskControlBlock for stackless coroutines

use super::{PidHandle, pid_alloc};
use super::{TaskContext, TaskStatus};
use crate::config::{KERNEL_STACK_PAGES, TRAP_CONTEXT};
use crate::fs::{File, Stdin, Stdout};
use crate::mm::{
    PAGE_SIZE, PhysPageNum, UserVMSet, VMSpace, VirtAddr, kernel_alloc, kernel_dealloc,
};
use crate::sync::UPSafeCell;
use crate::trap::TrapContext;
use alloc::sync::{Arc, Weak};
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefMut;
use core::future::Future;
use core::pin::Pin;
use core::task::Waker;
use core::task::{Context, Poll};
use log::warn;

/// 内核栈（每个任务独立）
pub struct KernelStack {
    bottom: usize, // 低地址
    top: usize,    // 高地址（栈顶）
}

impl KernelStack {
    pub fn new() -> Self {
        let pages = KERNEL_STACK_PAGES; // 通常为 8 页（32KB）
        let bottom = kernel_alloc(pages).expect("failed to allocate kernel stack");
        let top = bottom + pages * PAGE_SIZE;
        KernelStack { bottom, top }
    }

    pub fn top(&self) -> usize {
        self.top
    }
}

impl Drop for KernelStack {
    fn drop(&mut self) {
        let pages = (self.top - self.bottom) / PAGE_SIZE;
        kernel_dealloc(self.bottom, pages);
    }
}

/// 保存的 CPU 寄存器（被调用者保存）
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CpuContext {
    pub ra: usize,
    pub sp: usize,
    pub s0: usize,
    pub s1: usize,
    pub s2: usize,
    pub s3: usize,
    pub s4: usize,
    pub s5: usize,
    pub s6: usize,
    pub s7: usize,
    pub s8: usize,
    pub s9: usize,
    pub s10: usize,
    pub s11: usize,
}

#[allow(missing_docs)]
pub struct TaskControlBlock {
    pub pid: PidHandle,
    inner: UPSafeCell<TaskControlBlockInner>,
}

pub struct TaskControlBlockInner {
    pub trap_cx_ppn: PhysPageNum,

    /// 用户栈大小
    pub base_size: usize,

    /// 任务上下文（包含 Future）
    pub task_cx: TaskContext,

    pub task_status: TaskStatus,
    pub vm_set: UserVMSet,
    pub parent: Option<Weak<TaskControlBlock>>,
    pub children: Vec<Arc<TaskControlBlock>>,
    pub exit_code: i32,
    pub fd_table: Vec<Option<Arc<dyn File + Send + Sync>>>,

    // === 新增字段 ===
    /// 任务独立内核栈
    pub kernel_stack: KernelStack,
    /// 保存的 CPU 上下文（用于切换）
    pub saved_context: Option<CpuContext>,
}

impl TaskControlBlockInner {
    pub fn get_trap_cx(&self) -> &'static mut TrapContext {
        self.trap_cx_ppn.get_mut()
    }

    pub fn get_user_token(&self) -> usize {
        self.vm_set.token()
    }

    pub fn is_zombie(&self) -> bool {
        self.task_status == TaskStatus::Zombie
    }

    pub fn alloc_fd(&mut self) -> usize {
        if let Some(fd) = (0..self.fd_table.len()).find(|fd| self.fd_table[*fd].is_none()) {
            fd
        } else {
            self.fd_table.push(None);
            self.fd_table.len() - 1
        }
    }
}

#[allow(missing_docs)]
impl TaskControlBlock {
    pub fn inner_exclusive_access(&self) -> RefMut<'_, TaskControlBlockInner> {
        self.inner.exclusive_access()
    }

    /// 创建新的任务（从用户程序）
    pub fn new_user<F>(elf_data: &[u8], user_future: F, pid: PidHandle) -> Self
    where
        F: Future<Output = i32> + Send + 'static,
    {
        let pid_handle = pid;

        let (vm_set, user_sp, entry_point) = UserVMSet::from_elf(elf_data);

        let trap_cx_ppn = vm_set
            .translate(VirtAddr::from(TRAP_CONTEXT).into())
            .unwrap()
            .ppn();

        // 创建任务上下文（包含 Future）
        let task_cx = TaskContext::new(user_future);

        // === 新增：分配内核栈 ===
        let kernel_stack = KernelStack::new();

        let task = Self {
            pid: pid_handle,
            inner: unsafe {
                UPSafeCell::new(TaskControlBlockInner {
                    trap_cx_ppn,
                    base_size: user_sp,
                    task_cx,
                    task_status: TaskStatus::Ready,
                    vm_set,
                    parent: None,
                    children: Vec::new(),
                    exit_code: 0,
                    fd_table: vec![
                        Some(Arc::new(Stdin)),
                        Some(Arc::new(Stdout)),
                        Some(Arc::new(Stdout)),
                    ],
                    kernel_stack,
                    saved_context: None,
                })
            },
        };

        let trap_cx = task.inner_exclusive_access().get_trap_cx();
        *trap_cx = TrapContext::app_init_context(
            entry_point,
            user_sp,
            task.inner_exclusive_access().kernel_stack.top(), // 内核栈顶
        );

        task
    }

    pub fn getpid(&self) -> usize {
        self.pid.0
    }

    pub fn poll(&self, waker: &Waker) -> core::task::Poll<i32> {
        let mut inner = self.inner_exclusive_access();
        let mut cx = core::task::Context::from_waker(waker);
        inner.task_cx.poll(&mut cx)
    }

    pub fn set_waker(&self, waker: Waker) {
        let mut inner = self.inner_exclusive_access();
        inner.task_cx.set_waker(waker);
    }

    pub fn status(&self) -> TaskStatus {
        self.inner_exclusive_access().task_status
    }

    pub fn set_status(&self, status: TaskStatus) {
        let mut inner = self.inner_exclusive_access();
        inner.task_status = status;
    }

    pub fn get_trap_cx(&self) -> &'static mut TrapContext {
        self.inner_exclusive_access().get_trap_cx()
    }
}

impl TaskControlBlock {
    /// 执行新程序（exec 系统调用）
    pub fn exec(&self, elf_data: &[u8]) {
        // 创建新的用户内存空间
        let (vm_set, user_sp, entry_point) = UserVMSet::from_elf(elf_data);
        let trap_cx_ppn = vm_set
            .translate(VirtAddr::from(TRAP_CONTEXT).into())
            .unwrap()
            .ppn();

        let mut inner = self.inner_exclusive_access();

        // 替换内存集和 Trap 上下文物理页号
        inner.vm_set = vm_set;
        inner.trap_cx_ppn = trap_cx_ppn;

        // 重新初始化 Trap 上下文（用户态入口、栈指针等）
        let trap_cx = inner.get_trap_cx();
        *trap_cx = TrapContext::app_init_context(
            entry_point,
            user_sp,
            inner.kernel_stack.top(), // 使用任务自己的内核栈
        );

        // 替换任务的 Future 为新程序的 Future
        let new_future = UserProgram {
            pid: self.getpid(),
            first_run: true,
        };
        inner.task_cx = TaskContext::new(new_future);

        // 状态设为 Ready（可被调度）
        inner.task_status = TaskStatus::Ready;
    }

    /// 复制当前任务（fork 系统调用）
    pub fn fork(self: &Arc<TaskControlBlock>) -> Arc<TaskControlBlock> {
        let mut parent_inner = self.inner_exclusive_access();

        // 复制用户地址空间
        let vm_set = UserVMSet::from_existed_user(&parent_inner.vm_set);
        let trap_cx_ppn = vm_set
            .translate(VirtAddr::from(TRAP_CONTEXT).into())
            .unwrap()
            .ppn();

        // 分配新 PID
        let pid_handle = pid_alloc();

        // 复制文件描述符表
        let mut new_fd_table = Vec::new();
        for fd in parent_inner.fd_table.iter() {
            if let Some(file) = fd {
                new_fd_table.push(Some(file.clone()));
            } else {
                new_fd_table.push(None);
            }
        }

        // 创建子进程的 Future
        let child_future = UserProgram {
            pid: pid_handle.0,
            first_run: true,
        };
        let task_cx = TaskContext::new(child_future);

        // === 新增：为子进程分配独立内核栈 ===
        let kernel_stack = KernelStack::new();

        // 构建子进程 TCB
        let child = Arc::new(TaskControlBlock {
            pid: pid_handle,
            inner: unsafe {
                UPSafeCell::new(TaskControlBlockInner {
                    trap_cx_ppn,
                    base_size: parent_inner.base_size,
                    task_cx,
                    task_status: TaskStatus::Ready,
                    vm_set,
                    parent: Some(Arc::downgrade(self)),
                    children: Vec::new(),
                    exit_code: 0,
                    fd_table: new_fd_table,
                    kernel_stack,
                    saved_context: None,
                })
            },
        });

        // 复制父进程 Trap 上下文，并将返回值修改为 0（子进程）
        {
            let mut child_inner = child.inner_exclusive_access();
            let parent_trap_cx = parent_inner.get_trap_cx();
            let child_trap_cx = child_inner.get_trap_cx();
            *child_trap_cx = parent_trap_cx.clone();
            child_trap_cx.x[10] = 0; // a0 = 0 表示子进程
            child_trap_cx.kernel_sp = child_inner.kernel_stack.top(); // 设置子进程内核栈
        }

        // 将子进程加入父进程的 children 列表
        parent_inner.children.push(child.clone());

        child
    }
}

// 用户任务的 Future 实现（保持不变）
struct UserProgram {
    pid: usize,
    first_run: bool,
}

impl Future for UserProgram {
    type Output = i32;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };
        if this.first_run {
            warn!("[UserTask {}] First time entering user mode", this.pid);
            Poll::Ready(666) // 触发进入用户态
        } else {
            Poll::Pending
        }
    }
}

/// 创建一个用户态任务
pub fn create_user_task(elf_data: Vec<u8>) -> Arc<TaskControlBlock> {
    let new_pid = pid_alloc();

    let task = TaskControlBlock::new_user(
        &elf_data,
        UserProgram {
            pid: new_pid.0,
            first_run: true,
        },
        new_pid,
    );

    Arc::new(task)
}
