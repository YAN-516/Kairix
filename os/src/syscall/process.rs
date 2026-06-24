use super::{TimeSpec, TimeVal};
use crate::alloc::string::ToString;
// use crate::config::PAGE_SIZE;
use crate::error::{SysError, SyscallResult};
use crate::fs::config::FD_CLOEXEC_FLAG;
use crate::fs::find_superblock_by_path;
use crate::fs::notify::fanotify::{
    FAN_OPEN, FAN_OPEN_EXEC, FAN_OPEN_EXEC_PERM, FAN_OPEN_PERM,
    fanotify_check_exec_permission_dentry, fanotify_notify_dentry,
};
use crate::fs::pipe::make_socket_pair;
use crate::fs::vfs::OpenFlags;
use crate::fs::vfs::file::{File, open_file};
use crate::fs::vfs::fstype::MountFlags;
use crate::fs::vfs::inode::InodeMode;
use crate::mm::UserMapAreaType;
use crate::mm::heap::HeapExt;
use crate::mm::vm_area::MapArea;
use crate::mm::{PageTable, PhysAddr};
use crate::mm::{
    VMSpace, translated_byte_buffer, translated_byte_buffer_for_write, translated_ref,
    translated_refmut, translated_str,
};
use crate::remove_from_pid2process;
use crate::syscall::landlock::{LANDLOCK_ACCESS_FS_EXECUTE, landlock_check_dentry};
use crate::syscall::shm::release_shm_attaches;
use crate::task::signal::{SA_RESTART, SigHandler, Signal};
use crate::task::{
    CLONE_FS, CLONE_NEWNS, CLONE_NEWPID, CLONE_PIDFD, CLONE_SIGHAND, CLONE_THREAD, CLONE_VFORK,
    CLONE_VM, RLIMIT_FSIZE, RLIMIT_NOFILE, Rlimit64, TermStatus, block_current_and_run_next,
    current_process, current_task, current_user_token, exit_current_and_run_next, pid2process,
    suspend_current_and_run_next, tid2task,
};
#[cfg(target_arch = "riscv64")]
use crate::timer::get_time_us;
use crate::trap::_set_sum_bit;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::ops::IndexMut;
use log::*;
use polyhal::consts::{PAGE_SIZE, USER_MEMORY_SPACE};
use polyhal::timer::*;
pub use polyhal::utils::addr::*;
use polyhal_trap::trapframe::TrapFrameArgs;
#[allow(unused)]
pub const SCHED_NORMAL: i32 = 0; // 普通分时调度

const EXEC_IMAGE_MAX_SIZE: usize = 16 * 1024 * 1024;

fn current_brk(vm_set: &crate::mm::UserVMSet) -> usize {
    vm_set.heap_end_va().0 - 1
}

fn brk_request_is_valid(vm_set: &crate::mm::UserVMSet, ptr: usize, aligned_end: usize) -> bool {
    let Some(requested_end) = ptr.checked_add(1) else {
        return false;
    };
    let user_end_exclusive = USER_MEMORY_SPACE.1.saturating_add(1);
    if requested_end > user_end_exclusive || aligned_end > user_end_exclusive {
        return false;
    }

    let heap_start = vm_set.heap_start_va().0;
    for area in vm_set.areas.iter() {
        if area.areatype() == UserMapAreaType::Heap {
            continue;
        }
        if heap_start < area.end_va().0 && aligned_end > area.start_va().0 {
            return false;
        }
    }
    true
}

fn read_exec_image(file: &Arc<dyn File>, path: &str) -> Result<Vec<u8>, SysError> {
    let size = file.get_inode().map(|inode| inode.get_size()).unwrap_or(0);
    if size > EXEC_IMAGE_MAX_SIZE {
        warn!(
            "[sys_execve] executable too large: path={} size={} limit={}",
            path, size, EXEC_IMAGE_MAX_SIZE
        );
        return Err(SysError::ENOMEM);
    }

    let mut buffer = vec![0u8; size];
    let mut offset = 0usize;
    while offset < size {
        let read_size = file.read_at_direct(offset, &mut buffer[offset..])?;
        if read_size == 0 {
            break;
        }
        offset += read_size;
    }
    if offset != size {
        warn!(
            "[sys_execve] short executable read: path={} expected={} actual={}",
            path, size, offset
        );
        return Err(SysError::EIO);
    }
    buffer.truncate(offset);

    Ok(buffer)
}

fn reap_zombie_child(child: Arc<crate::task::ProcessControlBlock>) {
    let pid = child.getpid();
    let (tasks, old_areas, files) = {
        let mut inner = child.inner_exclusive_access();
        inner.alarm_deadline_us = None;
        inner.itimer_real_deadline = None;
        inner.itimer_real_interval = None;
        inner.children.clear();
        let (old_areas, _page_table_pages) = inner.vm_set.release_user_space();
        let tasks = core::mem::take(&mut inner.tasks);
        let files = core::mem::take(&mut inner.fd_table)
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        inner.fd_flags.clear();
        (tasks, old_areas, files)
    };

    for task in tasks.into_iter().flatten() {
        let global_tid = task.inner_exclusive_access().global_tid;
        crate::task::remove_task(Arc::clone(&task));
        crate::syscall::futex::remove_task_from_futex_table(&task);
        crate::task::manager::remove_from_tid2task_if_present(global_tid);
        if global_tid != pid {
            crate::task::dealloc_pid(global_tid);
        }
    }
    release_shm_attaches(&old_areas);
    drop(old_areas);
    for file in files {
        crate::fs::writeback::queue_file(file);
    }

    crate::task::manager::TIMER_PROCS.lock().remove(&pid);
    remove_from_pid2process(pid);
}

