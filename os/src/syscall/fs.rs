
use core::fmt::Result;

use crate::fs::vfs::file::open_file;
use crate::fs::vfs::OpenFlags;
use crate::fs::vfs::dcache::GLOBAL_DCACHE;
use crate::fs::vfs::path::{get_start_dentry, split_parent_and_name};
use crate::mm::copy_to_user;
use crate::mm::{UserBuffer, translated_byte_buffer, translated_refmut, translated_str};
use crate::sync::mutex::*;
use crate::mm::PageTable;
use alloc::string::String;
use crate::fs::vfs::file::File;
use crate::fs::vfs::kstat::Kstat;
use crate::mm::{VirtAddr};
use crate::sync::mutex::*;
use crate::task::{current_process, current_task, current_user_token};
use crate::trap::_set_sum_bit;
use alloc::ffi::CString;
use alloc::format;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use crate::fs::lwext4::ext4::file::ExtFS;
use lazy_static::*;
use log::{error, warn};
use lwext4_rust::InodeTypes;
use riscv::register::sstatus::FS;
use crate::fs::vfs::inode::InodeMode;
use crate::mm::translated_ref;
use crate::fs::vfs::path::resolve_path;
use crate::config::PAGE_SIZE;
use log::*;

///
#[allow(unused)]
pub fn sys_getcwd(buf: *const u8, len: usize) -> isize {
    let process = current_process();
    let token = current_user_token();
    let path = process.inner_exclusive_access().cwd.clone().path();
    let cstr = CString::new(path).expect("fail to convert CString");
    copy_to_user(token, buf, cstr.as_bytes_with_nul()) as isize
}

///create a directory with the path, the path is the name of the directory
/// the mode was not used in this function
pub fn sys_mkdirat(dirfd: isize, path: *const u8, _mode: u32) -> isize {
    let token = current_user_token();
    let path = translated_str(token, path);
    let start_dentry = match get_start_dentry(dirfd, &path) {
        Ok(dentry) => dentry,
        Err(errno) => return errno,
    };
    let (parent_path, dir_name) = split_parent_and_name(&path);

    let parent = if parent_path == "." || parent_path == "/" {
        start_dentry
    } else {
        match resolve_path(start_dentry, &parent_path) {
            Some(dentry) => dentry,
            None => return -1,
        }
    };
    match parent.create(dir_name.as_str(), InodeMode::DIR) {
        Some(new_dir) => {
            let new_path = if parent.path() == "/" {
                format!("/{}", dir_name)
            } else {
                format!("{}/{}", parent.path(), dir_name)
            };
            GLOBAL_DCACHE.insert(new_path, new_dir);
            0
        }
        None => -1,
    }
}
///
pub fn sys_unlinkat(dirfd: isize, path: *const u8, flags: u32) -> isize {
    let token = current_user_token();
    let path = translated_str(token, path);
    let start_dentry = match get_start_dentry(dirfd, &path) {
        Ok(dentry) => dentry,
        Err(errno) => return errno,
    };
    let (parent_path, name) = split_parent_and_name(&path);

    let parent = if parent_path == "." || parent_path == "/" {
        start_dentry
    } else {
        match resolve_path(start_dentry, &parent_path) {
            Some(dentry) => dentry,
            None => return -1,
        }
    };
    if name == "." || name == ".." {
        return -22;
    }
    parent.unlink(name.as_str(), flags)
}
///
pub fn sys_linkat(
    olddirfd: isize,
    oldpath: *const u8,
    newdirfd: isize,
    newpath: *const u8,
    _flags: u32,
) -> isize {
    let token = current_user_token();
    let old_path = translated_str(token, oldpath);
    let new_path = translated_str(token, newpath);
    let old_start_dentry = match get_start_dentry(olddirfd, &old_path) {
        Ok(dentry) => dentry,
        Err(errno) => return errno,
    };
    let new_start_dentry = match get_start_dentry(newdirfd, &new_path) {
        Ok(dentry) => dentry,
        Err(errno) => return errno,
    };
    let old_dentry = match resolve_path(old_start_dentry, &old_path) {
        Some(dentry) => dentry,
        None => return -1,
    };
    let (new_parent_path, new_name) = split_parent_and_name(&new_path);
    let new_parent = if new_parent_path == "." || new_parent_path == "/" {
        new_start_dentry
    } else {
        match resolve_path(new_start_dentry, &new_parent_path) {
            Some(dentry) => dentry,
            None => return -1,
        }
    };
    if new_parent.find(new_name.as_str()).is_some() {
        return -17;
    }
    new_parent.link(new_name.as_str(), old_dentry)
}

