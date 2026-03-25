use crate::fs::{OpenFlags, open_file};
use crate::mm::{UserBuffer, translated_byte_buffer, translated_refmut, translated_str};
use crate::sync::mutex::*;
use crate::syscall::process;
use crate::task::{current_process, current_user_token};
use alloc::sync::Arc;
use lazy_static::*;
use riscv::register::sstatus::FS;
use crate::fs::lwext4::file::find_dentry;
use lwext4_rust::InodeTypes;
// lazy_static! {
//     pub static ref FS_LOCK: MutexSpin = MutexSpin::new();
// }

use crate::fs::vfs::cwd::{resolve_path};
use log::*;

///create a directory with the path, the path is the name of the directory
/// the mode was not used in this function
pub fn sys_mkdirat(path: *const u8)->isize{
    let process = current_process();
    let token = current_user_token();
    let path = translated_str(token, path);
    let cwd = process.inner_exclusive_access().cwd.clone();

    let dentry = resolve_path(cwd, &path);
    if dentry == None{
        return -1;
    }
    let parent_dentry = resolve_path(cwd, ".");

    0
}
pub fn sys_chdir(path: *const u8) -> isize {
    let process = current_process();
    let token = current_user_token();
    let path = translated_str(token, path);
    let mut inner = process.inner_exclusive_access();
    let cwd = inner.cwd.clone(); 
    if let Some(target_dentry) = resolve_path(cwd, &path) {
        if target_dentry.get_inode().unwrap().get_types()==(InodeTypes::EXT4_DE_REG_FILE) { 
            return -1;
        }
        inner.cwd = target_dentry;
        0 
    } else {
        -1 
    }
}

pub fn sys_write(fd: usize, buf: *const u8, len: usize) -> isize {
    info!("sys_write called for fd: {}", fd);
    let token = current_user_token();
    let process = current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if let Some(file) = &inner.fd_table[fd] {
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
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if let Some(file) = &inner.fd_table[fd] {
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
    if let Some(file) = open_file(
        cwd,
        raw_path.as_str(),
        OpenFlags::from_bits(flags).unwrap(),
    ) {
        let mut inner = process.inner_exclusive_access();
        let fd = inner.alloc_fd();
        inner.fd_table[fd] = Some(file);
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
    0
}

