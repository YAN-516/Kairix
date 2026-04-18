use super::TaskControlBlock;
use super::add_task;
use super::id::{RecycleAllocator, kstack_alloc};
use super::manager::*;
use super::task_entry;
use super::{PidHandle, pid_alloc};
// use crate::config::PAGE_SIZE;
use crate::fs::vfs::Dentry;
use crate::fs::vfs::dcache::GLOBAL_DCACHE;
use crate::fs::{File, Stdin, Stdout};
use crate::mm::frame_alloc;
use crate::mm::frame_allocator;
use crate::mm::vm_set;
use crate::mm::PageTable;
use crate::mm::UserMapArea;
use crate::mm::VMSpace;
use crate::mm::{VirtAddr,MapType,MapPermission};
use crate::mm::{UserVMSet, translated_refmut};
use crate::socket::*;
use crate::sync::UPSafeCell;
// use crate::timer::get_time;
use crate::mm::UserMapAreaType;
use crate::trap::_set_sum_bit;
// use crate::trap::{TrapContext, trap_handler};
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec;
use alloc::vec::Vec;

use polyhal::consts::*;
use polyhal::pagetable;
use polyhal::pagetable::PTEFlags;
use polyhal::println;
use polyhal::timer::current_time;
use polyhal::utils::addr::VirtPageNum;
use polyhal::MappingFlags;
use polyhal::MappingSize;
#[cfg(target_arch = "riscv64")]
use riscv::register::mcause::Trap;