///假装成功，直接返回 0
pub fn sys_umount2(target: *const u8, _flags: u32) -> isize {
    let token = current_user_token();
    let _target_path = translated_str(token, target);
    0
}

///假挂载，直接返回 0
pub fn sys_mount(
    source: *const u8,
    mount_path: *const u8,
    fstype: *const u8,
    _flags: usize,
    _data: *const u8,
) -> isize {
    let token = current_user_token();
    let source_path = translated_str(token, source);
    let mount_path = translated_str(token, mount_path);
    let fstype_path = translated_str(token, fstype);
    info!(
        "[sys_mount] source: {}, mount_point: {}, fstype: {}",
        source_path, mount_path, fstype_path
    );
    let cwd = current_process().inner_exclusive_access().cwd.clone();
    let _mount_dentry = resolve_path(cwd, &mount_path).unwrap();
    0
}
///
pub fn sys_chdir(path: *const u8) -> isize {
    let process = current_process();
    let token = current_user_token();
    let path = translated_str(token, path);
    let mut inner = process.inner_exclusive_access();
    let cwd = inner.cwd.clone();
    if let Some(target_dentry) = resolve_path(cwd, &path) {
        if target_dentry.get_inode().unwrap().get_types() == (InodeTypes::EXT4_DE_REG_FILE) {
            return -1;
        }
        inner.cwd = target_dentry;
        0
    } else {
        -1
    }
}
///
pub fn sys_write(fd: usize, buf: *const u8, len: usize) -> isize {
    // info!("sys_write called for fd: {}", fd);
    let token = current_user_token();
    
    if fd == 1 || fd == 2 {
        let buffers = crate::mm::translated_byte_buffer(token, buf, len);
        // info!("[Shell Output fd {}]: ", fd);
        for buffer in &buffers {
            if let Ok(_s) = core::str::from_utf8(buffer) {
                // info!("{}", s);
            } else {
                info!("<Invalid UTF-8>");
            }
        }
        // info!("");
    }

    let process = current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if let Some(file) = &inner.fd_table[fd] {
        // warn!("write {} {}", fd, len);
        if !file.writable() {
            return -1;
        }
        let file = file.clone();
        // release current task TCB manually to avoid multi-borrow
        drop(inner);

        let ret = file.write(UserBuffer::new(translated_byte_buffer(token, buf, len))) as isize;
        ret
    } else {
        -1
    }
}
///
pub fn sys_fstat(fd: usize, stat_buf: *mut u8) -> isize {
    let token = current_user_token();
    let process = current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if let Some(file) = &inner.fd_table[fd] {
        let file = file.clone();
        drop(inner);
        let mut stat = Kstat::new();
        match file.get_stat(&mut stat) {
            Ok(_) => {
                let stat_bytes = unsafe {
                    core::slice::from_raw_parts(
                        &stat as *const _ as *const u8,
                        core::mem::size_of::<Kstat>(),
                    )
                };
                copy_to_user(token, stat_buf, stat_bytes) as isize
            }
            Err(_) => return -1,
        }
    } else {
        -1
    }
}

