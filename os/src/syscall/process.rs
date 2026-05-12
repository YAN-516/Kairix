use super::TimeVal;
use crate::alloc::string::ToString;
// use crate::config::PAGE_SIZE;
use crate::error::{SysError, SyscallResult};
use crate::fs::vfs::OpenFlags;
use crate::fs::vfs::file::open_file;
use crate::fs::vfs::inode::InodeMode;
use crate::mm::heap::HeapExt;
use crate::mm::{PageTable, PhysAddr};
use crate::mm::{VMSpace, translated_ref, translated_refmut, translated_str};
use crate::remove_from_pid2process;
use crate::task::{
    RLIMIT_NOFILE, Rlimit64, block_current_and_run_next, current_process, current_task,
    current_user_token, exit_current_and_run_next, pid2process, suspend_current_and_run_next,
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
use polyhal::consts::PAGE_SIZE;
use polyhal::timer::*;
pub use polyhal::utils::addr::*;
use polyhal_trap::trapframe::TrapFrameArgs;
#[allow(unused)]
pub const SCHED_NORMAL: i32 = 0;  // 普通分时调度
#[allow(unused)]
pub const SCHED_FIFO: i32 = 1;    // 先进先出实时调度
#[allow(unused)]
pub const SCHED_RR: i32 = 2;      // 轮转实时调度
#[allow(unused)]
pub const SCHED_BATCH: i32 = 3;   // 批处理调度
#[allow(unused)]
pub const SCHED_IDLE: i32 = 5;    // 空闲调度
#[repr(C)]
#[derive(Copy, Clone)]
#[allow(unused)]
pub struct SchedParam {
    pub sched_priority: i32,
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
    _set_sum_bit();
    let _ns = current_time().as_nanos() as u128;
    unsafe {
        *(_ts) = TimeVal {
            sec: (_ns / 1_000_000_000) as i64,
            usec: ((_ns / 1_000) % 1_000_000) as i64,
        };
    }
    Ok(0)
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
    let token = current_user_token();
    let path_str = translated_str(token, path as *const u8);
    let mut args_vec: Vec<String> = Vec::new();
    if argv != 0 {
        let mut argv_ptr = argv as *const usize;
        loop {
            let str_ptr = *translated_ref(token, argv_ptr);
            if str_ptr == 0 {
                break;
            }
            args_vec.push(translated_str(token, str_ptr as *const u8));
            argv_ptr = unsafe { argv_ptr.add(1) };
        }
    }
    let mut envs_vec: Vec<String> = Vec::new();
    if envp != 0 {
        let mut envp_ptr = envp as *const usize;
        loop {
            let str_ptr = *translated_ref(token, envp_ptr);
            if str_ptr == 0 {
                break;
            }
            envs_vec.push(translated_str(token, str_ptr as *const u8));
            envp_ptr = unsafe { envp_ptr.add(1) };
        }
    }
    let task = current_task().unwrap();
    let process = task.process.upgrade().unwrap();
    let cwd = process.inner_exclusive_access().cwd.clone();
    error!("[sys_execve] path={} cwd_name={}", path_str, cwd.name());
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
    error!("Executing program: {}", path_str);
    let all_data = app_file.read_all();
    let mut ret = process.execve(all_data.as_slice(), args_vec.clone(), envs_vec.clone());
    let is_elf = all_data.len() >= 4
        && all_data[0] == 0x7f
        && all_data[1] == 0x45
        && all_data[2] == 0x4c
        && all_data[3] == 0x46;

    // 如果它是纯文本脚本,重新使用busybox加载
    if ret == -8 && !is_elf {
        error!(
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
            let busybox_data = busybox_file.read_all();
            ret = process.execve(busybox_data.as_slice(), new_args, envs_vec);
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
        Ok(ret as usize)
    }
}

pub fn sys_brk(ptr: *const i32) -> SyscallResult {
    // Linux 语义：brk 系统调用返回“当前程序 break 地址”，
    // glibc 封装会据此判断是否成功（ret < requested 视为失败）。
    let process = current_process();
    let vm_set = &mut process.inner_exclusive_access().vm_set;
    if ptr as usize == 0 {
        return Ok(vm_set.heap_end_va().0);
    }
    let current_end_va = vm_set.heap_end_va();
    if current_end_va.0 == ptr as usize {
        return Ok(current_end_va.0);
    }
    if current_end_va.0 < ptr as usize {
        vm_set.append_to(VirtAddr::from(ptr as usize));
    } else {
        vm_set.shrink_to(VirtAddr::from(ptr as usize));
    }
    Ok(vm_set.heap_end_va().0)
}

/// If there is not a child process whose pid is same as given, return -1.
/// Else if there is a child process but it is still running:
///   - with WNOHANG: return 0
///   - without WNOHANG: block until a child exits
pub fn sys_waitpid(pid: isize, exit_code_ptr: *mut i32, options: i32) -> SyscallResult {
    _set_sum_bit();
    let process = current_process();
    loop {
        let mut inner = process.inner_exclusive_access();

        if !inner
            .children
            .iter()
            .any(|p| pid == -1 || pid as usize == p.getpid())
        {
            return Err(SysError::ECHILD);
        }

        if let Some((idx, _)) = inner.children.iter().enumerate().find(|(_, p)| {
            let p_inner = p.inner_exclusive_access();
            p_inner.is_zombie
                && p_inner.alive_thread_count == 0
                && (pid == -1 || pid as usize == p.getpid())
        }) {
            let exit_code = {
                let child = &inner.children[idx];
                let child_inner = child.inner_exclusive_access();
                child_inner.exit_code
            };
            let child = inner.children.remove(idx);
            let found_pid = child.getpid();
            remove_from_pid2process(found_pid);
            drop(inner);
            let parent_pid = process.getpid();
            drop(process);
            if !exit_code_ptr.is_null() {
                unsafe {
                    *exit_code_ptr = (exit_code & 0xFF) << 8;
                }
            }
            error!(
                "[DEBUG waitpid] parent_pid={} found zombie child pid={} exit_code={}",
                parent_pid, found_pid, exit_code
            );
            return Ok(found_pid);
        }

        if options & 0x00000001 != 0 {
            return Ok(0);
        }

        drop(inner);
        block_current_and_run_next();
        // 如果当前进程自身被 kill，才返回 EINTR；
        // 单纯的信号中断（如 SIGUSR1）不退出，继续等待子进程。
        if crate::task::current_process()
            .inner_exclusive_access()
            .is_zombie
        {
            return Err(SysError::EINTR);
        }
    }
}

#[allow(unused)]
pub fn sys_clone(flags: u32, stack: usize, ptid: usize, ctid: usize, tls: usize) -> SyscallResult {
    let process = current_process();
    Ok(process._clone(flags, stack, ptid, ctid, tls) as usize)
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
    Ok(0)
}



pub fn sys_getpgid(pid: i32) -> SyscallResult {
    error!("sys_getpgid called with pid: {}", pid);
    let target_pid = if pid == 0 {
        current_process().getpid() as i32
    } else {
        pid
    };
    if target_pid < 0 {
        return Ok(0);
    }
    if let Some(proc) = pid2process(target_pid as usize) {
        Ok(proc.getpgid() as usize)
    } else {
        Ok(0)
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
        let rlim = translated_refmut::<Rlimit64>(token, old_limit as *mut Rlimit64);
        match resource {
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
        let new_rlim = translated_ref::<Rlimit64>(token, new_limit as *const Rlimit64);
        match resource {
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

pub fn sys_sched_getaffinity(_pid: usize, cpusetusize: usize, user_mask_ptr: usize) -> SyscallResult {
    use core::mem::size_of;
    
    log::info!("sys_sched_getaffinity: pid={}, cpusetsize={}, mask_ptr={:#x}", 
               _pid, cpusetusize, user_mask_ptr);
    
    // 参数验证
    if user_mask_ptr == 0 {
        log::warn!("sys_sched_getaffinity: NULL pointer");
        return Err(SysError::EFAULT);
    }
    
    let required_size = size_of::<u64>();
    if cpusetusize < required_size {
        log::warn!("sys_sched_getaffinity: buffer too small, need={}, got={}", 
                   required_size, cpusetusize);
        return Err(SysError::EINVAL);
    }
    
    // CPU mask: 假设有 1 个 CPU (CPU 0)
    let cpu_mask: u64 = 0x01;
    
    // 安全地写入用户空间
    let _token = current_user_token();
    let ptr = user_mask_ptr as *mut u64;
    
    // 使用已有的 copy_to_user 或安全写入函数
    unsafe {
        // 检查地址是否在用户空间范围内
        if ptr.is_null() || (ptr as usize) < 0x1000 {
            log::warn!("sys_sched_getaffinity: invalid address {:#p}", ptr);
            return Err(SysError::EFAULT);
        }
        
        // 写入 mask
        match core::ptr::write_volatile(ptr, cpu_mask) {
            () => {}
        }
    }
    
    log::info!("sys_sched_getaffinity: success, mask=0x{:x}, size={}", 
               cpu_mask, required_size);
    
    // 关键：返回写入的字节数，而不是 1
    Ok(required_size)  // 返回 8，不是 1
    // Err(SysError::EINVAL)
}

pub fn sys_sched_setaffinity(_pid: isize, len: usize, user_mask: *const u64) -> SyscallResult {
    if user_mask.is_null() {
        return Err(SysError::EFAULT);
    }
    
    // 简化实现：只验证参数，不实际设置 CPU 亲和性
    // 因为我们的系统可能只有一个 CPU，或者调度器不支持亲和性
    
    // 检查长度是否足够
    if len < 8 {  // 至少需要 8 字节（一个 u64）
        return Err(SysError::EINVAL);
    }
    
    // 读取用户空间的 CPU 掩码（只是为了验证地址有效）
    let token = current_user_token();
    let _mask = *translated_ref(token, user_mask);
    
    // 对于单 CPU 系统，直接返回成功
    // 因为所有进程都只能在唯一的 CPU 上运行
    Ok(0)
}

pub fn sys_sched_getscheduler(_pid: isize) -> SyscallResult {
    // 返回当前任务的调度策略
    Ok(SCHED_FIFO as usize)
}

pub fn sys_sched_setscheduler(_pid: isize, policy: i32, _param: *const SchedParam) -> SyscallResult {
    // 简化实现：只支持 SCHED_FIFO
    if policy != SCHED_FIFO {
        return Err(SysError::EINVAL);
    }
    Ok(0)
}

pub fn sys_sched_getparam(_pid: isize, param: *mut SchedParam) -> SyscallResult {
    // For simplicity, all tasks use SCHED_NORMAL with priority 0
    let sched_param = SchedParam {
        sched_priority: 0,
    };
    
    // 直接将结构体写入用户空间指针
    unsafe {
        core::ptr::write(param, sched_param);
    }
    
    Ok(0)
}

pub fn sys_socketpair(_domain: i32, _type_: i32, _protocol: i32, _sv: *mut i32) -> SyscallResult {
    // use crate::fs::tempfs::dentry::TempDentry;
    // use crate::fs::tempfs::file::TempFile;
    
    // if sv.is_null() {
    //     return Err(SysError::EFAULT);
    // }
    // // 
    // // Allocate two file descriptors
    // let process = current_process();
    // let mut inner = process.inner_exclusive_access();
    
    // let fd1 = inner.alloc_fd()?;
    // let fd2 = inner.alloc_fd()?;
    
    // // Create dummy socket files
    // let dentry = TempDentry::new("socket", None);  // 添加第二个参数 None
    // let file = Arc::new(TempFile::new(dentry));
    
    // inner.fd_table[fd1] = Some(file.clone());
    // inner.fd_table[fd2] = Some(file);
    
    // // Write the file descriptors to user space
    // let token = current_user_token();
    // unsafe {
    //     *translated_refmut(token, sv) = fd1 as i32;
    //     *translated_refmut(token, sv.add(1)) = fd2 as i32;
    // }
    
    // Ok(0)
    Err(SysError::EINVAL)
}
