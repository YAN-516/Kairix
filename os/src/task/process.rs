use super::TaskControlBlock;
use super::add_task;
use super::id::{RecycleAllocator, kstack_alloc};
use super::manager::*;
use super::{PidHandle, pid_alloc};
use crate::config::PAGE_SIZE;
use crate::fs::vfs::Dentry;
use crate::fs::vfs::dcache::GLOBAL_DCACHE;
use crate::fs::{File, Stdin, Stdout};
use crate::mm::VMSpace;
use crate::mm::{UserVMSet, VMSet, translated_refmut};
use crate::sync::UPSafeCell;
use crate::timer::get_time;
use crate::trap::{TrapContext, trap_handler};
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec;
use alloc::vec::Vec;
use core::arch::asm;
use core::cell::RefMut;
use core::error;
use log::error;
use log::info;
use log::warn;
use spin::MutexGuard;
#[allow(unused)]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Tms {
    pub tms_utime: usize,
    pub tms_stime: usize,
    pub tms_cutime: usize,
    pub tms_cstime: usize,
}
#[allow(unused)]
impl Tms {
    pub fn new() -> Self {
        Self {
            tms_utime: 0,
            tms_stime: 0,
            tms_cutime: 0,
            tms_cstime: 0,
        }
    }
}
pub struct ProcessControlBlock {
    // immutable
    pub pid: PidHandle,
    // mutable
    inner: UPSafeCell<ProcessControlBlockInner>,
}

pub struct ProcessControlBlockInner {
    pub is_zombie: bool,
    pub vm_set: UserVMSet,
    pub parent: Option<Weak<ProcessControlBlock>>,
    pub children: Vec<Arc<ProcessControlBlock>>,
    pub exit_code: i32,
    pub fd_table: Vec<Option<Arc<dyn File + Send + Sync>>>,
    pub tasks: Vec<Option<Arc<TaskControlBlock>>>,
    pub task_res_allocator: RecycleAllocator,
    pub cwd: Arc<dyn Dentry>,
    pub time: Tms,
    pub ustart: usize,
    pub kstart: usize,
}

impl ProcessControlBlockInner {
    #[allow(unused)]
    pub fn get_user_token(&self) -> usize {
        self.vm_set.token()
    }
    pub fn is_zombie(&self) -> bool {
        self.is_zombie
    }
    pub fn alloc_fd(&mut self) -> usize {
        if let Some(fd) = (0..self.fd_table.len()).find(|fd| self.fd_table[*fd].is_none()) {
            fd
        } else {
            self.fd_table.push(None);
            self.fd_table.len() - 1
        }
    }

    pub fn alloc_tid(&mut self) -> usize {
        self.task_res_allocator.alloc()
    }

    pub fn dealloc_tid(&mut self, tid: usize) {
        self.task_res_allocator.dealloc(tid)
    }

    pub fn thread_count(&self) -> usize {
        self.tasks.len()
    }

    pub fn get_task(&self, tid: usize) -> Arc<TaskControlBlock> {
        self.tasks[tid].as_ref().unwrap().clone()
    }
}