pub fn sys_fstatat(dirfd: isize, path: *const u8, stat_buf: *mut u8, flags: u32) -> isize {
    let token = current_user_token();
    let raw_path = translated_str(token, path);
    info!("[DEBUG] sys_fstatat called: dirfd={}, path={}", dirfd, raw_path);
    // 标准1：AT_EMPTY_PATH (0x1000)
    // 如果路径为空，且 flags 包含了 AT_EMPTY_PATH，说明它想直接查 dirfd 这个句柄的属性
    const AT_EMPTY_PATH: u32 = 0x1000;
    if raw_path.is_empty() {
        if (flags & AT_EMPTY_PATH) != 0 {
            return sys_fstat(dirfd as usize, stat_buf);
        } else {
            return -2; 
        }
    }

    // 标准2：获取路径解析的起点 dentry
    let start_dentry = match get_start_dentry(dirfd, &raw_path) {
        Ok(dentry) => dentry,
        Err(errno) => return errno,
    };

    // 标准3：临时打开目标文件（不分配 fd，只为了查属性）
    // 注意：传 RDONLY 即可，哪怕是查目录属性底层也能获取到
    if let Some(file) = open_file(start_dentry, raw_path.as_str(), OpenFlags::RDONLY) {
        let dentry = file.get_dentry();
        let file_inner = file.get_fileinner();
        // let real_size = file.ext4file.lock().file_desc.fsize as usize; 
        let real_size = dentry.get_inode().unwrap().get_size() as usize;
        dentry.get_inode().unwrap().set_size(real_size); 
        drop(file_inner);
        let mut stat = Kstat::new();
        match file.get_stat(&mut stat) {
            Ok(_) => {
                info!("[DEBUG] fstatat {}: st_mode={:o} (octal), st_size={}, st_ino={}", 
                      raw_path, stat.st_mode, stat.st_size, stat.st_ino);
                let is_dir = (stat.st_mode & 0o170000) == 0o040000;
                info!("[DEBUG] is_dir={}, type_bits={:o}", is_dir, stat.st_mode & 0o170000);
                let stat_bytes = unsafe {
                    core::slice::from_raw_parts(
                        &stat as *const _ as *const u8,
                        core::mem::size_of::<Kstat>(),
                    )
                };
                crate::mm::copy_to_user(token, stat_buf, stat_bytes) as isize
            }
            Err(_) => -1,
        }
    } else {
        -2 
    }
}

///
pub fn sys_read(fd: usize, buf: *const u8, len: usize) -> isize {
    let token = current_user_token();
    let process = current_process();
    let inner = process.inner_exclusive_access();
    // if fd >= inner.fd_table.len() {
    //     return -1;
    // }
    if let Some(file) = &inner.fd_table[fd] {
        // warn!("read {} {}", fd, len);
        let file = file.clone();
        if !file.readable() {
            return -1;
        }
        // release current task TCB manually to avoid multi-borrow
        drop(inner);
        let buffers = crate::mm::translated_byte_buffer(token, buf, len);
        let user_buf = UserBuffer::new(buffers);
        let ret = file.read(user_buf) as isize;
        ret
    } else {
        -1
    }
}

// pub const F_OK: i32 = 0;
// pub const X_OK: i32 = 1;
// pub const W_OK: i32 = 2;
// pub const R_OK: i32 = 4;
///
pub fn sys_faccessat(dirfd: isize, path: *const u8, _mode: u32, _flags: u32) -> isize {
    let token = current_user_token();
    let raw_path = translated_str(token, path);
    
    const AT_EMPTY_PATH: u32 = 0x1000;
    if raw_path.is_empty() {
        if (_flags & AT_EMPTY_PATH) != 0 {
            return match get_start_dentry(dirfd, &raw_path) {
                Ok(_) => 0,
                Err(errno) => errno,
            };
        } else {
            return -2; // ENOENT: 路径为空且没传 AT_EMPTY_PATH，标准规定算找不到
        }
    }

    let start_dentry = match get_start_dentry(dirfd, &raw_path) {
        Ok(dentry) => dentry,
        Err(errno) => return errno,
    };

    if resolve_path(start_dentry, &raw_path).is_some() {
        0
    } else {
        -2 // ENOENT
    }
}

