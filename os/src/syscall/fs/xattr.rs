use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::notify::{NotifyTarget, notify_attrib, notify_target_for_file_if_needed};
use crate::fs::vfs::file::File;
use crate::fs::vfs::inode::{Inode, XATTR_LIST_MAX, XATTR_NAME_MAX, XATTR_SIZE_MAX};
use crate::fs::vfs::path::{resolve_path, resolve_path_nofollow_last};
use crate::mm::{copy_to_user, translated_byte_buffer, translated_str};
use crate::task::{current_process, current_user_token};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

fn read_xattr_name(token: usize, name: *const u8) -> SysResult<String> {
    if name.is_null() {
        return Err(SysError::EFAULT);
    }
    let mut name_str = String::new();
    let mut va = name as usize;
    for _ in 0..=XATTR_NAME_MAX {
        let mut byte = [0u8; 1];
        let parts = translated_byte_buffer(token, va as *const u8, 1)?;
        byte[0] = parts[0][0];
        if byte[0] == 0 {
            if name_str.is_empty() {
                return Err(SysError::ERANGE);
            }
            return Ok(name_str);
        }
        name_str.push(byte[0] as char);
        va += 1;
    }
    if name_str.is_empty() {
        return Err(SysError::ERANGE);
    }
    Err(SysError::ERANGE)
}

fn read_xattr_value(token: usize, value: *const u8, size: usize) -> SysResult<Vec<u8>> {
    if value.is_null() && size > 0 {
        return Err(SysError::EFAULT);
    }
    if size > XATTR_SIZE_MAX {
        return Err(SysError::E2BIG);
    }
    if size == 0 {
        return Ok(Vec::new());
    }
    Ok(translated_byte_buffer(token, value, size)?
        .into_iter()
        .flat_map(|b| b.iter().copied())
        .collect::<Vec<u8>>())
}

fn xattr_output_buffer(buf: *mut u8, size: usize, limit: usize) -> SysResult<Vec<u8>> {
    if buf.is_null() && size > 0 {
        return Err(SysError::EFAULT);
    }
    let alloc_size = size.min(limit);
    if buf.is_null() || alloc_size == 0 {
        Ok(Vec::new())
    } else {
        Ok(vec![0u8; alloc_size])
    }
}

fn fd_to_file(fd: usize) -> SysResult<Arc<dyn File + Send + Sync>> {
    let process = current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
        return Err(SysError::EBADF);
    }
    Ok(inner.fd_table[fd].as_ref().unwrap().clone())
}

fn fd_to_inode(fd: usize) -> SysResult<Arc<dyn Inode>> {
    let file = fd_to_file(fd)?;
    file.get_inode().ok_or(SysError::EBADF)
}

fn path_to_dentry(
    path: *const u8,
    follow_last_link: bool,
) -> SysResult<Arc<dyn crate::fs::vfs::Dentry>> {
    const PATH_MAX: usize = 4096;

    let token = current_user_token();
    let raw_path = translated_str(token, path)?;
    if raw_path.is_empty() {
        return Err(SysError::ENOENT);
    }
    if raw_path.len() > PATH_MAX {
        return Err(SysError::ENAMETOOLONG);
    }
    let cwd = current_process().inner_exclusive_access().cwd.clone();
    if follow_last_link {
        resolve_path(cwd, &raw_path)
    } else {
        resolve_path_nofollow_last(cwd, &raw_path)
    }
}

pub fn sys_fsetxattr(
    fd: usize,
    name: *const u8,
    value: *const u8,
    size: usize,
    flags: i32,
) -> SyscallResult {
    let token = current_user_token();
    let name_str = read_xattr_name(token, name)?;
    let value_buf = read_xattr_value(token, value, size)?;
    let file = fd_to_file(fd)?;
    let notify_target = notify_target_for_file_if_needed(&file);
    let inode = file.get_inode().ok_or(SysError::EBADF)?;
    inode.setxattr(&name_str, &value_buf, flags)?;
    if let Some(target) = notify_target.as_ref() {
        notify_attrib(target);
    }
    Ok(0)
}

pub fn sys_fgetxattr(fd: usize, name: *const u8, buf: *mut u8, size: usize) -> SyscallResult {
    let token = current_user_token();
    let name_str = read_xattr_name(token, name)?;
    let mut dst = xattr_output_buffer(buf, size, XATTR_SIZE_MAX)?;
    let file = fd_to_file(fd)?;
    if file.is_socket() {
        return Err(SysError::ENODATA);
    }
    let inode = file.get_inode().ok_or(SysError::EBADF)?;
    let ret = inode.getxattr(&name_str, &mut dst)?;
    if !buf.is_null() && size > 0 {
        copy_to_user(token, buf, &dst[..ret.min(dst.len())])?;
    }
    Ok(ret)
}

pub fn sys_flistxattr(fd: usize, buf: *mut u8, size: usize) -> SyscallResult {
    let token = current_user_token();
    let mut dst = xattr_output_buffer(buf, size, XATTR_LIST_MAX)?;
    let inode = fd_to_inode(fd)?;
    let ret = inode.listxattr(&mut dst)?;
    if !buf.is_null() && size > 0 {
        copy_to_user(token, buf, &dst[..ret.min(dst.len())])?;
    }
    Ok(ret)
}

