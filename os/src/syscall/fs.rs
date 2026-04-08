//use crate::fs::lwext4::file::find_dentry;
use crate::fs::lwext4::ext4::file::ExtFS;
use crate::fs::open_file;
use crate::fs::vfs::OpenFlags;
use crate::fs::vfs::dcache::GLOBAL_DCACHE;
use crate::fs::vfs::file::File;
use crate::fs::vfs::inode::InodeType;
use crate::fs::vfs::kstat::Kstat;
use crate::fs::vfs::mount::{vfs_mount, vfs_umount2};
use crate::fs::vfs::path::{get_start_dentry, split_parent_and_name};
use crate::mm::PageTable;
use crate::mm::copy_to_user;
use crate::mm::{UserBuffer, translated_byte_buffer, translated_refmut, translated_str};
use crate::sync::mutex::*;
use crate::syscall::process;
use crate::syscall::thread::sys_gettid;
use crate::task::{current_process, current_user_token};
use crate::trap::_set_sum_bit;
use alloc::ffi::CString;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use lazy_static::*;
use log::{error, warn};
use lwext4_rust::InodeTypes;
use polyhal::consts::VIRT_ADDR_START;

use crate::mm::VirtAddr;
use crate::task::current_task;
#[cfg(target_arch = "riscv64")]
use riscv::register::sstatus::FS;
// lazy_static! {
//     pub static ref FS_LOCK: MutexSpin = MutexSpin::new();
// }

use crate::fs::vfs::path::resolve_path;
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
    match parent.create(dir_name.as_str(), InodeType::Dir) {
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

///
pub fn sys_umount2(target: *const u8, flags: u32) -> isize {
    let token = current_user_token();
    let target_path = translated_str(token, target);
    match vfs_umount2(&target_path, flags) {
        Ok(_) => 0,
        Err(errno) => errno,
    }
}

///
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
    let mount_dentry = resolve_path(cwd, &mount_path).unwrap();
    match vfs_mount(&source_path, &mount_path, mount_dentry, &fstype_path) {
        Ok(_) => 0,
        Err(errno) => errno,
    }
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
    info!("sys_write called for fd: {}", fd);
    let token = current_user_token();

    //截获 BusyBox 要往屏幕上打印的报错遗言！
    if fd == 1 || fd == 2 {
        let buffers = crate::mm::translated_byte_buffer(token, buf, len);
        info!("[Shell Output fd {}]: ", fd);
        for buffer in &buffers {
            if let Ok(s) = core::str::from_utf8(buffer) {
                info!("{}", s);
            } else {
                info!("<Invalid UTF-8>");
            }
        }
        info!("");
    }

    let process = current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if let Some(file) = &inner.fd_table[fd] {
        warn!("write {} {}", fd, len);
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
    error!("sys_fstat called with fd: {}", fd);
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

    // 标准1：AT_EMPTY_PATH (0x1000)
    // 如果路径为空，且 flags 包含了 AT_EMPTY_PATH，说明它想直接查 dirfd 这个句柄的属性
    const AT_EMPTY_PATH: u32 = 0x1000;
    if raw_path.is_empty() {
        if (flags & AT_EMPTY_PATH) != 0 {
            return sys_fstat(dirfd as usize, stat_buf);
        } else {
            return -1; // POSIX 规定：路径为空且没有 AT_EMPTY_PATH，必须返回 ENOENT (-1)
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
        // ==========================================
        // 🌟 致命细节同步：必须在这里也把底层真实大小同步上来！
        // 否则 ls -l 看到的所有文件大小全都会变成 0！
        // ==========================================
        let file_inner = file.get_fileinner();
        let real_size = file_inner.ext4file.file_desc.fsize as usize;
        file_inner.dentry.get_inode().unwrap().set_size(real_size);
        drop(file_inner);

        // 构造状态并填充
        let mut stat = Kstat::new();
        match file.get_stat(&mut stat) {
            Ok(_) => {
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
        -1 // ENOENT 找不到文件
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
        warn!("read {} {}", fd, len);
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

///
pub fn sys_openat(dirfd: isize, path: *const u8, flags: u32) -> isize {
    let process = current_process();
    let token = current_user_token();
    let mut raw_path = translated_str(token, path);
    let mut safe_flags = OpenFlags::from_bits_truncate(flags & 0xFFF); // 只保留低 12 位，去掉 O_CLOEXEC 等不相关的标志
    if raw_path == "/dev/null" {
        raw_path = String::from("/null"); // 变成根目录下的普通文件
        safe_flags |= OpenFlags::O_CREAT;
    }

    let start_dentry = match get_start_dentry(dirfd, &raw_path) {
        Ok(dentry) => dentry,
        Err(errno) => return errno,
    };

    if let Some(file) = open_file(start_dentry, raw_path.as_str(), safe_flags) {
        let mut inner = process.inner_exclusive_access();
        let file_inner = file.get_fileinner();
        let real_size = file_inner.ext4file.file_desc.fsize as usize; // 获取底层真实大小
        file_inner.dentry.get_inode().unwrap().set_size(real_size); // 赋值给你的 shadow size
        drop(file_inner);
        let fd = inner.alloc_fd();
        inner.fd_table[fd] = Some(file);
        info!("sys_openat return with fd: {}", fd);
        fd as isize
    } else {
        error!("sys_open failed, returning -1");
        -1
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
///
pub fn sys_getdents64(fd: usize, buf: *mut u8, len: usize) -> isize {
    let process = current_process();
    let token = current_user_token();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
        return -9;
    }
    let file = inner.fd_table[fd].as_ref().unwrap().clone();
    let entries = file.ls();
    let mut offset = file.get_offset() as usize;
    if offset >= entries.len() {
        return 0;
    }
    let mut buffer: Vec<u8> = Vec::new();
    for (name, ino, dt_type) in entries.into_iter().skip(offset) {
        let name_bytes = name.as_bytes();
        let mut reclen = 8 + 8 + 2 + 1 + name_bytes.len() + 1;
        reclen = (reclen + 7) & !7;

        if buffer.len() + reclen > len {
            break;
        }
        offset += 1;
        buffer.extend_from_slice(&ino.to_ne_bytes()); //ino
        buffer.extend_from_slice(&(offset as u64).to_ne_bytes()); // off
        buffer.extend_from_slice(&(reclen as u16).to_ne_bytes()); // d_reclen
        buffer.push(dt_type); // d_type
        buffer.extend_from_slice(name_bytes); // d_name
        buffer.push(0);
        let current_len = 8 + 8 + 2 + 1 + name_bytes.len() + 1;
        buffer.extend(alloc::vec![0u8; reclen - current_len]);
    }
    file.set_offset(offset as usize);
    let copy_size = buffer.len();
    if copy_size > 0 {
        copy_to_user(token, buf, &buffer);
    }
    copy_size as isize
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

/// 极其简易的 ioctl 桩（Stub）
/// 直接返回 0 (假装成功)
// request: 控制命令 (比如 0x5413 代表获取窗口大小)
// argp: 用户态传过来的结构体指针
pub fn sys_ioctl(fd: usize, request: usize, argp: usize) -> isize {
    info!(
        "[DEBUG] sys_ioctl fd: {}, request: {:#x}, argp: {:#x}",
        fd, request, argp
    );
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

        let base = unsafe { *((base_pa.0 + VIRT_ADDR_START) as *const usize) };
        let len = unsafe { *((len_pa.0 + VIRT_ADDR_START) as *const usize) };

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