///
pub fn sys_openat(dirfd: isize, path: *const u8, flags: u32) -> isize {
    let process = current_process();
    let token = current_user_token();
    let raw_path = translated_str(token, path);
    let safe_flags = OpenFlags::from_bits_truncate(flags & 0xFFF); // 只保留低 12 位，去掉 O_CLOEXEC 等不相关的标志

    let start_dentry = match get_start_dentry(dirfd, &raw_path) {
        Ok(dentry) => dentry,
        Err(errno) => return errno,
    };

    if let Some(file) = open_file(start_dentry, raw_path.as_str(), safe_flags) {
        let mut inner = process.inner_exclusive_access();
        let file_inner = file.get_fileinner();
        // let read_size = file.ext4file.lock().file_desc.fsize as usize;
        let real_size = file_inner.dentry.get_inode().unwrap().get_size() as usize; 
        file_inner.dentry.get_inode().unwrap().set_size(real_size);
        drop(file_inner);
        let fd = inner.alloc_fd();
        inner.fd_table[fd] = Some(file);
        fd as isize
    } else {
        error!("sys_open failed for path: {}, returning -1", raw_path);
        -2
    }
}
///
pub fn sys_close(fd: usize) -> isize {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();

    if fd >= inner.fd_table.len() {
        return -1;
    }
    if inner.fd_table[fd].is_none() {
        return -1;
    }
    let file = inner.fd_table[fd].take().unwrap();
    drop(inner);
    file.flush();
    0
}

pub fn sys_dup(fd: usize) -> isize {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();

    let file_clone = if let Some(file) = inner.fd_table.get(fd) {
        file.clone()
    } else {
        return -1;
    };

    let new_fd = inner.alloc_fd();
    inner.fd_table[new_fd] = file_clone;
    new_fd as isize
}

pub fn sys_dup2(old_fd: usize, new_fd: usize) -> isize {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();

    let file_clone = if let Some(file) = inner.fd_table.get(old_fd) {
        file.clone()
    } else {
        return -1;
    };
    if new_fd >= inner.fd_table.len() {
        inner.fd_table.resize(new_fd + 1, None);
    }

    if inner.fd_table[new_fd].is_some() {
        inner.fd_table[new_fd].take();
    }

    inner.fd_table[new_fd] = file_clone;
    new_fd as isize
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct LinuxDirent64 {
    pub d_ino: u64,      // 8 bytes
    pub d_off: i64,      // 8 bytes
    pub d_reclen: u16,   // 2 bytes  
    pub d_type: u8,      // 1 byte
    // 编译器自动添加 5 bytes padding，结构体总大小 = 24 bytes
}
pub fn sys_getdents64(fd: usize, buf: *mut u8, len: usize) -> isize {
    info!("[DEBUG] sys_getdents64 called: fd={}, len={}", fd, len);
    let process = current_process();
    let token = current_user_token();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
        return -9;
    }
    let file = inner.fd_table[fd].as_ref().unwrap().clone();
    drop(inner);
    let entries = file.ls();
    info!("[DEBUG] got {} entries", entries.len());
    let skip_count = file.get_offset();
    if skip_count >= entries.len() {
        return 0;
    }
    let mut kernel_buffer: Vec<u8> = Vec::new(); 
    let mut entries_returned = 0;
    
    for (name, ino, d_type) in entries.iter().skip(skip_count) {
        let name_bytes = name.as_bytes();
        let name_len = name_bytes.len() + 1;
        // 24 (结构体) + name_len，对齐到8
        let reclen = (24 + name_len + 7) & !7;
        if kernel_buffer.len() + reclen > len {
            break;
        }
        let dirent = LinuxDirent64 {
            d_ino: *ino,
            d_off: 0,
            d_reclen: reclen as u16,
            d_type: *d_type,
        };
        let dirent_bytes = unsafe {
            core::slice::from_raw_parts(
                &dirent as *const _ as *const u8,
                24
            )
        };
        kernel_buffer.extend_from_slice(dirent_bytes);
        kernel_buffer.extend_from_slice(name_bytes);
        kernel_buffer.push(0);
        let current_len = 24 + name_bytes.len() + 1;
        let padding = reclen - current_len;
        kernel_buffer.extend(vec![0u8; padding]);
        entries_returned += 1;
    }
    if !kernel_buffer.is_empty() {
        copy_to_user(token, buf, &kernel_buffer);
    }
    file.set_offset(skip_count + entries_returned);
    info!("[DEBUG] returning {} bytes, {} entries", kernel_buffer.len(), entries_returned);
    kernel_buffer.len() as isize
}

