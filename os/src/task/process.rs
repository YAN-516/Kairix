use super::TaskControlBlock;
use super::add_task;
use super::id::{RecycleAllocator, kstack_alloc};
use super::manager::*;
use super::task_entry;
use super::{PidHandle, pid_alloc};
// use crate::config::PAGE_SIZE;
use crate::error::SysError;
use crate::fs::File;
use crate::sync::SpinNoIrqLock;
use crate::timer::set_next_trigger;
use crate::trap::disable_timer_interrupt;
use core::sync::atomic::{AtomicBool, Ordering};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Rlimit64 {
    pub rlim_cur: u64,
    pub rlim_max: u64,
}

pub const RLIMIT_NOFILE: i32 = 7;
use crate::fs::devfs::tty::TtyFile;
use crate::fs::vfs::Dentry;
use crate::fs::vfs::dcache::GLOBAL_DCACHE;
use crate::fs::vfs::file::find_dentry;
use crate::mm::PageTable;
use crate::mm::UserMapArea;
use crate::mm::VMSpace;
use crate::mm::frame_alloc;
use crate::mm::frame_allocator;
use crate::mm::vm_set;
use crate::mm::{MapPermission, MapType, VirtAddr};
use crate::mm::{UserVMSet, translated_refmut};
use crate::signal::*;
use crate::socket::*;
use crate::syscall::shm::{fork_inherit_shm_attach, release_shm_attaches};
use crate::task::id::PgidHandle;
// use crate::timer::get_time;
use crate::mm::UserMapAreaType;
use crate::trap::_set_sum_bit;
// use crate::trap::{TrapContext, trap_handler};
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec;
use alloc::vec::Vec;

use polyhal::MappingFlags;
use polyhal::MappingSize;
use polyhal::consts::*;
use polyhal::pagetable;
use polyhal::pagetable::PTEFlags;
use polyhal::println;
use polyhal::timer::current_time;
use polyhal::utils::addr::VirtPageNum;
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

pub enum ProcessStatus {
    Ready,
    Running,
    Blocked,
    Terminal,
}
pub struct ProcessControlBlock {
    // immutable
    pub pid: PidHandle,
    // mutable
    inner: SpinNoIrqLock<ProcessControlBlockInner>,
}

pub struct ProcessControlBlockInner {
    pub is_zombie: bool,
    pub zombie_flag: AtomicBool,
    pub pgid: PgidHandle,
    /// 真实 UID
    pub uid: u32,
    /// 有效 UID
    pub euid: u32,
    /// 保存的 set-user-ID
    pub suid: u32,
    /// 真实 GID
    pub gid: u32,
    /// 有效 GID
    pub egid: u32,
    /// 保存的 set-group-ID
    pub sgid: u32,
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
    pub state: ProcessStatus,

    pub pending_signals: SignalSet,
    pub blocked_signals: SignalSet,
    pub signals_handler: SignalHandlers,
    pub need_signal_handle: bool,
    pub itimer_real_deadline: Option<usize>,
    pub itimer_real_interval: Option<usize>,
    pub wait_waker: Option<core::task::Waker>,
    /// 信号处理上下文栈（保存在 PCB 中，单线程场景下安全）
    pub sig_context_stack: Vec<(TrapFrame, SignalSet)>,
    /// ITIMER_REAL 的到期时间（微秒），None 表示未设置
    pub alarm_deadline_us: Option<u128>,
    /// ITIMER_REAL 的间隔时间（微秒），None 表示单次定时器
    pub alarm_interval_us: Option<u128>,
    /// 资源限制：单文件描述符最大数量
    pub rlimit_nofile: Rlimit64,
    /// 文件创建权限掩码
    pub umask: u32,
    /// 还活着的线程数量（用于 waitpid 判断是否可以回收进程）
    pub alive_thread_count: usize,
}

impl ProcessControlBlockInner {
    #[allow(unused)]
    pub fn get_user_token(&self) -> usize {
        self.vm_set.token()
    }
    pub fn is_zombie(&self) -> bool {
        self.is_zombie
    }

    pub fn handle_default_action(&mut self, signal: Signal) {
        match signal.default_action() {
            SignalAction::Ignore => {}
            SignalAction::Stop => {
                self.state = ProcessStatus::Terminal;
            }
            SignalAction::Continue => {
                self.state = ProcessStatus::Ready;
            }
            SignalAction::Terminate | SignalAction::Core => {
                self.is_zombie = true;
                self.zombie_flag.store(true, Ordering::SeqCst);
            }
        }
    }