use core::arch::asm;
use core::cell::RefMut;
use core::error;
use core::mem;
use log::error;
use log::info;
use log::warn;
use polyhal::kcontext::*;
use polyhal_trap::trap::*;
use polyhal_trap::trapframe::*;
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
    // pub rawsocket: SocketManager,
    // pub udpsocket: SocketManager,
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

        let (vm_set, ustack_top, entry_point, _auxv) = UserVMSet::from_elf(elf_data).unwrap();

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
                    kstart: current_time().as_secs() as usize,
                    // rawsocket: SocketManager::new(),
                    // udpsocket: SocketManager::new(),
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
        let mut task_inner = task.inner_exclusive_access();
        let trap_cx = task_inner.get_trap_cx();
        let ustack_top = task_inner.res.as_ref().unwrap().ustack_top();
        let kstack_top = task.kstack.get_top();

        task_inner.task_cx[KContextArgs::KSP] = kstack_top;
        task_inner.task_cx[KContextArgs::KPC] = task_entry as usize;

        drop(task_inner);
        // *trap_cx = TrapContext::app_init_context(entry_point, ustack_top, kstack_top);
        trap_cx[TrapFrameArgs::SEPC] = entry_point;
        println!("set sp {:#x}", ustack_top);
        trap_cx[TrapFrameArgs::SP] = ustack_top;
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
    pub fn execve(
        self: &Arc<Self>,
        elf_data: &[u8],
        args: Vec<String>,
        envs: Vec<String>,
    ) -> isize {
        info!("execve");
        //println!("execve a new elf for process");
        assert_eq!(self.inner_exclusive_access().thread_count(), 1);
        // memory_set with elf program headers/trampoline/trap context/user stack
        let elf_result = UserVMSet::from_elf(elf_data);
        let (memory_set, ustack_base, entry_point, auxv) = match elf_result {
            Some(res) => res,
            None => {
                // BusyBox 收到 -8 后会自动把它当成 Shell 脚本去解释执行！
                return -8;
            }
        };
        let _task_satp = memory_set.token();
        memory_set.activate();
    
        // substitute memory_set
        self.inner_exclusive_access().vm_set = memory_set;
        // then we alloc user resource for main thread again
        // since memory_set has been changed
        let task = self.inner_exclusive_access().get_task(0);
        let mut task_inner = task.inner_exclusive_access();
        task_inner.res.as_mut().unwrap().ustack_base = ustack_base;

        println!("ustack base: {:#x}", ustack_base);
        task_inner.res.as_mut().unwrap().alloc_user_res();
        // task_inner.trap_cx_ppn = task_inner.res.as_mut().unwrap().trap_cx_ppn();
        task_inner.trap_cx = TrapFrame::new();
        // push arguments on user stack
        let mut user_sp = task_inner.res.as_mut().unwrap().ustack_top();

        // 闭包：安全地将内核数据跨页写入新进程的用户空间
        let write_to_user = |mut va: usize, data: &[u8]| {
            // let page_table = PageTable::from_token(task_satp);
            _set_sum_bit();
            let mut offset = 0;
            while offset < data.len() {
                let page_offset = va % PAGE_SIZE;
                let write_len = (PAGE_SIZE - page_offset).min(data.len() - offset);
                // println!("current page{:#x}", current_page.0);
                // if current_page != VirtAddr::from(va).floor(){
                //     vm_set.push(UserMapArea::new(VirtAddr(va),
                //     VirtAddr(va).ceil().into(), 
                //     MapType::Framed, 
                //     MapPermission::R|MapPermission::W, 
                //     UserMapAreaType::Elf, 
                //     false), None, va);
                //     current_page = VirtAddr::from(va).floor();
                // }
                // let page_table = vm_set.page_table_mut();

                // let pa = page_table
                //     .translate_va(VirtAddr::from(va))
                //     .expect("Failed to translate user stack va");
                // println!("pa: {:#x}", pa.0 + VIRT_ADDR_START);
                // let dst_ptr = (pa.0 + VIRT_ADDR_START) as *mut u8;
                println!("va {:#x} write to user",va);

                let dst_slice = unsafe { core::slice::from_raw_parts_mut(va as *mut u8, write_len) };
                dst_slice.copy_from_slice(&data[offset..offset + write_len]);

                va += write_len;
                offset += write_len;
            }
        };
        let mut arg_ptrs: Vec<usize> = Vec::new();
        let mut env_ptrs: Vec<usize> = Vec::new();

        //压入环境变量字符串 (Env)
        for env in envs.iter() {
            let bytes = env.as_bytes();
            user_sp -= bytes.len() + 1;
            write_to_user(user_sp, bytes);
            write_to_user(user_sp + bytes.len(), &[0]); // 写入字符串结尾的 null
            env_ptrs.push(user_sp);
        }

        // 压入参数字符串 (Args)
        for arg in args.iter() {
            let bytes = arg.as_bytes();
            user_sp -= bytes.len() + 1;
            write_to_user(user_sp, bytes);
            write_to_user(user_sp + bytes.len(), &[0]);
            arg_ptrs.push(user_sp);
        }
        user_sp &= !0xF;
        //压入auxv
        user_sp -= 16;
        let random_ptr = user_sp;
        let random_bytes: [u8; 16] = [
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33,
            0x22, 0x11,
        ];
        write_to_user(random_ptr, &random_bytes);

        user_sp &= !0xF;
        //指针数组
        // 布局：[argc, argv[0], ..., NULL, envp[0], ..., NULL]
        let mut ptrs: Vec<usize> = Vec::new();
        ptrs.push(args.len()); // argc
        for ptr in arg_ptrs.iter() {
            ptrs.push(*ptr);
        } // argv pointers
        ptrs.push(0);
        for ptr in env_ptrs.iter() {
            ptrs.push(*ptr);
        } // envp pointers
        ptrs.push(0);

        for (aux_type, aux_val) in auxv {
            ptrs.push(aux_type);
            ptrs.push(aux_val);
        }
        ptrs.push(0); // AT_NULL (结束标志)
        ptrs.push(0);

        // 将指针数组压入用户栈
        let ptrs_size = ptrs.len() * core::mem::size_of::<usize>();
        user_sp -= ptrs_size;
        user_sp &= !0xF; // 16字节对齐  
        let ptrs_bytes =
            unsafe { core::slice::from_raw_parts(ptrs.as_ptr() as *const u8, ptrs_size) };
        write_to_user(user_sp, ptrs_bytes);
        // unsafe {
        //     riscv::register::satp::write(task_satp);
        //     core::arch::asm!("sfence.vma");
        // }
        // initialize trap_cx
        // let trap_cx = TrapContext::app_init_context(entry_point, user_sp, task.kstack.get_top());
        let mut trap_cx = TrapFrame::new();

        trap_cx[TrapFrameArgs::SEPC] = entry_point;
        println!("user sp {:#x}", user_sp);
        trap_cx[TrapFrameArgs::SP] = user_sp;
        trap_cx[TrapFrameArgs::ARG0] = args.len();
        trap_cx[TrapFrameArgs::ARG1] = user_sp + core::mem::size_of::<usize>();

        *task_inner.get_trap_cx() = trap_cx;
        0
        // *task_inner.get_trap_cx() = trap_cx;
    }

    /// Only support processes with a single thread.
    pub fn fork(self: &Arc<Self>) -> Arc<Self> {

        info!("enter fork");
        let mut parent = self.inner_exclusive_access();
        assert_eq!(parent.thread_count(), 1);
        // clone parent's memory_set completely including trampoline/ustacks/trap_cxs
        //let memory_set = UserVMSet::from_existed_user(&parent.vm_set);
        // alloc a pid
        let memory_set = UserVMSet::new_bare();
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
                    kstart: current_time().as_secs() as usize,
                    // rawsocket: SocketManager::new(),
                    // udpsocket: SocketManager::new(),
                })
            },
        });
        // add child
        parent.children.push(Arc::clone(&child));
        let kstack = kstack_alloc();
        
        let vmset = UserVMSet::from_existed_user(&mut parent.vm_set);

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
        // trap_cx.kernel_sp = task.kstack.get_top();
        trap_cx.clone_from(&parent.get_task(0).inner_exclusive_access().trap_cx);

        drop(task_inner);
        insert_into_pid2process(child.getpid(), Arc::clone(&child));
        // add this thread to scheduler
        // modify trap context of new_task, because it returns immediately after switching
        // let new_process_inner = child.inner_exclusive_access();
        // let tk = new_process_inner.tasks[0].as_ref().unwrap();
        // let trap_cx = tk.inner_exclusive_access().get_trap_cx();
        // // we do not have to move to next instruction since we have done it before
        // // for child process, fork returns 0

        // trap_cx.x[10] = 0;
        // drop(new_process_inner);
        add_task(task);
        warn!(
            "fork a new process with pid {}, parent pid = {}",
            child.getpid(),
            self.getpid()
        );
        // loop{}

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

        let vm_set = UserVMSet::from_existed_user(&mut parent.vm_set);
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
                    kstart: current_time().as_secs() as usize,
                    // rawsocket: SocketManager::new(),
                    // udpsocket: SocketManager::new(),
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

        if stack != 0 {
            let stack_align = if stack % PAGE_SIZE != 0 {
                stack & !(PAGE_SIZE - 1)
            } else {
                stack
            };
            println!("set sp {:#x}", stack_align);
            trap_cx.set_sp(stack_align);
        }

        trap_cx[TrapFrameArgs::RET] = 0; // 子进程返回 0
        // trap_cx.kernel_sp = task.kstack.get_top();
        drop(task_inner);

        // 子进程返回 0（fork 语义）
        // for (i, &arg) in args.iter().enumerate() {
        //     if i < 8 {
        //         // RISC-V 最多 8 个参数寄存器
        //         trap_cx.x[10 + i] = arg; // a0 = x10, a1 = x11, ...
        //     }
        // }
        // trap_cx.x[10] = 0;

        // // 设置内核栈
        // trap_cx.kernel_sp = task.kstack.get_top();
        // drop(task_inner);

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