pub fn sys_fremovexattr(fd: usize, name: *const u8) -> SyscallResult {
    let token = current_user_token();
    let name_str = read_xattr_name(token, name)?;
    let file = fd_to_file(fd)?;
    let notify_target = notify_target_for_file_if_needed(&file);
    let inode = file.get_inode().ok_or(SysError::EBADF)?;
    inode.removexattr(&name_str)?;
    if let Some(target) = notify_target.as_ref() {
        notify_attrib(target);
    }
    Ok(0)
}

pub fn sys_setxattr(
    path: *const u8,
    name: *const u8,
    value: *const u8,
    size: usize,
    flags: i32,
) -> SyscallResult {
    let token = current_user_token();
    let name_str = read_xattr_name(token, name)?;
    let value_buf = read_xattr_value(token, value, size)?;
    let dentry = path_to_dentry(path, true)?;
    let inode = dentry.get_inode().ok_or(SysError::ENOENT)?;
    inode.setxattr(&name_str, &value_buf, flags)?;
    notify_attrib(&NotifyTarget::new(dentry));
    Ok(0)
}

pub fn sys_lsetxattr(
    path: *const u8,
    name: *const u8,
    value: *const u8,
    size: usize,
    flags: i32,
) -> SyscallResult {
    let token = current_user_token();
    let name_str = read_xattr_name(token, name)?;
    let value_buf = read_xattr_value(token, value, size)?;
    let dentry = path_to_dentry(path, false)?;
    let inode = dentry.get_inode().ok_or(SysError::ENOENT)?;
    inode.setxattr(&name_str, &value_buf, flags)?;
    notify_attrib(&NotifyTarget::new(dentry));
    Ok(0)
}

pub fn sys_getxattr(path: *const u8, name: *const u8, buf: *mut u8, size: usize) -> SyscallResult {
    let token = current_user_token();
    let name_str = read_xattr_name(token, name)?;
    let mut dst = xattr_output_buffer(buf, size, XATTR_SIZE_MAX)?;
    let dentry = path_to_dentry(path, true)?;
    let inode = dentry.get_inode().ok_or(SysError::ENOENT)?;
    let ret = inode.getxattr(&name_str, &mut dst)?;
    if !buf.is_null() && size > 0 {
        copy_to_user(token, buf, &dst[..ret.min(dst.len())])?;
    }
    Ok(ret)
}

pub fn sys_lgetxattr(path: *const u8, name: *const u8, buf: *mut u8, size: usize) -> SyscallResult {
    let token = current_user_token();
    let name_str = read_xattr_name(token, name)?;
    let mut dst = xattr_output_buffer(buf, size, XATTR_SIZE_MAX)?;
    let dentry = path_to_dentry(path, false)?;
    let inode = dentry.get_inode().ok_or(SysError::ENOENT)?;
    let ret = inode.getxattr(&name_str, &mut dst)?;
    if !buf.is_null() && size > 0 {
        copy_to_user(token, buf, &dst[..ret.min(dst.len())])?;
    }
    Ok(ret)
}

pub fn sys_listxattr(path: *const u8, buf: *mut u8, size: usize) -> SyscallResult {
    let token = current_user_token();
    let mut dst = xattr_output_buffer(buf, size, XATTR_LIST_MAX)?;
    let dentry = path_to_dentry(path, true)?;
    let inode = dentry.get_inode().ok_or(SysError::ENOENT)?;
    let ret = inode.listxattr(&mut dst)?;
    if !buf.is_null() && size > 0 {
        copy_to_user(token, buf, &dst[..ret.min(dst.len())])?;
    }
    Ok(ret)
}

pub fn sys_llistxattr(path: *const u8, buf: *mut u8, size: usize) -> SyscallResult {
    let token = current_user_token();
    let mut dst = xattr_output_buffer(buf, size, XATTR_LIST_MAX)?;
    let dentry = path_to_dentry(path, false)?;
    let inode = dentry.get_inode().ok_or(SysError::ENOENT)?;
    let ret = inode.listxattr(&mut dst)?;
    if !buf.is_null() && size > 0 {
        copy_to_user(token, buf, &dst[..ret.min(dst.len())])?;
    }
    Ok(ret)
}

pub fn sys_removexattr(path: *const u8, name: *const u8) -> SyscallResult {
    let token = current_user_token();
    let name_str = read_xattr_name(token, name)?;
    let dentry = path_to_dentry(path, true)?;
    let inode = dentry.get_inode().ok_or(SysError::ENOENT)?;
    inode.removexattr(&name_str)?;
    notify_attrib(&NotifyTarget::new(dentry));
    Ok(0)
}

pub fn sys_lremovexattr(path: *const u8, name: *const u8) -> SyscallResult {
    let token = current_user_token();
    let name_str = read_xattr_name(token, name)?;
    let dentry = path_to_dentry(path, false)?;
    let inode = dentry.get_inode().ok_or(SysError::ENOENT)?;
    inode.removexattr(&name_str)?;
    notify_attrib(&NotifyTarget::new(dentry));
    Ok(0)
}