    pub fn alloc_fd(&mut self) -> Result<usize, SysError> {
        let max_fd = self.rlimit_nofile.rlim_cur as usize;
        // 在允许范围内寻找最小的空闲 fd（0..max_fd）
        if let Some(fd) = (0..max_fd.min(self.fd_table.len()))
            .find(|fd| self.fd_table[*fd].is_none())
        {
            return Ok(fd);
        }
        // 允许范围内没有空闲 slot，尝试扩展（前提是当前长度 < max_fd）
        if self.fd_table.len() < max_fd {
            self.fd_table.push(None);
            Ok(self.fd_table.len() - 1)
        } else {
            Err(SysError::EMFILE)
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
    pub fn inner_exclusive_access(
        &self,
    ) -> crate::sync::SpinMutexGuard<'_, ProcessControlBlockInner, crate::sync::SpinNoIrq> {
        self.inner.lock()
    }

    pub fn new(elf_data: &[u8]) -> Arc<Self> {
        // memory_set with elf program headers/trampoline/trap context/user stack
        // let (memory_set, ustack_base, entry_point) = UserVMSet::from_elf(elf_data);
        // allocate a pid

        // let memory_set = UserVMSet {
        //     inner: VMSet::new_bare(),
        // };
        let pid_handle = pid_alloc();
        let pid = pid_handle.0;
        let kstack = kstack_alloc();

        let (vm_set, ustack_top, entry_point, _auxv) = UserVMSet::from_elf(elf_data).unwrap();
        let tty_dentry =
            find_dentry("/dev/tty").expect("Failed to find /dev/tty! Make sure devfs is mounted.");

        let tty_file: Arc<dyn File> = Arc::new(TtyFile::new(tty_dentry));
        let process = Arc::new(Self {
            pid: pid_handle,
            inner: SpinNoIrqLock::new(ProcessControlBlockInner {
                uid: 0,
                euid: 0,
                suid: 0,
                gid: 0,
                egid: 0,
                sgid: 0,
                is_zombie: false,
                zombie_flag: AtomicBool::new(false),
                pgid: PgidHandle(pid),
                vm_set: vm_set,
                parent: None,
                children: Vec::new(),
                exit_code: 0,
                fd_table: vec![
                    Some(tty_file.clone()), // fd 0: 准标准输入
                    Some(tty_file.clone()), // fd 1: 标准输出
                    Some(tty_file.clone()), // fd 2: 标准错误输出
                ],
                // fd_table: vec![
                //     // 0 -> stdin
                //     Some(Arc::new(Stdin)),
                //     // 1 -> stdout
                //     Some(Arc::new(Stdout)),
                //     // 2 -> stderr
                //     Some(Arc::new(Stdout)),
                // ],
                tasks: Vec::new(),
                task_res_allocator: RecycleAllocator::new(),
                cwd: GLOBAL_DCACHE.get("/").unwrap().clone(),
                time: Tms::new(),
                ustart: 0,
                kstart: current_time().as_micros() as usize,
                state: ProcessStatus::Ready,

                pending_signals: SignalSet::empty(),
                blocked_signals: SignalSet::empty(),
                signals_handler: SignalHandlers::new(),
                wait_waker: None,
                need_signal_handle: false,
                itimer_real_deadline: None,
                itimer_real_interval: None,
                sig_context_stack: Vec::new(),
                alarm_deadline_us: None,
                alarm_interval_us: None,
                rlimit_nofile: Rlimit64 {
                    rlim_cur: 1024,
                    rlim_max: 1024,
                },
                umask: 0o022,
                alive_thread_count: 1,
            }),
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
        {
            let mut inner = self.inner_exclusive_access();
            let old_vm_set = core::mem::replace(&mut inner.vm_set, memory_set);
            release_shm_attaches(&old_vm_set.areas);
            drop(old_vm_set);
            // POSIX: execve 必须重置所有信号处理器为 SIG_DFL（SIG_IGN 保持不变）
            inner.signals_handler.reset_all();
            inner.pending_signals = SignalSet::empty();
            inner.need_signal_handle = false;
        }
        // then we alloc user resource for main thread again
        // since memory_set has been changed
        let task = self.inner_exclusive_access().get_task(0);
        let mut task_inner = task.inner_exclusive_access();
        task_inner.res.as_mut().unwrap().ustack_base = ustack_base;

        info!("ustack base: {:#x}", ustack_base);
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
                info!("va {:#x} write to user", va);

                let dst_slice =
                    unsafe { core::slice::from_raw_parts_mut(va as *mut u8, write_len) };
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
        // glibc 启动期会使用这两个辅助向量项。
        const AT_RANDOM: usize = 25;
        const AT_EXECFN: usize = 31;
        ptrs.push(AT_RANDOM);
        ptrs.push(random_ptr);
        ptrs.push(AT_EXECFN);
        ptrs.push(arg_ptrs.first().copied().unwrap_or(0));
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
        info!("user sp {:#x}", user_sp);
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
        // clone sockets in SOCKET_MANAGER for child process
        let parent_pid = self.getpid();
        let sockets_to_clone: Vec<(usize, SocketInner)> = {
            let manager = SOCKET_MANAGER.lock();
            new_fd_table
                .iter()
                .enumerate()
                .filter_map(|(fd, _)| {
                    manager
                        .get_socket(fd, parent_pid)
                        .map(|sock| (fd, sock.inner.clone()))
                })
                .collect()
        };
        {
            let mut manager = SOCKET_MANAGER.lock();
            for (fd, inner) in sockets_to_clone {
                let new_socket = Socket::new(inner, fd, pid.0);
                let _ = manager.add_socket(fd, new_socket, pid.0);
            }
        }
        // create child process pcb
        let child = Arc::new(Self {
            pid,
            inner: SpinNoIrqLock::new(ProcessControlBlockInner {
                uid: parent.uid,
                euid: parent.euid,
                suid: parent.suid,
                gid: parent.gid,
                egid: parent.egid,
                sgid: parent.sgid,
                is_zombie: false,
                zombie_flag: AtomicBool::new(false),
                pgid: parent.pgid,
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
                kstart: current_time().as_micros() as usize,
                state: ProcessStatus::Ready,
                pending_signals: SignalSet::empty(),
                blocked_signals: parent.blocked_signals.clone(),
                signals_handler: parent.signals_handler.clone(),
                need_signal_handle: false,
                itimer_real_deadline: None,
                itimer_real_interval: None,
                wait_waker: None,
                sig_context_stack: Vec::new(),
                alarm_deadline_us: None,
                alarm_interval_us: None,
                rlimit_nofile: parent.rlimit_nofile,
                umask: parent.umask,
                alive_thread_count: 1,
            }),
        });
        // add child
        parent.children.push(Arc::clone(&child));
        let kstack = kstack_alloc();

        let vmset = UserVMSet::from_existed_user_cow(&mut parent.vm_set);

        child.inner_exclusive_access().vm_set = vmset;
        fork_inherit_shm_attach(&child.inner_exclusive_access().vm_set.areas, child.getpid());

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
        // 子进程 fork 返回 0
        trap_cx[TrapFrameArgs::RET] = 0;

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

    pub fn getpgid(&self) -> usize {
        self.inner_exclusive_access().pgid.0
    }

    pub fn setpgid(&self, pgid: usize) {
        self.inner_exclusive_access().pgid = PgidHandle(pgid);
    }

    #[cfg(target_arch = "riscv64")]
    pub fn _clone(
        self: &Arc<Self>,
        _flags: u32,
        _stack: usize,
        _ptid: usize,
        _ctid: usize,
        _tls: usize,
    ) -> isize {
        disable_timer_interrupt();
        if (_flags & CLONE_THREAD) != 0 {
            // 线程创建路径：共享进程、地址空间、fd_table 等
            info!(
                "_clone thread: flags={:#x}, stack={:#x}, ptid={:#x}, ctid={:#x}, tls={:#x}",
                _flags, _stack, _ptid, _ctid, _tls
            );
            let caller_task = crate::task::current_task().unwrap();

            // 1. 先读取 ustack_base，释放进程锁后再创建 TaskControlBlock
            //    避免在持有进程锁时调用 TaskControlBlock::new（内部会再次获取进程锁）
            //    同时也避免 process.inner -> task.inner 的锁顺序，防止与 exit_current_and_run_next 死锁。
            let ustack_base = {
                let inner = self.inner_exclusive_access();
                let task0 = inner.get_task(0).clone();
                drop(inner);
                task0
                    .inner_exclusive_access()
                    .res
                    .as_ref()
                    .unwrap()
                    .ustack_base()
            };

            let kstack = kstack_alloc();
            let task = Arc::new(TaskControlBlock::new(
                Arc::clone(self),
                ustack_base,
                true,
                kstack,
            ));
            let tid = task.inner_exclusive_access().res.as_ref().unwrap().tid;

            // 2. 将新线程加入当前进程的 tasks
            {
                let mut parent_inner = self.inner_exclusive_access();
                parent_inner.alive_thread_count += 1;
                let tasks = &mut parent_inner.tasks;
                while tasks.len() < tid + 1 {
                    tasks.push(None);
                }
                tasks[tid] = Some(Arc::clone(&task));
            }

            // 3. 设置 clear_child_tid
            if _ctid != 0 {
                let mut t_inner = task.inner_exclusive_access();
                t_inner.clear_child_tid = _ctid;
            }

            // 4. CLONE_PARENT_SETTID：将 tid 写入 ptid 指向的用户地址
            if _ptid != 0 && (_flags & CLONE_PARENT_SETTID) != 0 {
                let token = crate::task::current_user_token();
                let mut buf = match crate::mm::translated_byte_buffer(
                    token,
                    _ptid as *const u8,
                    core::mem::size_of::<i32>(),
                ) {
                    Ok(buf) => buf,
                    Err(_) => return -(SysError::EFAULT.code() as isize),
                };
                if !buf.is_empty() && buf[0].len() >= 4 {
                    buf[0][0..4].copy_from_slice(&(tid as i32).to_ne_bytes());
                }
            }

            // 5. 设置 trap_cx
            {
                let mut task_inner = task.inner_exclusive_access();
                let trap_cx = task_inner.get_trap_cx();
                trap_cx.clone_from(&caller_task.inner_exclusive_access().trap_cx);
                if _stack != 0 {
                    info!("_clone thread: set sp to {:#x}", _stack);
                    trap_cx[TrapFrameArgs::SP] = _stack;
                }
                if (_flags & CLONE_SETTLS) != 0 {
                    trap_cx[TrapFrameArgs::TLS] = _tls;
                }
                trap_cx[TrapFrameArgs::RET] = 0; // 子线程 clone 返回 0
                task_inner.blocked_signals =
                    caller_task.inner_exclusive_access().blocked_signals.clone();
            }

            add_task(task);
            info!("_clone thread: created tid {}", tid);
            set_next_trigger();
            tid as isize
        } else {
            // fork 路径：创建新进程
            info!("enter fork");
            let mut parent = self.inner_exclusive_access();
            assert_eq!(parent.thread_count(), 1);
            let memory_set = UserVMSet::new_bare();
            let pid = pid_alloc();
            let mut new_fd_table: Vec<Option<Arc<dyn File + Send + Sync>>> = Vec::new();
            for fd in parent.fd_table.iter() {
                if let Some(file) = fd {
                    new_fd_table.push(Some(file.clone()));
                } else {
                    new_fd_table.push(None);
                }
            }
            let child = Arc::new(Self {
                pid,
                inner: SpinNoIrqLock::new(ProcessControlBlockInner {
                    uid: parent.uid,
                    euid: parent.euid,
                    suid: parent.suid,
                    gid: parent.gid,
                    egid: parent.egid,
                    sgid: parent.sgid,
                    is_zombie: false,
                    zombie_flag: AtomicBool::new(false),
                    pgid: parent.pgid,
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
                    kstart: current_time().as_micros() as usize,
                    state: ProcessStatus::Ready,
                    pending_signals: SignalSet::empty(),
                    blocked_signals: parent.blocked_signals.clone(),
                    signals_handler: parent.signals_handler.clone(),
                    need_signal_handle: false,
                    itimer_real_deadline: None,
                    itimer_real_interval: None,
                    wait_waker: None,
                    sig_context_stack: Vec::new(),
                    alarm_deadline_us: None,
                    alarm_interval_us: None,
                    rlimit_nofile: parent.rlimit_nofile,
                    umask: parent.umask,
                    alive_thread_count: 1,
                    
                }),
            });
            parent.children.push(Arc::clone(&child));
            let kstack = kstack_alloc();
            let vmset = UserVMSet::from_existed_user_cow(&mut parent.vm_set);
            child.inner_exclusive_access().vm_set = vmset;
            fork_inherit_shm_attach(&child.inner_exclusive_access().vm_set.areas, child.getpid());
            let task = Arc::new(TaskControlBlock::new(
                Arc::clone(&child),
                parent
                    .get_task(0)
                    .inner_exclusive_access()
                    .res
                    .as_ref()
                    .unwrap()
                    .ustack_base(),
                false,
                kstack,
            ));
            let mut child_inner = child.inner_exclusive_access();
            child_inner.tasks.push(Some(Arc::clone(&task)));
            drop(child_inner);
            let mut task_inner = task.inner_exclusive_access();
            let trap_cx = task_inner.get_trap_cx();
            let parent_task = parent.get_task(0);
            let parent_task_inner = parent_task.inner_exclusive_access();
            trap_cx.clone_from(&parent_task_inner.trap_cx);
            task_inner.blocked_signals = parent_task_inner.blocked_signals.clone();
            drop(parent_task_inner);
            if _stack != 0 {
                info!("_clone fork: set sp to {:#x}", _stack);
                trap_cx[TrapFrameArgs::SP] = _stack;
            }
            trap_cx[TrapFrameArgs::RET] = 0;
            drop(task_inner);
            insert_into_pid2process(child.getpid(), Arc::clone(&child));
            add_task(task);
            warn!(
                "fork a new process with pid {}, parent pid = {}",
                child.getpid(),
                self.getpid()
            );
            set_next_trigger();
            child.getpid() as isize
        }
    }

    #[cfg(target_arch = "loongarch64")]
    pub fn _clone(
        self: &Arc<Self>,
        _flags: u32,
        _stack: usize,
        _ptid: usize,
        _tls: usize,
        _ctid: usize,
    ) -> isize {
        disable_timer_interrupt();
        if (_flags & CLONE_THREAD) != 0 {
            // 线程创建路径：共享进程、地址空间、fd_table 等
            error!(
                "_clone thread: flags={:#x}, stack={:#x}, ptid={:#x}, ctid={:#x}, tls={:#x}",
                _flags, _stack, _ptid, _ctid, _tls
            );
            let caller_task = crate::task::current_task().unwrap();

            // 1. 先读取 ustack_base，释放进程锁后再创建 TaskControlBlock
            //    避免在持有进程锁时调用 TaskControlBlock::new（内部会再次获取进程锁）
            //    同时也避免 process.inner -> task.inner 的锁顺序，防止与 exit_current_and_run_next 死锁。
            let ustack_base = {
                let inner = self.inner_exclusive_access();
                let task0 = inner.get_task(0).clone();
                drop(inner);
                task0
                    .inner_exclusive_access()
                    .res
                    .as_ref()
                    .unwrap()
                    .ustack_base()
            };

            let kstack = kstack_alloc();
            let task = Arc::new(TaskControlBlock::new(
                Arc::clone(self),
                ustack_base,
                true,
                kstack,
            ));
            let tid = task.inner_exclusive_access().res.as_ref().unwrap().tid;

            // 2. 将新线程加入当前进程的 tasks
            {
                let mut parent_inner = self.inner_exclusive_access();
                parent_inner.alive_thread_count += 1;
                let tasks = &mut parent_inner.tasks;
                while tasks.len() < tid + 1 {
                    tasks.push(None);
                }
                tasks[tid] = Some(Arc::clone(&task));
            }

            // 3. 设置 clear_child_tid
            if _ctid != 0 {
                let mut t_inner = task.inner_exclusive_access();
                t_inner.clear_child_tid = _ctid;
            }

            // 4. CLONE_PARENT_SETTID：将 tid 写入 ptid 指向的用户地址
            if _ptid != 0 && (_flags & CLONE_PARENT_SETTID) != 0 {
                let token = crate::task::current_user_token();
                let mut buf = match crate::mm::translated_byte_buffer(
                    token,
                    _ptid as *const u8,
                    core::mem::size_of::<i32>(),
                ){
                    Ok(buf) => buf,
                    Err(_) => return -(SysError::EFAULT.code() as isize),
                };
                if !buf.is_empty() && buf[0].len() >= 4 {
                    buf[0][0..4].copy_from_slice(&(tid as i32).to_ne_bytes());
                }
            }

            // 5. 设置 trap_cx
            {
                let mut task_inner = task.inner_exclusive_access();
                let trap_cx = task_inner.get_trap_cx();
                trap_cx.clone_from(&caller_task.inner_exclusive_access().trap_cx);
                if _stack != 0 {
                    info!("_clone thread: set sp to {:#x}", _stack);
                    trap_cx[TrapFrameArgs::SP] = _stack;
                }
                if (_flags & CLONE_SETTLS) != 0 {
                    trap_cx[TrapFrameArgs::TLS] = _tls;
                }
                trap_cx[TrapFrameArgs::RET] = 0; // 子线程 clone 返回 0
                task_inner.blocked_signals =
                    caller_task.inner_exclusive_access().blocked_signals.clone();
            }

            add_task(task);
            info!("_clone thread: created tid {}", tid);
            set_next_trigger();
            tid as isize
        } else {
            // fork 路径：创建新进程
            info!("enter fork");
            let mut parent = self.inner_exclusive_access();
            assert_eq!(parent.thread_count(), 1);
            let memory_set = UserVMSet::new_bare();
            let pid = pid_alloc();
            let mut new_fd_table: Vec<Option<Arc<dyn File + Send + Sync>>> = Vec::new();
            for fd in parent.fd_table.iter() {
                if let Some(file) = fd {
                    new_fd_table.push(Some(file.clone()));
                } else {
                    new_fd_table.push(None);
                }
            }
            let child = Arc::new(Self {
                pid,
                inner: SpinNoIrqLock::new(ProcessControlBlockInner {
                    is_zombie: false,
                    zombie_flag: AtomicBool::new(false),
                    pgid: parent.pgid,
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
                    kstart: current_time().as_micros() as usize,
                    state: ProcessStatus::Ready,
                    pending_signals: SignalSet::empty(),
                    blocked_signals: parent.blocked_signals.clone(),
                    signals_handler: parent.signals_handler.clone(),
                    need_signal_handle: false,
                    itimer_real_deadline: None,
                    itimer_real_interval: None,
                    wait_waker: None,
                    sig_context_stack: Vec::new(),
                    alarm_deadline_us: None,
                    alarm_interval_us: None,
                    rlimit_nofile: parent.rlimit_nofile,
                    umask: parent.umask,
                    uid: parent.uid,
                    euid: parent.euid,
                    gid: parent.gid,
                    egid: parent.egid,
                    suid: parent.suid,
                    sgid: parent.sgid,
                    alive_thread_count: 1,
                }),
            });
            parent.children.push(Arc::clone(&child));
            let kstack = kstack_alloc();
            let vmset = UserVMSet::from_existed_user_cow(&mut parent.vm_set);
            child.inner_exclusive_access().vm_set = vmset;
            fork_inherit_shm_attach(&child.inner_exclusive_access().vm_set.areas, child.getpid());
            let task = Arc::new(TaskControlBlock::new(
                Arc::clone(&child),
                parent
                    .get_task(0)
                    .inner_exclusive_access()
                    .res
                    .as_ref()
                    .unwrap()
                    .ustack_base(),
                false,
                kstack,
            ));
            let mut child_inner = child.inner_exclusive_access();
            child_inner.tasks.push(Some(Arc::clone(&task)));
            drop(child_inner);
            let mut task_inner = task.inner_exclusive_access();
            let trap_cx = task_inner.get_trap_cx();
            let parent_task = parent.get_task(0);
            let parent_task_inner = parent_task.inner_exclusive_access();
            trap_cx.clone_from(&parent_task_inner.trap_cx);
            task_inner.blocked_signals = parent_task_inner.blocked_signals.clone();
            drop(parent_task_inner);
            if _stack != 0 {
                info!("_clone fork: set sp to {:#x}", _stack);
                trap_cx[TrapFrameArgs::SP] = _stack;
            }
            trap_cx[TrapFrameArgs::RET] = 0;
            drop(task_inner);
            insert_into_pid2process(child.getpid(), Arc::clone(&child));
            add_task(task);
            warn!(
                "fork a new process with pid {}, parent pid = {}",
                child.getpid(),
                self.getpid()
            );
            set_next_trigger();
            child.getpid() as isize
        }
    }
}

pub const CLONE_VM: u32 = 0x00000100; // 共享内存描述符
pub const CLONE_FS: u32 = 0x00000200; // 共享文件系统信息
pub const CLONE_FILES: u32 = 0x00000400; // 共享文件描述符表
pub const CLONE_SIGHAND: u32 = 0x00000800; // 共享信号处理函数表
pub const CLONE_PARENT_SETTID: u32 = 0x00100000; // 父进程设置 tid
pub const CLONE_CHILD_CLEARTID: u32 = 0x00200000; // 子进程退出时清零 tid
pub const CLONE_SETTLS: u32 = 0x00080000; // 设置 TLS
pub const CLONE_THREAD: u32 = 0x00010000; // 创建线程（同一线程组）
pub const CLONE_NEWNS: u32 = 0x00020000; // 新的挂载命名空间
pub const CLONE_NEWNET: u32 = 0x40000000; // 新的网络命名空间

pub const CLONE_THREAD_FLAGS: u32 =
    CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD;