use crate::fs::{OpenFlags, open_file};
use crate::mm::{UserBuffer, translated_byte_buffer, translated_refmut, translated_str};
use crate::sync::mutex::*;
use crate::syscall::process;
use crate::task::{current_process, current_user_token};
use crate::trap::_set_sum_bit;
use alloc::sync::Arc;
use lazy_static::*;
use log::{error, warn};
use riscv::register::sstatus::FS;

// lazy_static! {
//     pub static ref FS_LOCK: MutexSpin = MutexSpin::new();
// }

use crate::fs::vfs::cwd::build_absolute_path;
use log::*;
pub fn sys_write(fd: usize, buf: *const u8, len: usize) -> isize {
    info!("sys_write called for fd: {}", fd);
    let token = current_user_token();
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
        //FS_LOCK.lock();
        let ret = file.write(UserBuffer::new(translated_byte_buffer(token, buf, len))) as isize;
        //FS_LOCK.unlock();
        ret
    } else {
        -1
    }
}

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
        file.read(UserBuffer::new(translated_byte_buffer(token, buf, len))) as isize
    } else {
        -1
    }
}

pub fn sys_open(path: *const u8, flags: u32) -> isize {
    let process = current_process();
    let token = current_user_token();
    let raw_path = translated_str(token, path);
    let cwd = process.inner_exclusive_access().cwd.clone();
    let absolute_path = build_absolute_path(&cwd, &raw_path);

    if let Some(inode) = open_file(
        &absolute_path.as_str(),
        OpenFlags::from_bits(flags).unwrap(),
    ) {
        let mut inner = process.inner_exclusive_access();
        let fd = inner.alloc_fd();
        inner.fd_table[fd] = Some(inode);
        fd as isize
    } else {
        error!("sys_open failed, returning -1");
        -1
    }
}

pub fn sys_close(fd: usize) -> isize {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if inner.fd_table[fd].is_none() {
        return -1;
    }
    inner.fd_table[fd].take();
    warn!("close {}", fd);
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