///
pub fn sys_fsync(fd: usize) -> isize {
    let process = current_process();
    let inner = process.inner_exclusive_access();

    if fd >= inner.fd_table.len() {
        return -1;
    }
    if inner.fd_table[fd].is_none() {
        return -1;
    }
    let file = inner.fd_table[fd].as_ref().unwrap();
    file.flush();
    0
}


//对已打开的文件描述符进行各种操作
const F_DUPFD: usize = 0;
const F_GETFD: usize = 1;
const F_SETFD: usize = 2;
const F_GETFL: usize = 3;
const F_SETFL: usize = 4;
const F_DUPFD_CLOEXEC: usize = 1030;

pub fn sys_fcntl(fd: usize, cmd: usize, arg: usize) -> isize {
    let process = crate::task::current_process();
    let mut inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
        return -1;
    }

    match cmd {
        F_DUPFD | F_DUPFD_CLOEXEC => {
            let file = inner.fd_table[fd].as_ref().unwrap().clone();
            let mut new_fd = arg;
            while new_fd < inner.fd_table.len() && inner.fd_table[new_fd].is_some() {
                new_fd += 1;
            }
            if new_fd >= inner.fd_table.len() {
                inner.fd_table.resize(new_fd + 1, None);
            }
            inner.fd_table[new_fd] = Some(file);
            new_fd as isize
        }
        F_GETFD => {
            // 获取 fd 标志。通常只看有没有 FD_CLOEXEC (值为 1)
            0
        }
        F_SETFD => {
            // 设置 fd 标志 (比如设置 FD_CLOEXEC)
            0
        }
        F_GETFL => {
            // 获取文件状态标志 (O_RDONLY, O_NONBLOCK 等)
            2
        }
        F_SETFL => {
            // 设置文件状态标志 (通常是用来设置 O_NONBLOCK 非阻塞模式)
            0
        }
        _ => {
            warn!("Unsupported fcntl cmd: {}", cmd);
            -1
        }
    }
}

/// sys_writev 的核心结构体
#[repr(C)]
pub struct IoVec {
    pub base: usize,
    pub len: usize,
}
//一次性将多个不连续的内存缓冲区写入同一个文件。
pub fn sys_writev(fd: usize, iov_ptr: usize, iovcnt: usize) -> isize {
    let process = crate::task::current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
        return -1;
    }
    let file = inner.fd_table[fd].as_ref().unwrap().clone();
    drop(inner);

    let token = crate::task::current_user_token();
    let page_table = PageTable::from_token(token);
    let mut total_written = 0;

    for i in 0..iovcnt {
        let iov_addr = iov_ptr + i * core::mem::size_of::<IoVec>();
        let base_pa = page_table.translate_va(VirtAddr::from(iov_addr)).unwrap();
        let len_pa = page_table
            .translate_va(VirtAddr::from(iov_addr + 8))
            .unwrap();

        let base = unsafe { *((base_pa.0 + crate::config::KERNEL_SPACE_OFFSET) as *const usize) };
        let len = unsafe { *((len_pa.0 + crate::config::KERNEL_SPACE_OFFSET) as *const usize) };

        if len == 0 {
            continue;
        }
        let buffers = crate::mm::translated_byte_buffer(token, base as *const u8, len);
        let user_buffer = UserBuffer::new(buffers);
        let written = file.write(user_buffer);
        total_written += written;
    }
    total_written as isize
}

