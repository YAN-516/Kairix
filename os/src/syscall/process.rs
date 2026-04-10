use super::TimeVal;
use crate::alloc::string::ToString;
use crate::config::PAGE_SIZE;
use crate::fs::open_file;
use crate::fs::vfs::OpenFlags;
use crate::mm::heap::HeapExt;
use crate::mm::{PageTable, PhysAddr};
use crate::mm::{VMSpace, translated_ref, translated_refmut, translated_str};
use crate::task::{
    block_current_and_run_next, current_process, current_task, current_user_token,
    exit_current_and_run_next, pid2process, suspend_current_and_run_next,
};
#[cfg(target_arch = "riscv64")]
use crate::timer::get_time_us;
use crate::trap::_set_sum_bit;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use log::*;
use polyhal::timer::*;
pub use polyhal::utils::addr::*;
pub fn sys_exit(exit_code: i32) -> ! {
    exit_current_and_run_next(exit_code);
    panic!("Unreachable in sys_exit!");
}

pub fn sys_yield() -> isize {
    //println!("enter yield!");
    suspend_current_and_run_next();
    0
}

pub fn sys_get_time(_ts: *mut TimeVal, _tz: usize) -> isize {
    _set_sum_bit();
    let _ns = current_time().as_nanos() as usize;
    unsafe {
        *(_ts) = TimeVal {
            sec: _ns / 1_000_000,
            usec: _ns % 1_000_000,
        };
    }
    0
}

pub fn sys_getpid() -> isize {
    current_task().unwrap().process.upgrade().unwrap().getpid() as isize
}

pub fn sys_getppid() -> isize {
    let process = current_process();
    let inner = process.inner_exclusive_access();
    let parent = inner.parent.as_ref().and_then(|weak| weak.upgrade());

    if let Some(parent) = parent {
        parent.getpid() as isize
    } else {
        -1
    }
}

pub fn sys_fork() -> isize {
    let current_process = current_process();
    let new_process = current_process.fork();
    let new_pid = new_process.getpid();
    // // modify trap context of new_task, because it returns immediately after switching
    // let new_process_inner = new_process.inner_exclusive_access();
    // let task = new_process_inner.tasks[0].as_ref().unwrap();
    // let trap_cx = task.inner_exclusive_access().get_trap_cx();
    // // we do not have to move to next instruction since we have done it before
    // // for child process, fork returns 0
    // trap_cx.x[10] = 0;
    warn!(
        "fork a new process with pid {}, parent pid = {}",
        new_pid,
        current_process.getpid()
    );
    new_pid as isize
}

// pub fn sys_exec(path: *const u8) -> isize {
//     let token = current_user_token();
//     let path = translated_str(token, path);
//     if let Some(app_inode) = open_file(path.as_str(), OpenFlags::RDONLY) {
//         let all_data = app_inode.read_all();
//         let task = current_task().unwrap();
//         task.exec(all_data.as_slice());
//         0
//     } else {
//         -1
//     }
// }
#[allow(unused)]
pub fn sys_execve(path: usize, argv: usize, envp: usize) -> isize {
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
    let app_file = match open_file(cwd.clone(), path_str.as_str(), OpenFlags::RDONLY) {
        Some(f) => f,
        None => return -1,
    };
    info!("Executing program: {}", path_str);
    let all_data = app_file.read_all();
    let mut ret = process.execve(all_data.as_slice(), args_vec.clone(), envs_vec.clone());

    // 如果它是纯文本脚本,重新使用busybox加载
    if ret == -8 {
        info!(
            "Not an ELF! Fallback to busybox sh to run script: {}",
            path_str
        );
        if let Some(busybox_file) = open_file(cwd, "busybox", OpenFlags::RDONLY) {
            // 重新构造参数：["busybox", "sh", "原本的脚本路径", 原本的参数1, 原本的参数2...]
            let mut new_args = vec!["busybox".to_string(), "sh".to_string(), path_str];
            if args_vec.len() > 1 {
                new_args.extend_from_slice(&args_vec[1..]);
            }
            let busybox_data = busybox_file.read_all();
            ret = process.execve(busybox_data.as_slice(), new_args, envs_vec);
        } else {
            warn!("Fallback failed: busybox not found!");
        }
    }
    ret
}

pub fn sys_brk(ptr: *const i32) -> isize {
    let process = current_process();
    let vm_set = &mut process.inner_exclusive_access().vm_set;
    if ptr as usize == 0 {
        return vm_set.heap_end_va().0 as isize;
    }
    let current_end_va = vm_set.heap_end_va();
    if current_end_va.0 == ptr as usize {
        return 0;
    }
    if current_end_va.0 < ptr as usize {
        vm_set.append_to(VirtAddr::from(ptr as usize));
    } else {
        vm_set.shrink_to(VirtAddr::from(ptr as usize));
    }
    0
}

/// If there is not a child process whose pid is same as given, return -1.
/// Else if there is a child process but it is still running, return -2.
pub fn sys_waitpid(pid: isize, exit_code_ptr: *mut i32) -> isize {
    _set_sum_bit();
    let process = current_process();
    // find a child process

    let mut inner = process.inner_exclusive_access();

    if !inner
        .children
        .iter()
        .any(|p| pid == -1 || pid as usize == p.getpid())
    {
        return -1;
        // ---- release current PCB
    }
    let pair = inner.children.iter().enumerate().find(|(_, p)| {
        // ++++ temporarily access child PCB exclusively
        p.inner_exclusive_access().is_zombie && (pid == -1 || pid as usize == p.getpid())
        // ++++ release child PCB
    });
    if let Some((idx, _)) = pair {
        let exit_code = {
            let child = &inner.children[idx];
            let child_inner = child.inner_exclusive_access();
            child_inner.exit_code
        };
        let child = inner.children.remove(idx);
        let found_pid = child.getpid();
        // confirm that child will be deallocated after being removed from children list
        //assert_eq!(Arc::strong_count(&child), 1);
        // ++++ release child PCB
        drop(inner);
        drop(process);
        unsafe {
            *exit_code_ptr = ((exit_code as i32) & 0xFF) << 8;
        }

        found_pid as isize
    } else {
        -2
    }
    // ---- release current PCB automatically
}
#[allow(unused)]
pub fn sys_clone(flags: u32, stack: usize /* , arg: usize*/) -> isize {
    let process = current_process();
    process._clone(flags, stack)
}

pub fn sys_getuid() -> isize {
    // 单用户系统，所有进程都是 Root
    0
}

pub fn sys_rt_sigprocmask(_how: usize, _set: usize, _oldset: usize, _sigsetsize: usize) -> isize {
    0
}

pub fn sys_rt_sigaction(_signum: usize, _act: usize, _oldact: usize, _sigsetsize: usize) -> isize {
    // 这里暂时没实现信号处理，所以直接返回成功
    0
}

pub fn sys_setpgid(_pid: i32, _pgid: i32) -> isize {
    0
}