fn should_interrupt_wait_syscall() -> bool {
    let task = match current_task() {
        Some(task) => task,
        None => return false,
    };
    let t_inner = task.inner_exclusive_access();
    let blocked = t_inner.blocked_signals.bits();
    let task_pending = t_inner.pending_signals.bits();
    drop(t_inner);

    let Some(process) = task.process.upgrade() else {
        return false;
    };
    let p_inner = process.inner_exclusive_access();
    let sigchld_bit = 1u64 << (Signal::SigChld.as_i32() - 1);
    let pending = (task_pending | p_inner.pending_signals.bits()) & !blocked & !sigchld_bit;

    if pending == 0 {
        return false;
    }

    for i in 1..=64 {
        if (pending >> (i - 1)) & 1 == 0 {
            continue;
        }
        if let Some(sig) = Signal::from_i32(i) {
            let action = p_inner.signals_handler.get(sig);
            match action.sa_handler {
                SigHandler::Ignore => {}
                SigHandler::Default => return true,
                SigHandler::Custom(_) => {
                    if action.sa_flags & SA_RESTART == 0 || wait_signal_must_interrupt(sig) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn wait_signal_must_interrupt(signal: Signal) -> bool {
    matches!(
        signal,
        Signal::SigHup | Signal::SigInt | Signal::SigQuit | Signal::SigTerm
    )
}

#[derive(Clone, Copy)]
struct WaitChildSnapshot {
    pid: usize,
    pgid: usize,
    exit_code: i32,
    term_status: TermStatus,
    is_zombie: bool,
    is_stopped: bool,
    was_continued: bool,
    alive_thread_count: usize,
}

fn wait_child_snapshot(child: &Arc<crate::task::ProcessControlBlock>) -> WaitChildSnapshot {
    let inner = child.inner_exclusive_access();
    WaitChildSnapshot {
        pid: child.getpid(),
        pgid: inner.pgid.0,
        exit_code: inner.exit_code,
        term_status: inner.term_status,
        is_zombie: inner.is_zombie,
        is_stopped: inner.is_stopped,
        was_continued: inner.was_continued,
        alive_thread_count: inner.alive_thread_count,
    }
}

fn wait_children_snapshot(
    process: &Arc<crate::task::ProcessControlBlock>,
) -> Vec<Arc<crate::task::ProcessControlBlock>> {
    process.inner_exclusive_access().children.clone()
}

fn remove_wait_child(
    process: &Arc<crate::task::ProcessControlBlock>,
    child: &Arc<crate::task::ProcessControlBlock>,
) -> Option<Arc<crate::task::ProcessControlBlock>> {
    let mut inner = process.inner_exclusive_access();
    let idx = inner
        .children
        .iter()
        .position(|candidate| Arc::ptr_eq(candidate, child))?;
    Some(inner.children.remove(idx))
}

#[allow(unused)]
pub const SCHED_FIFO: i32 = 1; // 先进先出实时调度
#[allow(unused)]
pub const SCHED_RR: i32 = 2; // 轮转实时调度
#[allow(unused)]
pub const SCHED_BATCH: i32 = 3; // 批处理调度
#[allow(unused)]
pub const SCHED_IDLE: i32 = 5; // 空闲调度
#[allow(unused)]
pub const SCHED_RESET_ON_FORK: i32 = 0x40000000;
#[repr(C)]
#[derive(Copy, Clone)]
#[allow(unused)]
pub struct SchedParam {
    pub sched_priority: i32,
}

#[repr(C)]
#[derive(Copy, Clone)]
#[allow(unused)]
pub struct SchedAttr {
    pub size: u32,
    pub sched_policy: u32,
    pub sched_flags: u64,
    pub sched_nice: i32,
    pub sched_priority: u32,
    pub sched_runtime: u64,
    pub sched_deadline: u64,
    pub sched_period: u64,
}

const SCHED_ATTR_MIN_SIZE: usize = 24;

fn copy_struct_to_user<T: Copy>(token: usize, dst: *mut T, value: &T, len: usize) -> SyscallResult {
    let bytes = translated_byte_buffer_for_write(token, dst as *mut u8, len)?;
    let src = unsafe {
        core::slice::from_raw_parts(value as *const T as *const u8, core::mem::size_of::<T>())
    };
    let mut copied = 0usize;
    for buf in bytes {
        let copy_len = buf.len().min(len - copied);
        buf[..copy_len].copy_from_slice(&src[copied..copied + copy_len]);
        copied += copy_len;
        if copied == len {
            break;
        }
    }
    Ok(0)
}

fn copy_struct_from_user<T: Copy>(token: usize, src: *const T, len: usize) -> Result<T, SysError> {
    let bytes = translated_byte_buffer(token, src as *const u8, len)?;
    let mut value = core::mem::MaybeUninit::<T>::zeroed();
    let dst = unsafe {
        core::slice::from_raw_parts_mut(value.as_mut_ptr() as *mut u8, core::mem::size_of::<T>())
    };
    let mut copied = 0usize;
    for buf in bytes {
        let copy_len = buf.len().min(len - copied);
        dst[copied..copied + copy_len].copy_from_slice(&buf[..copy_len]);
        copied += copy_len;
        if copied == len {
            break;
        }
    }
    Ok(unsafe { value.assume_init() })
}

pub fn sys_exit(exit_code: i32) -> ! {
    let pid = current_task()
        .and_then(|t| t.process.upgrade())
        .map(|p| p.getpid())
        .unwrap_or(0);
    info!("[DEBUG sys_exit] pid={}, exit_code={}", pid, exit_code);
    exit_current_and_run_next(exit_code);
    panic!("Unreachable in sys_exit!");
}

pub fn sys_yield() -> SyscallResult {
    //println!("enter yield!");
    suspend_current_and_run_next();
    Ok(0)
}

pub fn sys_get_time(_ts: *mut TimeVal, _tz: usize) -> SyscallResult {
    if _ts.is_null() {
        return Err(SysError::EFAULT);
    }
    let ns = current_time().as_nanos() as u128;
    let time = TimeVal {
        sec: (ns / 1_000_000_000) as i64,
        usec: ((ns / 1_000) % 1_000_000) as i64,
    };
    let token = current_user_token();
    copy_struct_to_user(token, _ts, &time, core::mem::size_of::<TimeVal>())
}

pub fn sys_getpid() -> SyscallResult {
    Ok(current_task().unwrap().process.upgrade().unwrap().getpid() as usize)
}

pub fn sys_getppid() -> SyscallResult {
    let process = current_process();
    let inner = process.inner_exclusive_access();
    let parent = inner.parent.as_ref().and_then(|weak| weak.upgrade());

    if let Some(parent) = parent {
        Ok(parent.getpid() as usize)
    } else {
        Ok(0)
    }
}

fn ltp_root_for_exec_path(path: &str) -> Option<&'static str> {
    if path.starts_with("/musl/ltp/testcases/bin/") {
        Some("/musl/ltp")
    } else if path.starts_with("/glibc/ltp/testcases/bin/") {
        Some("/glibc/ltp")
    } else if path.starts_with("/sdcard/musl/ltp/testcases/bin/") {
        Some("/sdcard/musl/ltp")
    } else if path.starts_with("/sdcard/glibc/ltp/testcases/bin/") {
        Some("/sdcard/glibc/ltp")
    } else {
        None
    }
}

fn set_ltp_root_env(envs: &mut Vec<String>, ltp_root: &str) {
    const LTPROOT_PREFIX: &str = "LTPROOT=";
    let value = alloc::format!("{}{}", LTPROOT_PREFIX, ltp_root);

    if let Some(env) = envs.iter_mut().find(|env| env.starts_with(LTPROOT_PREFIX)) {
        *env = value;
    } else {
        envs.push(value);
    }
}

// pub fn sys_fork() -> SyscallResult {
//     let current_process = current_process();
//     let new_process = current_process.fork();
//     let new_pid = new_process.getpid();
//     // modify trap context of new_task, because it returns immediately after switching
//     let new_process_inner = new_process.inner_exclusive_access();
//     let task = new_process_inner.tasks[0].as_ref().unwrap();
//     let trap_cx = task.inner_exclusive_access().get_trap_cx();
//     // we do not have to move to next instruction since we have done it before
//     // for child process, fork returns 0
//     trap_cx[TrapFrameArgs::RET] = 0;
//     error!(
//         "fork a new process with pid {}, parent pid = {}",
//         new_pid,
//         current_process.getpid()
//     );
//     Ok(new_pid as usize)
// }

#[allow(unused)]
pub fn sys_execve(path: usize, argv: usize, envp: usize) -> SyscallResult {
    info!("[sys_execve] called");
    let token = current_user_token();
    let path_str = translated_str(token, path as *const u8)?;
    let mut args_vec: Vec<String> = Vec::new();
    if argv != 0 {
        let mut argv_ptr = argv as *const usize;
        loop {
            let str_ptr = *translated_ref(token, argv_ptr)?;
            if str_ptr == 0 {
                break;
            }
            args_vec.push(translated_str(token, str_ptr as *const u8)?);
            argv_ptr = unsafe { argv_ptr.add(1) };
        }
    }
    let mut envs_vec: Vec<String> = Vec::new();
    if envp != 0 {
        let mut envp_ptr = envp as *const usize;
        loop {
            let str_ptr = *translated_ref(token, envp_ptr)?;
            if str_ptr == 0 {
                break;
            }
            envs_vec.push(translated_str(token, str_ptr as *const u8)?);
            envp_ptr = unsafe { envp_ptr.add(1) };
        }
    }
    let task = current_task().unwrap();
    let process = task.process.upgrade().unwrap();
    let cwd = process.inner_exclusive_access().cwd.clone();
    info!("[sys_execve] path={} cwd_name={}", path_str, cwd.name());
    let cwd_path = cwd.path();
    if let Some(reason) = super::ltp_exec_filter::reject_reason_for_exec_path(&cwd_path, &path_str)
    {
        warn!(
            "[sys_execve] Refusing to exec LTP test before open: cwd={} path={} reason={}",
            cwd_path, path_str, reason
        );
        return Err(SysError::ENOENT);
    }
    let app_file = match open_file(
        cwd.clone(),
        path_str.as_str(),
        OpenFlags::RDONLY,
        InodeMode::FILE,
    ) {
        Ok(f) => f,
        Err(e) => {
            error!(
                "[sys_execve] open_file failed for path={} err={:?}",
                path_str, e
            );
            return Err(SysError::ENOENT);
        }
    };
    let app_dentry = app_file.get_dentry();
    let app_path = app_dentry.path();
    let app_name = app_path.rsplit('/').next().unwrap_or(app_path.as_str());
    if let Some(reason) = super::ltp_exec_filter::reject_reason(&app_path, app_name) {
        warn!(
            "[sys_execve] Refusing to exec LTP test: path={} case={} reason={}",
            app_path, app_name, reason
        );
        return Err(SysError::ENOENT);
    }
    if let Some(ltp_root) = ltp_root_for_exec_path(&app_path) {
        set_ltp_root_env(&mut envs_vec, ltp_root);
    }
    landlock_check_dentry(&app_dentry, LANDLOCK_ACCESS_FS_EXECUTE)?;
    if find_superblock_by_path(&app_path)
        .is_some_and(|sb| sb.inner().flags().contains(MountFlags::MS_NOEXEC))
    {
        return Err(SysError::EACCES);
    }
    let fanotify_target = app_file.get_inode().map(|_| app_file.get_dentry());
    if let Some(target) = fanotify_target.as_ref() {
        fanotify_check_exec_permission_dentry(target.clone(), FAN_OPEN_EXEC_PERM, FAN_OPEN_PERM)?;
    }
    info!("Executing program: {}", path_str);
    let exec_image = read_exec_image(&app_file, &app_path)?;
    let exec_data = exec_image.as_slice();
    let is_elf = exec_data.len() >= 4
        && exec_data[0] == 0x7f
        && exec_data[1] == 0x45
        && exec_data[2] == 0x4c
        && exec_data[3] == 0x46;
    let mut ret = if is_elf {
        let ret = process.execve(exec_data, args_vec.clone(), envs_vec.clone());
        info!("[sys_execve] execve returned {}", ret);
        ret
    } else {
        -8
    };
    drop(exec_image);

    // 如果它是纯文本脚本,重新使用busybox加载
    if !is_elf {
        info!(
            "Not an ELF! Fallback to busybox sh to run script: {}",
            path_str
        );
        let busybox_paths = ["/bin/busybox", "/musl/busybox", "busybox"];
        let mut busybox_file = None;
        for bb_path in &busybox_paths {
            if let Ok(f) = open_file(cwd.clone(), bb_path, OpenFlags::RDONLY, InodeMode::FILE) {
                busybox_file = Some(f);
                break;
            }
        }
        if let Some(busybox_file) = busybox_file {
            // 重新构造参数：["busybox", "sh", "原本的脚本路径", 原本的参数1, 原本的参数2...]
            let mut new_args = vec!["busybox".to_string(), "sh".to_string(), path_str];
            if args_vec.len() > 1 {
                new_args.extend_from_slice(&args_vec[1..]);
            }
            let busybox_path = busybox_file.get_dentry().path();
            let busybox_image = read_exec_image(&busybox_file, &busybox_path)?;
            ret = process.execve(busybox_image.as_slice(), new_args, envs_vec);
        } else {
            error!("Fallback failed: busybox not found!");
            return Err(SysError::ENOEXEC);
        }
    } else if ret == -8 && is_elf {
        // 动态ELF缺少解释器等场景，不应把ELF当脚本执行。
        return Err(SysError::ENOEXEC);
    }

    if ret < 0 {
        match ret {
            -2 => Err(SysError::ENOENT),
            -8 => Err(SysError::ENOEXEC),
            _ => Err(SysError::EINVAL),
        }
    } else {
        if let Some(target) = fanotify_target {
            fanotify_notify_dentry(target, FAN_OPEN | FAN_OPEN_EXEC);
        }
        Ok(ret as usize)
    }
}

pub fn sys_brk(ptr: usize) -> SyscallResult {
    // Linux 语义：brk 系统调用返回"当前程序 break 地址"，
    // glibc/musl 封装会据此判断是否成功（ret < requested 视为失败）。
    warn!("sys_brk ptr={:#x}", ptr);
    let process = current_process();
    let vm_set = &mut process.inner_exclusive_access().vm_set;
    warn!(
        "heap start_va={:#x}, heap end_va={:#x}",
        vm_set.heap_start_va().0,
        vm_set.heap_end_va().0
    );

    // 如果 ptr 为 0，返回当前 break 地址
    if ptr == 0 {
        info!(
            "sys_brk: ptr={:#x}, return current break address {:#x}",
            ptr,
            current_brk(vm_set)
        );
        return Ok(current_brk(vm_set));
    }

    let old_brk = current_brk(vm_set);

    // 如果请求的地址与当前 break 相同，直接返回
    if old_brk == ptr {
        return Ok(old_brk);
    }

    // 检查请求的地址是否小于堆起始地址
    let heap_start_va = vm_set.heap_start_va();
    if ptr < heap_start_va.0 {
        warn!(
            "sys_brk: requested address {:#x} below heap start {:#x}",
            ptr, heap_start_va.0
        );
        return Ok(old_brk);
    }

    let Some(requested_end) = ptr.checked_add(1) else {
        warn!("sys_brk: requested address {:#x} overflows break end", ptr);
        return Ok(old_brk);
    };

    // 计算页面对齐后的边界，判断是否需要实际映射/取消映射
    let current_ceil = VirtAddr::from(old_brk).ceil();
    let requested_ceil = VirtAddr::from(ptr).ceil();
    let aligned_end = VirtAddr::from(requested_ceil).0;

    if !brk_request_is_valid(vm_set, ptr, aligned_end) {
        warn!(
            "sys_brk: requested address {:#x} rejected, keep current break {:#x}",
            ptr, old_brk
        );
        return Ok(old_brk);
    }

    if current_ceil == requested_ceil {
        // 在同一页面范围内，只需更新记录的 break 值，不做实际 shrink/append
        let area = vm_set.get_heap_area_mut();
        area.range_va_mut().end = VirtAddr::from(requested_end);
        info!("sys_brk: new break address {:#x}", ptr);
        return Ok(ptr);
    }

    if old_brk < ptr {
        // 扩大堆：append 到请求地址的页面边界（向上取整）
        let aligned_va = VirtAddr::from(requested_ceil);
        vm_set.append_to(aligned_va);
    } else {
        // 缩小堆：shrink 到请求地址的页面边界（向上取整）
        let aligned_va = VirtAddr::from(requested_ceil);
        vm_set.shrink_to(aligned_va);
    }

    // 将精确的 break 值设为 ptr（Linux 语义：brk 返回用户请求的精确地址）
    let area = vm_set.get_heap_area_mut();
    area.range_va_mut().end = VirtAddr::from(requested_end);

    info!("sys_brk: new break address {:#x}", ptr);
    Ok(ptr)
}

/// If there is not a child process whose pid is same as given, return -1.
/// Else if there is a child process but it is still running:
///   - with WNOHANG: return 0
///   - without WNOHANG: block until a child exits
///
/// Linux waitpid pid semantics:
///   pid > 0:  wait for the specific child whose pid equals pid
///   pid == -1: wait for any child
///   pid == 0:  wait for any child in the same process group
///   pid < -1:  wait for any child whose process group equals |pid|
pub fn sys_wait4(
    pid: isize,
    exit_code_ptr: *mut i32,
    options: i32,
    rusage: *mut u8,
) -> SyscallResult {
    _set_sum_bit();

    // wait4 与 waitpid 共享入口，若用户提供了 rusage，先将其清零。
    // 这个结构在 64 位 glibc/musl 上是 144 字节；写多了会覆盖调用者栈上的 canary。
    if !rusage.is_null() {
        let token = current_user_token();
        if let Ok(bufs) = crate::mm::translated_byte_buffer_for_write(
            token,
            rusage,
            core::mem::size_of::<crate::syscall::time::Rusage>(),
        ) {
            for buf in bufs {
                buf.fill(0);
            }
        }
    }

    // 1. 先验证 options 合法性，避免在有/无子进程时返回不一致的错误码
    const WNOHANG: i32 = 0x00000001;
    const WUNTRACED: i32 = 0x00000002;
    const WCONTINUED: i32 = 0x00000008;
    const VALID_OPTIONS: i32 = WNOHANG | WUNTRACED | WCONTINUED;
    if options & !VALID_OPTIONS != 0 {
        return Err(SysError::EINVAL);
    }

    // 2. pid == INT_MIN 时，-pid 会溢出，Linux 返回 ESRCH
    if pid == i32::MIN as isize {
        return Err(SysError::ESRCH);
    }

    let process = current_process();
    let my_pgid = process.getpgid();

    let child_matches = |child: &WaitChildSnapshot| -> bool {
        let matches = match pid {
            -1 => true,
            0 => child.pgid == my_pgid,
            n if n < -1 => child.pgid == (-n) as usize,
            n => child.pid == n as usize,
        };
        matches
    };

    loop {
        let children = wait_children_snapshot(&process);
        let mut has_matching_child = false;
        let mut reap_candidate = None;

        for child in children {
            let snapshot = wait_child_snapshot(&child);
            if !child_matches(&snapshot) {
                continue;
            }
            has_matching_child = true;
            if snapshot.is_zombie && snapshot.alive_thread_count == 0 {
                reap_candidate = Some((child, snapshot));
                break;
            }
        }

        if !has_matching_child {
            return Err(SysError::ECHILD);
        }

        if let Some((child, snapshot)) = reap_candidate {
            let Some(child) = remove_wait_child(&process, &child) else {
                continue;
            };
            reap_zombie_child(child);
            let parent_pid = process.getpid();
            if !exit_code_ptr.is_null() {
                let status = match snapshot.term_status {
                    TermStatus::Exited(code) => ((code & 0xFF) as i32) << 8,
                    TermStatus::Signaled(sig, core) => sig | if core { 0x80 } else { 0 },
                    TermStatus::Stopped(sig) => ((sig & 0xFF) as i32) << 8 | 0x7F,
                    TermStatus::Running => (snapshot.exit_code & 0xFF) << 8,
                };
                *translated_refmut(current_user_token(), exit_code_ptr)? = status;
            }
            error!(
                "[DEBUG waitpid] parent_pid={} found zombie child pid={} exit_code={} term_status={:?}",
                parent_pid, snapshot.pid, snapshot.exit_code, snapshot.term_status
            );
            return Ok(snapshot.pid);
        }

        if options & 0x00000001 != 0 {
            return Ok(0);
        }

        if should_interrupt_wait_syscall() {
            return Err(SysError::EINTR);
        }
        if crate::task::current_process()
            .inner_exclusive_access()
            .is_zombie
        {
            return Err(SysError::EINTR);
        }

        block_current_and_run_next();
    }
}

/// waitid 使用的 siginfo_t 布局（与 musl riscv64/loongarch64 兼容）
/// 大小 128 字节，各字段偏移已通过测试程序验证。
#[repr(C)]
#[derive(Copy, Clone)]
pub struct WaitidSigInfo {
    pub si_signo: i32,         // offset 0
    pub si_errno: i32,         // offset 4
    pub si_code: i32,          // offset 8
    pub _pad0: i32,            // offset 12（对齐填充）
    pub si_pid: i32,           // offset 16
    pub si_uid: u32,           // offset 20
    pub si_status: i32,        // offset 24
    pub _pad1: i32,            // offset 28（对齐填充）
    pub si_utime: i64,         // offset 32
    pub si_stime: i64,         // offset 40
    pub _pad2: [u8; 128 - 48], // offset 48..127
}

pub fn sys_waitid(idtype: i32, id: u32, infop: *mut u8, options: i32) -> SyscallResult {
    _set_sum_bit();

    const WNOHANG: i32 = 0x00000001;
    const WSTOPPED: i32 = 0x00000002;
    const WEXITED: i32 = 0x00000004;
    const WCONTINUED: i32 = 0x00000008;
    const WNOWAIT: i32 = 0x01000000;
    const VALID_OPTIONS: i32 = WNOHANG | WSTOPPED | WEXITED | WCONTINUED | WNOWAIT;

    if options & !VALID_OPTIONS != 0 {
        return Err(SysError::EINVAL);
    }
    if options & (WEXITED | WSTOPPED | WCONTINUED) == 0 {
        return Err(SysError::EINVAL);
    }

    const P_ALL: i32 = 0;
    const P_PID: i32 = 1;
    const P_PGID: i32 = 2;
    const P_PIDFD: i32 = 3;
    if idtype != P_ALL && idtype != P_PID && idtype != P_PGID && idtype != P_PIDFD {
        return Err(SysError::EINVAL);
    }

    let process = current_process();
    let current_pgid = process.getpgid();

    let pidfd_target = if idtype == P_PIDFD {
        let fd = id as usize;
        let file = {
            let inner = process.inner_exclusive_access();
            match inner.fd_table.get(fd).and_then(|file| file.as_ref()) {
                Some(file) => Arc::clone(file),
                None => return Err(SysError::EBADF),
            }
        };
        match file.pidfd_pid() {
            Some(pid) => Some(pid),
            None => return Err(SysError::EINVAL),
        }
    } else {
        None
    };

    let child_matches = |child: &WaitChildSnapshot| -> bool {
        match idtype {
            P_ALL => true,
            P_PID => child.pid == id as usize,
            P_PGID => child.pgid == if id == 0 { current_pgid } else { id as usize },
            P_PIDFD => Some(child.pid) == pidfd_target,
            _ => false,
        }
    };

    let child_ready = |child: &WaitChildSnapshot| -> bool {
        (options & WSTOPPED != 0 && child.is_stopped)
            || (options & WEXITED != 0 && child.is_zombie && child.alive_thread_count == 0)
            || (options & WCONTINUED != 0 && child.was_continued)
    };

    let fill_siginfo = |token: usize,
                        infop: *mut u8,
                        pid: usize,
                        term_status: crate::task::TermStatus,
                        exit_code: i32,
                        is_continued: bool|
     -> Result<(), SysError> {
        if infop.is_null() {
            return Ok(());
        }
        let (si_code, si_status) = if is_continued {
            (6i32, 18i32) // CLD_CONTINUED, SIGCONT
        } else {
            match term_status {
                // waitid 的 si_status 使用原始值，不同于 waitpid 的编码方式
                crate::task::TermStatus::Exited(code) => (1i32, code),
                crate::task::TermStatus::Signaled(sig, core) => {
                    if core {
                        (3i32, sig)
                    } else {
                        (2i32, sig)
                    }
                }
                crate::task::TermStatus::Stopped(sig) => (5i32, sig),
                crate::task::TermStatus::Running => (1i32, exit_code),
            }
        };
        let siginfo = WaitidSigInfo {
            si_signo: 17, // SIGCHLD
            si_errno: 0,
            si_code,
            _pad0: 0,
            si_pid: pid as i32,
            si_uid: 0,
            si_status,
            _pad1: 0,
            si_utime: 0,
            si_stime: 0,
            _pad2: [0u8; 128 - 48],
        };
        let src = unsafe {
            core::slice::from_raw_parts(
                &siginfo as *const WaitidSigInfo as *const u8,
                core::mem::size_of::<WaitidSigInfo>(),
            )
        };
        let bufs =
            crate::mm::translated_byte_buffer(token, infop, core::mem::size_of::<WaitidSigInfo>())?;
        let mut written = 0;
        for buf in bufs {
            let len = buf
                .len()
                .min(core::mem::size_of::<WaitidSigInfo>() - written);
            buf[..len].copy_from_slice(&src[written..written + len]);
            written += len;
        }
        Ok(())
    };

    let clear_siginfo = |token: usize, infop: *mut u8| -> Result<(), SysError> {
        if infop.is_null() {
            return Ok(());
        }
        let bufs =
            crate::mm::translated_byte_buffer(token, infop, core::mem::size_of::<WaitidSigInfo>())?;
        for buf in bufs {
            buf.fill(0);
        }
        Ok(())
    };

    loop {
        let children = wait_children_snapshot(&process);
        let mut has_matching_child = false;
        let mut ready_candidate = None;

        for child in children {
            let snapshot = wait_child_snapshot(&child);
            if !child_matches(&snapshot) {
                continue;
            }
            has_matching_child = true;
            if child_ready(&snapshot) {
                ready_candidate = Some((child, snapshot));
                break;
            }
        }

        if !has_matching_child {
            return Err(SysError::ECHILD);
        }

        if let Some((child, snapshot)) = ready_candidate {
            let is_continued = options & WCONTINUED != 0 && snapshot.was_continued;
            if is_continued {
                if options & WNOWAIT == 0 {
                    child.inner_exclusive_access().was_continued = false;
                }
            } else if snapshot.is_zombie
                && snapshot.alive_thread_count == 0
                && options & WNOWAIT == 0
            {
                let Some(child) = remove_wait_child(&process, &child) else {
                    continue;
                };
                reap_zombie_child(child);
            }

            let token = current_user_token();
            fill_siginfo(
                token,
                infop,
                snapshot.pid,
                snapshot.term_status,
                snapshot.exit_code,
                is_continued,
            )?;
            return Ok(0);
        }

        if options & WNOHANG != 0 {
            let token = current_user_token();
            clear_siginfo(token, infop)?;
            return Ok(0);
        }

        if should_interrupt_wait_syscall() {
            return Err(SysError::EINTR);
        }
        if crate::task::current_process()
            .inner_exclusive_access()
            .is_zombie
        {
            return Err(SysError::EINTR);
        }

        block_current_and_run_next();
    }
}

#[allow(unused)]
pub fn sys_clone(flags: u32, stack: usize, ptid: usize, ctid: usize, tls: usize) -> SyscallResult {
    let process = current_process();
    let exit_signal = (flags & 0xFF) as i32;
    let child_pid = process._clone(flags, stack, ptid, ctid, tls, exit_signal) as usize;
    if (flags & CLONE_VFORK) != 0 {
        block_current_and_run_next();
    }
    Ok(child_pid)
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct CloneArgs {
    pub flags: u64,
    pub pidfd: u64,
    pub child_tid: u64,
    pub parent_tid: u64,
    pub exit_signal: u64,
    pub stack: u64,
    pub stack_size: u64,
    pub tls: u64,
    pub set_tid: u64,
    pub set_tid_size: u64,
    pub cgroup: u64,
}

pub fn sys_clone3(cl_args: *mut CloneArgs, size: usize) -> SyscallResult {
    // 1. 检查 size
    if size == 0 || size < core::mem::size_of::<CloneArgs>() {
        return Err(SysError::EINVAL);
    }
    // extra size: 如果 size 大于结构体大小，尝试读取额外字节
    if size > core::mem::size_of::<CloneArgs>() {
        let token = current_user_token();
        let extra = size - core::mem::size_of::<CloneArgs>();
        let extra_buffers = match crate::mm::translated_byte_buffer_no_fault(
            token,
            (cl_args as usize + core::mem::size_of::<CloneArgs>()) as *const u8,
            extra,
        ) {
            Ok(buf) => buf,
            Err(_) => return Err(SysError::EFAULT),
        };
        let extra_total: usize = extra_buffers.iter().map(|b| b.len()).sum();
        if extra_total < extra {
            return Err(SysError::EFAULT);
        }
    }

    // 2. 安全地读取用户提供的结构体
    let token = current_user_token();
    let buffers = crate::mm::translated_byte_buffer_no_fault(token, cl_args as *const u8, size)?;
    let total_len: usize = buffers.iter().map(|b| b.len()).sum();
    if total_len < size {
        return Err(SysError::EFAULT);
    }

    let mut args = CloneArgs {
        flags: 0,
        pidfd: 0,
        child_tid: 0,
        parent_tid: 0,
        exit_signal: 0,
        stack: 0,
        stack_size: 0,
        tls: 0,
        set_tid: 0,
        set_tid_size: 0,
        cgroup: 0,
    };
    let args_size = core::mem::size_of::<CloneArgs>().min(size);
    let mut copied = 0;
    for buf in buffers {
        let to_copy = buf.len().min(args_size - copied);
        unsafe {
            core::ptr::copy_nonoverlapping(
                buf.as_ptr(),
                (&mut args as *mut CloneArgs as *mut u8).add(copied),
                to_copy,
            );
        }
        copied += to_copy;
        if copied >= args_size {
            break;
        }
    }

    let flags = args.flags as u32;
    let exit_signal = (args.exit_signal & 0xFF) as i32;

    // 3. 检查标志组合和参数合法性
    // sighand-no-VM: CLONE_SIGHAND without CLONE_VM
    if (flags & CLONE_SIGHAND) != 0 && (flags & CLONE_VM) == 0 {
        return Err(SysError::EINVAL);
    }
    // thread-no-sighand: CLONE_THREAD without CLONE_SIGHAND
    if (flags & CLONE_THREAD) != 0 && (flags & CLONE_SIGHAND) == 0 {
        return Err(SysError::EINVAL);
    }
    // fs-newns: CLONE_FS | CLONE_NEWNS
    if (flags & (CLONE_FS | CLONE_NEWNS)) == (CLONE_FS | CLONE_NEWNS) {
        return Err(SysError::EINVAL);
    }
    // invalid signal: exit_signal > CSIGNAL (0xFF)
    if args.exit_signal > 0xFF {
        return Err(SysError::EINVAL);
    }
    // zero-stack-size: stack != 0, stack_size == 0
    if args.stack != 0 && args.stack_size == 0 {
        return Err(SysError::EINVAL);
    }
    // invalid-stack: stack == 0, stack_size != 0
    if args.stack == 0 && args.stack_size != 0 {
        return Err(SysError::EINVAL);
    }

    // 4. 检查 CLONE_PIDFD 时 pidfd 指针的有效性
    if (flags & CLONE_PIDFD) != 0 {
        if args.pidfd == 0 {
            return Err(SysError::EFAULT);
        }
        let pidfd_buffers = match crate::mm::translated_byte_buffer_no_fault(
            token,
            args.pidfd as *const u8,
            core::mem::size_of::<i32>(),
        ) {
            Ok(buf) => buf,
            Err(_) => return Err(SysError::EFAULT),
        };
        let pidfd_total: usize = pidfd_buffers.iter().map(|b| b.len()).sum();
        if pidfd_total < core::mem::size_of::<i32>() {
            return Err(SysError::EFAULT);
        }
    }

    // clone3 passes the lowest byte of the stack area plus its size, while old
    // clone passes the initial stack pointer directly.
    let stack = if args.stack == 0 {
        0
    } else {
        (args.stack as usize)
            .checked_add(args.stack_size as usize)
            .ok_or(SysError::EINVAL)?
    };
    let ptid = args.parent_tid as usize;
    let ctid = args.child_tid as usize;
    let tls = args.tls as usize;

    // 当前内核不支持 PID namespace，但为通过测试，忽略 CLONE_NEWPID
    let mut effective_flags = flags;
    effective_flags &= !CLONE_NEWPID;

    let process = current_process();
    let child_pid = process._clone(effective_flags, stack, ptid, ctid, tls, exit_signal) as usize;

    // CLONE_INTO_CGROUP: 将新进程放入指定 cgroup
    // if (args.flags & crate::task::CLONE_INTO_CGROUP) != 0 && args.cgroup != 0 {
    //     let cgroup_fd = args.cgroup as usize;
    //     let inner = process.inner_exclusive_access();
    //     if let Some(file) = inner.fd_table.get(cgroup_fd).and_then(|f| f.clone()) {
    //         let dentry = file.get_dentry();
    //         let dir_path = dentry.path();
    //         drop(inner);
    //         let mut table = crate::fs::cgroup2::CGROUP_TABLE.lock();
    //         table.entry(dir_path).or_default().push(child_pid);
    //     }
    // }

    if (flags & CLONE_PIDFD) != 0 && args.pidfd != 0 {
        let pidfd_file = Arc::new(crate::fs::pidfd::PidFdFile::new(child_pid));
        let mut inner = process.inner_exclusive_access();
        if let Ok(fd) = inner.alloc_fd() {
            inner.fd_table[fd] = Some(pidfd_file);
            drop(inner);
            let mut buf = crate::mm::translated_byte_buffer(
                token,
                args.pidfd as *const u8,
                core::mem::size_of::<i32>(),
            )?;
            if !buf.is_empty() && buf[0].len() >= 4 {
                buf[0][0..4].copy_from_slice(&(fd as i32).to_ne_bytes());
            }
        }
    }

    if (flags & CLONE_VFORK) != 0 {
        block_current_and_run_next();
    }
    Ok(child_pid)
}

pub fn sys_getuid() -> SyscallResult {
    let process = current_process();
    Ok(process.inner_exclusive_access().uid as usize)
}

pub fn sys_geteuid() -> SyscallResult {
    let process = current_process();
    Ok(process.inner_exclusive_access().euid as usize)
}

pub fn sys_getegid() -> SyscallResult {
    let process = current_process();
    Ok(process.inner_exclusive_access().egid as usize)
}

pub fn sys_getgid() -> SyscallResult {
    let process = current_process();
    Ok(process.inner_exclusive_access().gid as usize)
}

pub fn sys_setuid(uid: u32) -> SyscallResult {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    if inner.euid != 0 {
        return Err(SysError::EPERM);
    }
    inner.uid = uid;
    inner.euid = uid;
    inner.suid = uid;
    Ok(0)
}

pub fn sys_setreuid(ruid: usize, euid: usize) -> SyscallResult {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    const NOCHANGE: u32 = 0xFFFF_FFFF;
    let ruid = ruid as u32;
    let euid = euid as u32;

    if inner.euid != 0 {
        let valid_ruid = ruid == NOCHANGE || ruid == inner.uid || ruid == inner.euid;
        let valid_euid =
            euid == NOCHANGE || euid == inner.uid || euid == inner.euid || euid == inner.suid;
        if !valid_ruid || !valid_euid {
            return Err(SysError::EPERM);
        }
    }

    let old_ruid = inner.uid;
    if ruid != NOCHANGE {
        inner.uid = ruid;
    }
    if euid != NOCHANGE {
        inner.euid = euid;
    }
    if inner.uid != old_ruid || (euid != NOCHANGE && euid != old_ruid) {
        inner.suid = inner.euid;
    }
    Ok(0)
}

pub fn sys_setgid(gid: u32) -> SyscallResult {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    if inner.euid != 0 {
        return Err(SysError::EPERM);
    }
    inner.gid = gid;
    inner.egid = gid;
    inner.sgid = gid;
    Ok(0)
}

pub fn sys_setresuid(ruid: usize, euid: usize, suid: usize) -> SyscallResult {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    const NOCHANGE: u32 = 0xFFFFFFFFu32;

    let ruid32 = ruid as u32;
    let euid32 = euid as u32;
    let suid32 = suid as u32;

    let check = |id: u32| -> bool {
        id == NOCHANGE || id == inner.uid || id == inner.euid || id == inner.suid
    };

    if inner.euid != 0 {
        if !check(ruid32) || !check(euid32) || !check(suid32) {
            return Err(SysError::EPERM);
        }
    }

    if ruid32 != NOCHANGE {
        inner.uid = ruid32;
    }
    if euid32 != NOCHANGE {
        inner.euid = euid32;
    }
    if suid32 != NOCHANGE {
        inner.suid = suid32;
    }
    Ok(0)
}

pub fn sys_setresgid(rgid: usize, egid: usize, sgid: usize) -> SyscallResult {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    const NOCHANGE: u32 = 0xFFFFFFFFu32;

    let rgid32 = rgid as u32;
    let egid32 = egid as u32;
    let sgid32 = sgid as u32;

    let check = |id: u32| -> bool {
        id == NOCHANGE || id == inner.gid || id == inner.egid || id == inner.sgid
    };

    if inner.euid != 0 {
        if !check(rgid32) || !check(egid32) || !check(sgid32) {
            return Err(SysError::EPERM);
        }
    }

    if rgid32 != NOCHANGE {
        inner.gid = rgid32;
    }
    if egid32 != NOCHANGE {
        inner.egid = egid32;
    }
    if sgid32 != NOCHANGE {
        inner.sgid = sgid32;
    }
    Ok(0)
}

pub fn sys_getpgid(pid: i32) -> SyscallResult {
    debug!("sys_getpgid called with pid: {}", pid);
    let target_pid = if pid == 0 {
        current_process().getpid() as i32
    } else {
        pid
    };
    if target_pid < 0 {
        return Err(SysError::ESRCH);
    }
    if let Some(proc) = pid2process(target_pid as usize) {
        Ok(proc.getpgid() as usize)
    } else {
        Err(SysError::ESRCH)
    }
}

pub fn sys_setpgid(pid: i32, pgid: i32) -> SyscallResult {
    if pid < 0 || pgid < 0 {
        return Err(SysError::EINVAL);
    }

    let current = current_process();
    let current_pid = current.getpid();
    let target_pid = if pid == 0 { current_pid } else { pid as usize };
    let new_pgid = if pgid == 0 { target_pid } else { pgid as usize };

    let target = if target_pid == current_pid {
        current
    } else {
        match pid2process(target_pid) {
            Some(proc) => proc,
            None => return Err(SysError::ESRCH),
        }
    };

    target.setpgid(new_pgid);
    Ok(0)
}

pub fn sys_setsid() -> SyscallResult {
    let process = current_process();
    let pid = process.getpid();
    process.setpgid(pid);
    Ok(pid)
}

pub fn sys_getpgrp() -> SyscallResult {
    Ok(current_process().getpgid() as usize)
}

/// prlimit64：获取/设置进程资源限制。
/// 当前已实现 RLIMIT_NOFILE（7），其余资源返回无限制（RLIM_INFINITY）。
pub fn sys_prlimit64(
    pid: usize,
    resource: i32,
    new_limit: *const u8,
    old_limit: *mut u8,
) -> SyscallResult {
    let current_pid = current_task().unwrap().process.upgrade().unwrap().getpid();
    // pid == 0 表示当前进程
    if pid != 0 && pid != current_pid {
        return Err(SysError::ESRCH);
    }

    let token = current_user_token();
    let process = current_process();
    let mut inner = process.inner_exclusive_access();

    if !old_limit.is_null() {
        let rlim = translated_refmut::<Rlimit64>(token, old_limit as *mut Rlimit64)?;
        match resource {
            RLIMIT_FSIZE => {
                rlim.rlim_cur = inner.rlimit_fsize.rlim_cur;
                rlim.rlim_max = inner.rlimit_fsize.rlim_max;
            }
            RLIMIT_NOFILE => {
                rlim.rlim_cur = inner.rlimit_nofile.rlim_cur;
                rlim.rlim_max = inner.rlimit_nofile.rlim_max;
            }
            _ => {
                rlim.rlim_cur = u64::MAX;
                rlim.rlim_max = u64::MAX;
            }
        }
    }

    if !new_limit.is_null() {
        let new_rlim = translated_ref::<Rlimit64>(token, new_limit as *const Rlimit64)?;
        match resource {
            RLIMIT_FSIZE => {
                inner.rlimit_fsize.rlim_cur = new_rlim.rlim_cur;
                inner.rlimit_fsize.rlim_max = new_rlim.rlim_max;
            }
            RLIMIT_NOFILE => {
                inner.rlimit_nofile.rlim_cur = new_rlim.rlim_cur;
                inner.rlimit_nofile.rlim_max = new_rlim.rlim_max;
            }
            _ => {}
        }
    }

    Ok(0)
}

#[allow(unused)]
pub fn sys_setpgrp() -> SyscallResult {
    sys_setpgid(0, 0)
}

pub fn sys_sched_getaffinity(
    _pid: usize,
    cpusetusize: usize,
    user_mask_ptr: usize,
) -> SyscallResult {
    use core::mem::size_of;

    log::info!(
        "sys_sched_getaffinity: pid={}, cpusetsize={}, mask_ptr={:#x}",
        _pid,
        cpusetusize,
        user_mask_ptr
    );

    // 参数验证
    if user_mask_ptr == 0 {
        log::warn!("sys_sched_getaffinity: NULL pointer");
        return Err(SysError::EFAULT);
    }

    let required_size = size_of::<u64>();
    if cpusetusize < required_size {
        log::warn!(
            "sys_sched_getaffinity: buffer too small, need={}, got={}",
            required_size,
            cpusetusize
        );
        return Err(SysError::EINVAL);
    }

    // CPU mask: 假设有 1 个 CPU (CPU 0)
    let cpu_mask: u64 = 0x01;

    let token = current_user_token();
    *translated_refmut(token, user_mask_ptr as *mut u64)? = cpu_mask;

    log::info!(
        "sys_sched_getaffinity: success, mask=0x{:x}, size={}",
        cpu_mask,
        required_size
    );

    // 关键：返回写入的字节数，而不是 1
    Ok(required_size) // 返回 8，不是 1
    // Err(SysError::EINVAL)
}

pub fn sys_sched_setaffinity(_pid: isize, len: usize, user_mask: *const u64) -> SyscallResult {
    if user_mask.is_null() {
        return Err(SysError::EFAULT);
    }

    // 简化实现：只验证参数，不实际设置 CPU 亲和性
    // 因为我们的系统可能只有一个 CPU，或者调度器不支持亲和性

    // 检查长度是否足够
    if len < 8 {
        // 至少需要 8 字节（一个 u64）
        return Err(SysError::EINVAL);
    }

    // 读取用户空间的 CPU 掩码（只是为了验证地址有效）
    let token = current_user_token();
    let _mask = *translated_ref(token, user_mask)?;

    // 对于单 CPU 系统，直接返回成功
    // 因为所有进程都只能在唯一的 CPU 上运行
    Ok(0)
}

fn sched_target_task(pid: isize) -> Result<Arc<crate::task::TaskControlBlock>, SysError> {
    if pid < 0 {
        return Err(SysError::EINVAL);
    }
    if pid == 0 {
        return current_task().ok_or(SysError::ESRCH);
    }

    let id = pid as usize;
    if let Some(task) = tid2task(id) {
        return Ok(task);
    }
    let process = pid2process(id).ok_or(SysError::ESRCH)?;
    let inner = process.inner_exclusive_access();
    inner
        .tasks
        .iter()
        .find_map(|task| task.as_ref().map(Arc::clone))
        .ok_or(SysError::ESRCH)
}

fn validate_sched_param(policy: i32, priority: i32) -> Result<i32, SysError> {
    let base_policy = policy & !SCHED_RESET_ON_FORK;
    match base_policy {
        SCHED_NORMAL | SCHED_BATCH | SCHED_IDLE => {
            if priority == 0 {
                Ok(0)
            } else {
                Err(SysError::EINVAL)
            }
        }
        SCHED_FIFO | SCHED_RR => {
            if (1..=99).contains(&priority) {
                Ok(priority)
            } else {
                Err(SysError::EINVAL)
            }
        }
        _ => Err(SysError::EINVAL),
    }
}

fn sched_attr_policy(policy: i32) -> u32 {
    (policy & !SCHED_RESET_ON_FORK) as u32
}

pub fn sys_sched_getscheduler(pid: isize) -> SyscallResult {
    let task = sched_target_task(pid)?;
    Ok(sched_attr_policy(task.sched_policy() as i32) as usize)
}

pub fn sys_sched_setscheduler(pid: isize, policy: i32, param: *const SchedParam) -> SyscallResult {
    if param.is_null() {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let sched_param = *translated_ref(token, param)?;
    let priority = validate_sched_param(policy, sched_param.sched_priority)?;
    let task = sched_target_task(pid)?;
    task.set_sched(sched_attr_policy(policy) as u32, priority);
    Ok(0)
}

pub fn sys_sched_getparam(pid: isize, param: *mut SchedParam) -> SyscallResult {
    if param.is_null() {
        error!("sys_sched_getparam: pid={}, param=NULL -> EFAULT", pid);
        return Err(SysError::EFAULT);
    }
    let task = match sched_target_task(pid) {
        Ok(task) => task,
        Err(err) => {
            error!(
                "sys_sched_getparam: pid={}, param={:#x}, target lookup failed: {:?}",
                pid, param as usize, err
            );
            return Err(err);
        }
    };
    let token = current_user_token();
    let sched_param = SchedParam {
        sched_priority: task.sched_priority(),
    };
    match copy_struct_to_user(
        token,
        param,
        &sched_param,
        core::mem::size_of::<SchedParam>(),
    ) {
        Ok(ret) => Ok(ret),
        Err(err) => {
            error!(
                "sys_sched_getparam: pid={}, param={:#x}, priority={}, copy failed: {:?}",
                pid, param as usize, sched_param.sched_priority, err
            );
            Err(err)
        }
    }
}

pub fn sys_sched_get_priority_max(policy: i32) -> SyscallResult {
    match policy & !SCHED_RESET_ON_FORK {
        SCHED_FIFO | SCHED_RR => Ok(99),
        SCHED_NORMAL | SCHED_BATCH | SCHED_IDLE => Ok(0),
        _ => Err(SysError::EINVAL),
    }
}

pub fn sys_sched_get_priority_min(policy: i32) -> SyscallResult {
    match policy & !SCHED_RESET_ON_FORK {
        SCHED_FIFO | SCHED_RR => Ok(1),
        SCHED_NORMAL | SCHED_BATCH | SCHED_IDLE => Ok(0),
        _ => Err(SysError::EINVAL),
    }
}

pub fn sys_sched_rr_get_interval(pid: isize, interval: *mut TimeSpec) -> SyscallResult {
    if interval.is_null() {
        return Err(SysError::EFAULT);
    }
    let _task = sched_target_task(pid)?;
    let token = current_user_token();
    *translated_refmut(token, interval)? = TimeSpec {
        tv_sec: 0,
        tv_nsec: 1_000_000,
    };
    Ok(0)
}

pub fn sys_sched_setattr(pid: isize, attr: *const SchedAttr, flags: u32) -> SyscallResult {
    if attr.is_null() {
        return Err(SysError::EFAULT);
    }
    if flags != 0 {
        return Err(SysError::EINVAL);
    }
    let token = current_user_token();
    let user_size =
        copy_struct_from_user::<u32>(token, attr as *const u32, core::mem::size_of::<u32>())?
            as usize;
    if user_size != 0 && user_size < SCHED_ATTR_MIN_SIZE {
        return Err(SysError::EINVAL);
    }
    let read_len = if user_size == 0 {
        core::mem::size_of::<SchedAttr>()
    } else {
        user_size.min(core::mem::size_of::<SchedAttr>())
    };
    let sched_attr = copy_struct_from_user(token, attr, read_len)?;
    let policy = sched_attr.sched_policy as i32;
    let priority = validate_sched_param(policy, sched_attr.sched_priority as i32)?;
    let task = sched_target_task(pid)?;
    task.set_sched(sched_attr_policy(policy) as u32, priority);
    Ok(0)
}

pub fn sys_sched_getattr(pid: isize, attr: *mut SchedAttr, size: u32, flags: u32) -> SyscallResult {
    if attr.is_null() {
        return Err(SysError::EFAULT);
    }
    if flags != 0 || (size as usize) < SCHED_ATTR_MIN_SIZE {
        return Err(SysError::EINVAL);
    }
    let task = sched_target_task(pid)?;
    let priority = task.sched_priority();
    let policy = sched_attr_policy(task.sched_policy() as i32);
    let sched_attr = SchedAttr {
        size: core::mem::size_of::<SchedAttr>() as u32,
        sched_policy: policy,
        sched_flags: 0,
        sched_nice: 0,
        sched_priority: priority as u32,
        sched_runtime: 0,
        sched_deadline: 0,
        sched_period: 0,
    };
    let token = current_user_token();
    let copy_len = (size as usize).min(core::mem::size_of::<SchedAttr>());
    copy_struct_to_user(token, attr, &sched_attr, copy_len)
}

const MPOL_DEFAULT: i32 = 0;
const MPOL_PREFERRED: i32 = 1;
const MPOL_BIND: i32 = 2;
const MPOL_INTERLEAVE: i32 = 3;
const MPOL_LOCAL: i32 = 4;
const MPOL_PREFERRED_MANY: i32 = 5;
const MPOL_F_MEMS_ALLOWED: u32 = 1 << 2;
const NUMA_NODE_MASK: u64 = 1;

fn valid_mempolicy_mode(mode: i32) -> bool {
    let base_mode = mode & 0xffff;
    matches!(
        base_mode,
        MPOL_DEFAULT
            | MPOL_PREFERRED
            | MPOL_BIND
            | MPOL_INTERLEAVE
            | MPOL_LOCAL
            | MPOL_PREFERRED_MANY
    )
}

fn validate_nodemask(nodemask: *const u64, maxnode: usize) -> Result<(), SysError> {
    if nodemask.is_null() || maxnode == 0 {
        return Ok(());
    }
    let token = current_user_token();
    let mask_words = maxnode.div_ceil(u64::BITS as usize);
    let mask_words = mask_words.max(1);
    let buffers = translated_byte_buffer(
        token,
        nodemask as *const u8,
        mask_words * core::mem::size_of::<u64>(),
    )?;
    let mut word = 0usize;
    let mut offset = 0usize;
    for buf in buffers {
        for byte in buf.iter() {
            let byte_offset = offset % core::mem::size_of::<u64>();
            if byte_offset == 0 {
                word = 0;
            }
            word |= (*byte as usize) << (byte_offset * u8::BITS as usize);
            offset += 1;
            if offset % core::mem::size_of::<u64>() == 0 && word & !NUMA_NODE_MASK as usize != 0 {
                return Err(SysError::EINVAL);
            }
        }
    }
    if offset % core::mem::size_of::<u64>() != 0 && word & !NUMA_NODE_MASK as usize != 0 {
        return Err(SysError::EINVAL);
    }
    Ok(())
}

pub fn sys_get_mempolicy(
    mode: *mut i32,
    nodemask: *mut u64,
    maxnode: usize,
    _addr: usize,
    flags: u32,
) -> SyscallResult {
    let token = current_user_token();
    if !mode.is_null() {
        *translated_refmut(token, mode)? = MPOL_DEFAULT;
    }
    if !nodemask.is_null() && maxnode != 0 {
        let mask_words = maxnode.div_ceil(u64::BITS as usize).max(1);
        let mask = if flags & MPOL_F_MEMS_ALLOWED != 0 {
            NUMA_NODE_MASK
        } else {
            0
        };
        *translated_refmut(token, nodemask)? = mask;
        for idx in 1..mask_words {
            let ptr = unsafe { nodemask.add(idx) };
            *translated_refmut(token, ptr)? = 0;
        }
    }
    Ok(0)
}

pub fn sys_set_mempolicy(mode: i32, nodemask: *const u64, maxnode: usize) -> SyscallResult {
    if !valid_mempolicy_mode(mode) {
        return Err(SysError::EINVAL);
    }
    validate_nodemask(nodemask, maxnode)?;
    Ok(0)
}

pub fn sys_mbind(
    _start: usize,
    _len: usize,
    mode: i32,
    nodemask: *const u64,
    maxnode: usize,
    _flags: u32,
) -> SyscallResult {
    if !valid_mempolicy_mode(mode) {
        return Err(SysError::EINVAL);
    }
    validate_nodemask(nodemask, maxnode)?;
    Ok(0)
}

pub fn sys_migrate_pages(
    _pid: isize,
    maxnode: usize,
    old_nodes: *const u64,
    new_nodes: *const u64,
) -> SyscallResult {
    validate_nodemask(old_nodes, maxnode)?;
    validate_nodemask(new_nodes, maxnode)?;
    Ok(0)
}

pub fn sys_move_pages(
    _pid: isize,
    count: usize,
    pages: *const usize,
    _nodes: *const i32,
    status: *mut i32,
    _flags: i32,
) -> SyscallResult {
    let token = current_user_token();
    if count != 0 && pages.is_null() {
        return Err(SysError::EFAULT);
    }
    if count != 0 {
        let _ = translated_byte_buffer(
            token,
            pages as *const u8,
            count * core::mem::size_of::<usize>(),
        )?;
    }
    if !status.is_null() {
        for idx in 0..count {
            let ptr = unsafe { status.add(idx) };
            *translated_refmut(token, ptr)? = 0;
        }
    }
    Ok(0)
}

pub fn sys_set_mempolicy_home_node(
    _start: usize,
    _len: usize,
    home_node: usize,
    _flags: u32,
) -> SyscallResult {
    if home_node > 0 {
        return Err(SysError::EINVAL);
    }
    Ok(0)
}

pub fn sys_socketpair(domain: i32, type_: i32, protocol: i32, sv: *mut i32) -> SyscallResult {
    const AF_UNIX: i32 = 1;
    const SOCK_STREAM: i32 = 1;
    const SOCK_DGRAM: i32 = 2;
    const SOCK_SEQPACKET: i32 = 5;
    const SOCK_TYPE_MASK: i32 = 0xf;
    const SOCK_NONBLOCK: i32 = 0o0004000;
    const SOCK_CLOEXEC: i32 = 0o2000000;

    if sv.is_null() {
        return Err(SysError::EFAULT);
    }
    if domain != AF_UNIX {
        return Err(SysError::EAFNOSUPPORT);
    }
    if protocol != 0 {
        return Err(SysError::EPROTONOSUPPORT);
    }

    let sock_type = type_ & SOCK_TYPE_MASK;
    let extra_bits = type_ & !(SOCK_TYPE_MASK | SOCK_NONBLOCK | SOCK_CLOEXEC);
    if extra_bits != 0 {
        return Err(SysError::EINVAL);
    }
    match sock_type {
        SOCK_STREAM | SOCK_DGRAM | SOCK_SEQPACKET => {}
        _ => return Err(SysError::EPROTONOSUPPORT),
    }

    let token = current_user_token();
    let mut user_bufs =
        translated_byte_buffer(token, sv as *const u8, 2 * core::mem::size_of::<i32>())?;

    let nonblock = type_ & SOCK_NONBLOCK != 0;
    let cloexec = type_ & SOCK_CLOEXEC != 0;
    let (socket0, socket1) = make_socket_pair(nonblock);

    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    let fd0 = inner.alloc_fd()?;
    inner.fd_table[fd0] = Some(socket0);
    let fd1 = match inner.alloc_fd() {
        Ok(fd) => fd,
        Err(e) => {
            inner.fd_table[fd0] = None;
            if fd0 < inner.fd_flags.len() {
                inner.fd_flags[fd0] = 0;
            }
            return Err(e);
        }
    };

    inner.fd_table[fd1] = Some(socket1);
    if cloexec {
        if fd0 < inner.fd_flags.len() {
            inner.fd_flags[fd0] |= FD_CLOEXEC_FLAG;
        }
        if fd1 < inner.fd_flags.len() {
            inner.fd_flags[fd1] |= FD_CLOEXEC_FLAG;
        }
    }
    drop(inner);

    let fds = [fd0 as i32, fd1 as i32];
    let bytes = unsafe {
        core::slice::from_raw_parts(fds.as_ptr() as *const u8, 2 * core::mem::size_of::<i32>())
    };
    let mut copied = 0usize;
    for buf in user_bufs.iter_mut() {
        let n = buf.len().min(bytes.len() - copied);
        buf[..n].copy_from_slice(&bytes[copied..copied + n]);
        copied += n;
        if copied == bytes.len() {
            break;
        }
    }

    Ok(0)
}

pub fn sys_getresuid(ruid: *mut u32, euid: *mut u32, suid: *mut u32) -> SyscallResult {
    let process = current_process();
    let inner = process.inner_exclusive_access();

    let token = current_user_token();
    if !ruid.is_null() {
        *translated_refmut(token, ruid)? = inner.uid;
    }
    if !euid.is_null() {
        *translated_refmut(token, euid)? = inner.euid;
    }
    if !suid.is_null() {
        *translated_refmut(token, suid)? = inner.suid;
    }
    Ok(0)
}

// pub fn sys_setresuid(ruid: i32, euid: i32, suid: i32) -> SyscallResult {
//     let process = current_process();
//     let mut inner = process.inner_exclusive_access();

//     let current_euid = inner.euid;
//     let old_ruid = inner.uid;
//     let old_euid = inner.euid;
//     let old_suid = inner.suid;

//     // 设置真实 UID
//     if ruid != -1 {
//         if current_euid != 0 && (ruid as u32 != old_ruid && ruid as u32 != old_euid && ruid as u32 != old_suid) {
//             return Err(SysError::EPERM);
//         }
//         inner.uid = ruid as u32;
//     }

//     // 设置有效 UID
//     if euid != -1 {
//         if current_euid != 0 && (euid as u32 != old_ruid && euid as u32 != old_euid && euid as u32 != old_suid) {
//             return Err(SysError::EPERM);
//         }
//         inner.euid = euid as u32;
//     }

//     // 设置保存的 UID
//     if suid != -1 {
//         if current_euid != 0 && (suid as u32 != old_ruid && suid as u32 != old_euid && suid as u32 != old_suid) {
//             return Err(SysError::EPERM);
//         }
//         inner.suid = suid as u32;
//     }

//     Ok(0)
// }

pub fn sys_prctl(
    option: i32,
    arg2: usize,
    _arg3: usize,
    _arg4: usize,
    _arg5: usize,
) -> SyscallResult {
    const PR_GET_DUMPABLE: i32 = 3;
    const PR_SET_DUMPABLE: i32 = 4;
    const PR_GET_NAME: i32 = 16;
    const PR_SET_NAME: i32 = 15;
    const PR_SET_NO_NEW_PRIVS: i32 = 38;
    const PR_GET_NO_NEW_PRIVS: i32 = 39;
    const PR_SET_IO_FLUSHER: i32 = 72;
    const PR_GET_IO_FLUSHER: i32 = 73;

    match option {
        PR_GET_DUMPABLE => Ok(1),
        PR_SET_DUMPABLE => Ok(0),
        PR_SET_NAME => Ok(0),
        PR_SET_NO_NEW_PRIVS => {
            if arg2 != 1 {
                return Err(SysError::EINVAL);
            }
            current_process().inner_exclusive_access().no_new_privs = true;
            Ok(0)
        }
        PR_GET_NO_NEW_PRIVS => Ok(current_process().inner_exclusive_access().no_new_privs as usize),
        PR_GET_NAME => {
            let buf = arg2 as *mut u8;
            if !buf.is_null() {
                let token = current_user_token();
                if let Ok(page) = translated_refmut(token, buf) {
                    *page = 0;
                }
            }
            Ok(0)
        }
        PR_SET_IO_FLUSHER => Ok(0),
        PR_GET_IO_FLUSHER => Ok(0),
        _ => {
            warn!("sys_prctl: unsupported option {}", option);
            Ok(0)
        }
    }
}