#[repr(C)]
pub struct PollFd {
    pub fd: i32,
    pub events: i16,
    pub revents: i16,
}
//暂时"忙轮询"
// ufds: 指向 pollfd 结构体数组的指针
// nfds: 数组的长度
pub fn sys_ppoll(ufds: usize, nfds: usize, _tmo_p: usize, _sigmask: usize) -> isize {
    let token = crate::task::current_user_token();
    let mut ready_count = 0;
    for i in 0..nfds {
        let ptr = ufds + i * core::mem::size_of::<PollFd>();
        let pollfd = crate::mm::translated_refmut::<PollFd>(token, ptr as *mut PollFd);
        // 无论在等什么事件，都认为已经发生
        pollfd.revents = pollfd.events;
        ready_count += 1;
    }
    ready_count as isize
}

const ENOTTY: isize = -25;
const EBADF:  isize = -9;
const EINVAL: isize = -22;

const TCGETS:    usize = 0x5401;
const TCSETS:    usize = 0x5402;
const TCSETSW:   usize = 0x5403;
const TCSETSF:   usize = 0x5404;
const TIOCGWINSZ:usize = 0x5413;
const TIOCSPGRP: usize = 0x5410;
const TIOCGPGRP: usize = 0x540F;

use crate::fs::devfs::tty::{TTY_STATE, Termios, WinSize};
pub fn sys_ioctl(fd: usize, request: usize, argp: usize) -> isize {
    log::info!("[DEBUG] sys_ioctl fd: {}, request: {:#x}, argp: {:#x}", fd, request, argp);

    let process = current_process();
    let token = current_user_token();
    let file = {
        let inner = process.inner_exclusive_access();
        if fd >= inner.fd_table.len() {
            return EBADF;
        }
        match inner.fd_table[fd].as_ref() {
            Some(f) => f.clone(),
            None => return EBADF,
        }
    };

    let inode = match file.get_inode() {
        Some(i) => {
            info!("sys_ioctl got inode with mode: {:?}", i.get_mode());
            i
        },
        None => return ENOTTY,
    };
    if inode.get_mode() != InodeMode::CHAR { return ENOTTY; }
    match request {
        // 获取终端属性
        TCGETS => {
            if argp == 0 { return EINVAL; }
            let user_t = translated_refmut(token, argp as *mut Termios);
            *user_t = TTY_STATE.lock().termios;
            0
        }

        // 设置终端属性
        TCSETS | TCSETSW | TCSETSF => {
            if argp == 0 { return EINVAL; }
            let user_t = translated_ref(token, argp as *const Termios);
            TTY_STATE.lock().termios = *user_t;
            0
        }

        // 读取窗口大小
        TIOCGWINSZ => {
            if argp == 0 { return EINVAL; }
            let ws = translated_refmut(token, argp as *mut WinSize);
            *ws = TTY_STATE.lock().winsize;
            0
        }

        // 获取终端的前台进程组
        TIOCGPGRP => {
            info!("sys_ioctl TIOCGPGRP called");
            if argp == 0 { return EINVAL; }
            let pgrp = translated_refmut(token, argp as *mut i32);
            info!("Current foreground pgid: {}", TTY_STATE.lock().fg_pgid);
            *pgrp = TTY_STATE.lock().fg_pgid;
            0
        }

        // 设置终端的前台进程组
        TIOCSPGRP => {
            if argp == 0 { return EINVAL; }
            let pgrp = translated_ref(token, argp as *const i32);
            TTY_STATE.lock().fg_pgid = *pgrp;
            0
        }

        _ => ENOTTY,
    }
}

