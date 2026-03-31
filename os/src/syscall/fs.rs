use crate::fs::{open_file};
use crate::mm::{UserBuffer, translated_byte_buffer, translated_refmut, translated_str};
use crate::sync::mutex::*;
use crate::syscall::process;
use crate::task::{current_process, current_user_token};
use alloc::sync::Arc;
use lazy_static::*;
use riscv::register::sstatus::FS;
use crate::fs::lwext4::file::find_dentry;
use crate::fs::vfs::path::{get_start_dentry, split_parent_and_name};
use lwext4_rust::InodeTypes;
use crate::fs::vfs::inode::InodeType;
use crate::fs::vfs::dcache::GLOBAL_DCACHE;
use alloc::ffi::CString;
use alloc::format;
use crate::mm::copy_to_user;
use alloc::vec::Vec;
use crate::fs::vfs::OpenFlags;
use crate::fs::lwext4::ext4::file::ExtFS;
use crate::fs::vfs::kstat::Kstat;
use crate::fs::vfs::mount::{vfs_mount,vfs_umount2};
// lazy_static! {
//     pub static ref FS_LOCK: MutexSpin = MutexSpin::new();
// }

use crate::fs::vfs::path::{resolve_path};
use log::*;

///
#[allow(unused)]
pub fn sys_getcwd(buf: *const u8, len: usize) -> isize {
    let process = current_process();
    let token = current_user_token();
    let path = process.inner_exclusive_access().cwd.clone().path();
    let cstr = CString::new(path).expect("fail to convert CString");
    copy_to_user(token, buf, cstr.as_bytes_with_nul())as isize

}


///create a directory with the path, the path is the name of the directory
/// the mode was not used in this function
pub fn sys_mkdirat(dirfd:isize, path: *const u8,_mode:u32)->isize{
    let token = current_user_token();
    let path = translated_str(token, path);
    let start_dentry = match get_start_dentry(dirfd, &path) {
        Ok(dentry) => dentry,
        Err(errno) => return errno, 
    };    
    let (parent_path, dir_name) = split_parent_and_name(&path);
    
    let parent = if parent_path == "."|| parent_path == "/"{
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
        },
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
pub fn sys_linkat(olddirfd: isize, oldpath: *const u8, newdirfd: isize, newpath: *const u8, _flags: u32) -> isize {
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
    info!("[sys_mount] source: {}, mount_point: {}, fstype: {}", source_path, mount_path, fstype_path);
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
        if target_dentry.get_inode().unwrap().get_types()==(InodeTypes::EXT4_DE_REG_FILE) { 
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
        match file.get_stat(&mut stat){
            
            Ok(_) => {
                let stat_bytes = unsafe {
                    core::slice::from_raw_parts(
                        &stat as *const _ as *const u8,
                        core::mem::size_of::<Kstat>()
                    )
                };
                copy_to_user(token, stat_buf, stat_bytes) as isize
            },
            Err(_) => return -1,
        }
    } else {
        -1
    }
}

///
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

///
pub fn sys_openat(dirfd: isize, path: *const u8, flags: u32) -> isize {
    let process = current_process();
    let token = current_user_token();
    let raw_path = translated_str(token, path);
    let start_dentry = match get_start_dentry(dirfd, &raw_path) {
        Ok(dentry) => dentry,
        Err(errno) => return errno, 
    };
    if let Some(file) = open_file(
        start_dentry,
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
    inner.fd_table[fd].take();
    0
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
        buffer.extend_from_slice(&ino.to_ne_bytes());                       //ino
        buffer.extend_from_slice(&(offset as u64).to_ne_bytes());           // off
        buffer.extend_from_slice(&(reclen as u16).to_ne_bytes());           // d_reclen
        buffer.push(dt_type);                                               // d_type
        buffer.extend_from_slice(name_bytes);                               // d_name
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