impl ProcessControlBlock {
    pub fn inner_exclusive_access(&self) -> MutexGuard<'_, ProcessControlBlockInner> {
        self.inner.exclusive_access()
    }

    pub fn new(elf_data: &[u8]) -> Arc<Self> {
        // memory_set with elf program headers/trampoline/trap context/user stack
        // let (memory_set, ustack_base, entry_point) = UserVMSet::from_elf(elf_data);
        // allocate a pid

        // let memory_set = UserVMSet {
        //     inner: VMSet::new_bare(),
        // };
        let pid_handle = pid_alloc();
        let kstack = kstack_alloc();

        let (vm_set, ustack_top, entry_point) = UserVMSet::from_elf(elf_data);

        let process = Arc::new(Self {
            pid: pid_handle,
            inner: unsafe {
                UPSafeCell::new(ProcessControlBlockInner {
                    is_zombie: false,
                    vm_set: vm_set,
                    parent: None,
                    children: Vec::new(),
                    exit_code: 0,
                    fd_table: vec![
                        // 0 -> stdin
                        Some(Arc::new(Stdin)),
                        // 1 -> stdout
                        Some(Arc::new(Stdout)),
                        // 2 -> stderr
                        Some(Arc::new(Stdout)),
                    ],
                    tasks: Vec::new(),
                    task_res_allocator: RecycleAllocator::new(),
                    cwd: GLOBAL_DCACHE.get("/").unwrap().clone(),
                    time: Tms::new(),
                    ustart: 0,
                    kstart: get_time(),
                })
            },
        });

        // create a main thread, we should allocate ustack and trap_cx here
        let task = Arc::new(TaskControlBlock::new(
            Arc::clone(&process),
            ustack_top,
            true,
            kstack,
        ));

        // prepare trap_cx of main thread
        let task_inner = task.inner_exclusive_access();
        let trap_cx = task_inner.get_trap_cx();
        let ustack_top = task_inner.res.as_ref().unwrap().ustack_top();
        let kstack_top = task.kstack.get_top();
        drop(task_inner);
        *trap_cx = TrapContext::app_init_context(entry_point, ustack_top, kstack_top);
        // add main thread to the process
        let mut process_inner = process.inner_exclusive_access();
        process_inner.tasks.push(Some(Arc::clone(&task)));
        drop(process_inner);
        insert_into_pid2process(process.getpid(), Arc::clone(&process));
        // add main thread to scheduler
        add_task(task);
        process
    }

    /// Only support processes with a single thread.
    pub fn execve(self: &Arc<Self>, elf_data: &[u8]) {
        info!("execve");
        //println!("execve a new elf for process");
        assert_eq!(self.inner_exclusive_access().thread_count(), 1);
        // memory_set with elf program headers/trampoline/trap context/user stack
        let (memory_set, ustack_base, entry_point) = UserVMSet::from_elf(elf_data);
        let task_satp = memory_set.token();
        //println!("satp in trap_return:  {:#x}", task_satp);

        unsafe {
            riscv::register::satp::write(task_satp);
            asm!("sfence.vma");
        }

        // substitute memory_set
        self.inner_exclusive_access().vm_set = memory_set;
        // then we alloc user resource for main thread again
        // since memory_set has been changed
        let task = self.inner_exclusive_access().get_task(0);
        let mut task_inner = task.inner_exclusive_access();
        task_inner.res.as_mut().unwrap().ustack_base = ustack_base;
        task_inner.res.as_mut().unwrap().alloc_user_res();
        task_inner.trap_cx_ppn = task_inner.res.as_mut().unwrap().trap_cx_ppn();
        // push arguments on user stack
        let user_sp = task_inner.res.as_mut().unwrap().ustack_top() - 256;

        // initialize trap_cx
        let trap_cx = TrapContext::app_init_context(entry_point, user_sp, task.kstack.get_top());
        *task_inner.get_trap_cx() = trap_cx;
    }

    /// Only support processes with a single thread.
    pub fn fork(self: &Arc<Self>) -> Arc<Self> {
        info!("enter fork");
        let mut parent = self.inner_exclusive_access();
        assert_eq!(parent.thread_count(), 1);
        // clone parent's memory_set completely including trampoline/ustacks/trap_cxs
        //let memory_set = UserVMSet::from_existed_user(&parent.vm_set);
        // alloc a pid
        let memory_set = UserVMSet {
            inner: VMSet::new_bare(),
        };
        let pid = pid_alloc();
        // copy fd table
        let mut new_fd_table: Vec<Option<Arc<dyn File + Send + Sync>>> = Vec::new();
        for fd in parent.fd_table.iter() {
            if let Some(file) = fd {
                new_fd_table.push(Some(file.clone()));
            } else {
                new_fd_table.push(None);
            }
        }
        // create child process pcb
        let child = Arc::new(Self {
            pid,
            inner: unsafe {
                UPSafeCell::new(ProcessControlBlockInner {
                    is_zombie: false,
                    vm_set: memory_set,
                    parent: Some(Arc::downgrade(self)),
                    children: Vec::new(),
                    exit_code: 0,
                    fd_table: new_fd_table,
                    tasks: Vec::new(),
                    task_res_allocator: RecycleAllocator::new(),
                    cwd: parent.cwd.clone(),
                    time: Tms::new(),
                    ustart: 0,
                    kstart: get_time(),
                })
            },
        });
        // add child
        parent.children.push(Arc::clone(&child));
        let kstack = kstack_alloc();

        let vmset = UserVMSet::from_existed_user_cow(&mut parent.vm_set);

        child.inner_exclusive_access().vm_set = vmset;
        // create main thread of child process
        let task = Arc::new(TaskControlBlock::new(
            Arc::clone(&child),
            parent
                .get_task(0)
                .inner_exclusive_access()
                .res
                .as_ref()
                .unwrap()
                .ustack_base(),
            // here we do not allocate trap_cx or ustack again
            // but mention that we allocate a new kstack here
            false,
            kstack,
        ));
        // attach task to child process
        let mut child_inner = child.inner_exclusive_access();
        child_inner.tasks.push(Some(Arc::clone(&task)));
        drop(child_inner);
        // modify kstack_top in trap_cx of this thread
        let task_inner = task.inner_exclusive_access();
        let trap_cx = task_inner.get_trap_cx();
        trap_cx.kernel_sp = task.kstack.get_top();
        drop(task_inner);
        insert_into_pid2process(child.getpid(), Arc::clone(&child));
        // add this thread to scheduler
        // modify trap context of new_task, because it returns immediately after switching
        let new_process_inner = child.inner_exclusive_access();
        let tk = new_process_inner.tasks[0].as_ref().unwrap();
        let trap_cx = tk.inner_exclusive_access().get_trap_cx();
        // we do not have to move to next instruction since we have done it before
        // for child process, fork returns 0

        trap_cx.x[10] = 0;
        drop(new_process_inner);
        add_task(task);
        warn!(
            "fork a new process with pid {}, parent pid = {}",
            child.getpid(),
            self.getpid()
        );

        child
    }

    pub fn getpid(&self) -> usize {
        self.pid.0
    }

    pub fn _clone(self: &Arc<Self>, _flags: u32, stack: usize /* , arg: usize*/) -> isize {
        let stack_align = if stack % PAGE_SIZE != 0 {
            warn!("Stack address {:#x} not page-aligned, adjusting", stack);
            // 向下对齐到页边界
            stack & !(PAGE_SIZE - 1)
        } else {
            stack
        };
        let mut parent = self.inner_exclusive_access();

        let vm_set = UserVMSet::from_existed_user_cow(&mut parent.vm_set);
        let pid = pid_alloc();

        let mut table = Vec::new();
        for fd in parent.fd_table.iter() {
            if let Some(file) = fd {
                table.push(Some(file.clone()));
            } else {
                table.push(None);
            }
        }

        let child = Arc::new(Self {
            pid,
            inner: unsafe {
                UPSafeCell::new(ProcessControlBlockInner {
                    is_zombie: false,
                    vm_set: vm_set,
                    parent: Some(Arc::downgrade(self)),
                    children: Vec::new(),
                    exit_code: 0,
                    fd_table: table,
                    tasks: Vec::new(),
                    task_res_allocator: RecycleAllocator::new(),
                    cwd: parent.cwd.clone(),
                    time: Tms::new(),
                    ustart: 0,
                    kstart: get_time(),
                })
            },
        });

        parent.children.push(Arc::clone(&child));

        let kstack = kstack_alloc();

        let task = Arc::new(TaskControlBlock::new(
            Arc::clone(&child),
            stack_align,
            false,
            kstack,
        ));

        let mut child_inner = child.inner_exclusive_access();
        child_inner.tasks.push(Some(Arc::clone(&task)));
        drop(child_inner);

        let task_inner = task.inner_exclusive_access();
        let trap_cx = task_inner.get_trap_cx();

        trap_cx.set_sp(stack);

        // 子进程返回 0（fork 语义）
        // for (i, &arg) in args.iter().enumerate() {
        //     if i < 8 {
        //         // RISC-V 最多 8 个参数寄存器
        //         trap_cx.x[10 + i] = arg; // a0 = x10, a1 = x11, ...
        //     }
        // }
        trap_cx.x[10] = 0;

        // 设置内核栈
        trap_cx.kernel_sp = task.kstack.get_top();
        drop(task_inner);

        // 注册到全局进程表
        insert_into_pid2process(child.getpid(), Arc::clone(&child));

        // 添加到调度器
        add_task(task);
        // 父进程返回子进程 PID
        child.getpid() as isize
    }
}

pub const CLONE_VM: u32 = 0x00000100; // 共享内存描述符
pub const CLONE_FS: u32 = 0x00000200; // 共享文件系统信息
pub const CLONE_FILES: u32 = 0x00000400; // 共享文件描述符表
pub const CLONE_SIGHAND: u32 = 0x00000800; // 共享信号处理函数表
pub const CLONE_THREAD: u32 = 0x00010000; // 创建线程（同一线程组）
pub const CLONE_NEWNS: u32 = 0x00020000; // 新的挂载命名空间
pub const CLONE_NEWNET: u32 = 0x40000000; // 新的网络命名空间

pub const CLONE_THREAD_FLAGS: u32 =
    CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD;