/// * out_fd: 目标 fd（通常是 socket）
/// * in_fd: 源 fd（通常是磁盘文件）
/// * offset_ptr: 用户空间的 offset 指针（可空）
/// * count: 要传输的字节数
pub fn sys_sendfile(out_fd: usize, in_fd: usize, offset_ptr: usize, count: usize) -> isize {
    info!("[DEBUG] sys_sendfile: out_fd={}, in_fd={}, offset_ptr={}, count={}",
          out_fd, in_fd, offset_ptr, count);
    
    let token = current_user_token();
    let process = current_process();
    let inner = process.inner_exclusive_access();
    
    let (in_file, out_file) = match (inner.fd_table.get(in_fd), inner.fd_table.get(out_fd)) {
        (Some(Some(in_f)), Some(Some(out_f))) => (in_f.clone(), out_f.clone()),
        _ => return -9, // EBADF
    };
    drop(inner);
    if !in_file.readable() || !out_file.writable() {
        return -1;
    }
    let file_size = in_file.get_inode().map(|i| i.get_size()).unwrap_or(0);
    let (mut offset, update_fd) = if offset_ptr != 0 {
        (*translated_ref(token, offset_ptr as *const isize) as usize, false)
    } else {
        (in_file.get_offset(), true)
    };
    let end = (offset + count).min(file_size);
    let mut total = 0;
    while offset < end {
        let page_id = offset / PAGE_SIZE;
        let page_off = offset % PAGE_SIZE;
        let chunk = (end - offset).min(PAGE_SIZE - page_off);
        let Some(frame) = in_file.get_cache_frame(page_id) else { return -22 };
        let bytes = frame.ppn.get_bytes_array();
        let slice = &mut bytes[page_off..page_off + chunk];
        let written = out_file.write(UserBuffer::new(vec![slice])); 
        if written == 0 { break; }
        total += written;
        offset += written;
        if written < chunk { break; }
    }
    if offset_ptr != 0 {
        *translated_refmut(token, offset_ptr as *mut isize) = offset as isize;
    } else if update_fd {
        in_file.set_offset(offset);
    }
    info!("[DEBUG] sendfile transferred {} bytes", total);
    total as isize
}



// pub fn sys_sendfile(out_fd: usize, in_fd: usize, offset_ptr: usize, count: usize) -> isize {
//     info!("[DEBUG] sys_sendfile: out_fd={}, in_fd={}, offset_ptr={}, count={}",
//           out_fd, in_fd, offset_ptr, count);
//     let token = current_user_token();
//     let process = current_process();
//     let inner = process.inner_exclusive_access();
//     if in_fd >= inner.fd_table.len() || inner.fd_table[in_fd].is_none()
//         || out_fd >= inner.fd_table.len() || inner.fd_table[out_fd].is_none() {
//         return -9; // EBADF
//     }
//     let in_file = inner.fd_table[in_fd].as_ref().unwrap().clone();
//     let out_file = inner.fd_table[out_fd].as_ref().unwrap().clone();
//     drop(inner);
//     if !in_file.readable() || !out_file.writable() {
//         return -1;
//     }

//     let saved_offset = in_file.get_offset();
//     let mut current_offset = saved_offset;
//     if offset_ptr != 0 {
//         current_offset = *translated_ref(token, offset_ptr as *const isize) as usize;
//         in_file.set_offset(current_offset);
//     }
//     const BUF_SIZE: usize = 8192;
//     let mut buffer = [0u8; BUF_SIZE];
//     let mut total = 0usize;

//     while total < count {
//         let chunk = (count - total).min(BUF_SIZE);
//         let buf = unsafe { core::slice::from_raw_parts_mut(buffer.as_mut_ptr(), chunk) };
//         let n = in_file.read(UserBuffer::new(vec![buf]));
//         if n == 0 { break; }
//         let write_buf = unsafe { core::slice::from_raw_parts_mut(buffer.as_mut_ptr(), n) };
//         let written = out_file.write(UserBuffer::new(vec![write_buf]));
//         total += written;
//         if written < n { break; }
//     }
//     if offset_ptr != 0 {
//         in_file.set_offset(saved_offset);
//         *translated_refmut(token, offset_ptr as *mut isize) = (current_offset + total) as isize;
//     }
//     info!("[DEBUG] sendfile transferred {} bytes", total);
//     total as isize
// }

/// syscall: syslog
/// TODO: unimplement
pub fn sys_syslog(_log_type: usize, _bufp: usize, _len: usize) -> isize {
    0
}