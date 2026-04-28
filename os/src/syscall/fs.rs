use crate::error::{SysError, SyscallResult};
use core::error;
use polyhal::print;
use polyhal::timer::current_time;
// use crate::config::PAGE_SIZE;
use crate::fs::find_superblock_by_path;
use crate::fs::lwext4::ext4::file::ExtFS;
use crate::fs::vfs::OpenFlags;
use crate::fs::vfs::dcache::GLOBAL_DCACHE;
use crate::fs::vfs::file::File;
use crate::fs::vfs::file::open_file;
use crate::fs::vfs::inode::InodeMode;
use crate::fs::vfs::kstat::kstat_to_statx;
use crate::fs::vfs::kstat::{Kstat, Statfs, Statx};
use crate::fs::vfs::path::resolve_path;
use crate::fs::vfs::path::{get_start_dentry, split_parent_and_name};
use crate::mm::PageTable;
use crate::mm::VirtAddr;
use crate::mm::copy_to_user;
use crate::mm::translated_ref;
use crate::mm::{UserBuffer, translated_byte_buffer, translated_refmut, translated_str};
use crate::socket::SOCKET_MANAGER;
use crate::sync::mutex::*;
use crate::sync::mutex::*;
use crate::task::{current_process, current_task, current_user_token};
#[cfg(target_arch = "riscv64")]
use crate::timer::get_time_us;
use crate::trap::_set_sum_bit;
use alloc::ffi::CString;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use lazy_static::*;
use log::*;
use log::{error, warn};
use lwext4_rust::InodeTypes;
use polyhal::consts::*;

// use crate::mm::VirtAddr;
// use crate::task::current_task;
#[cfg(target_arch = "riscv64")]
use riscv::register::sstatus::FS;
// lazy_static! {
//     pub static ref FS_LOCK: MutexSpin = MutexSpin::new();
// }
// use riscv::register::sstatus::FS;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct LinuxStat {
    st_dev: u64,
    st_ino: u64,
    st_mode: u32,
    st_nlink: u32,
    st_uid: u32,
    st_gid: u32,
    st_rdev: u64,
    __pad1: u64,
    st_size: i64,
    st_blksize: i32,
    __pad2: i32,
    st_blocks: u64,
    st_atime_sec: i64,
    st_atime_nsec: i64,
    st_mtime_sec: i64,
    st_mtime_nsec: i64,
    st_ctime_sec: i64,
    st_ctime_nsec: i64,
    __glibc_reserved: [i32; 2],
}

const _: [(); 128] = [(); core::mem::size_of::<LinuxStat>()];

fn kstat_to_linux_stat(stat: &Kstat) -> LinuxStat {
    LinuxStat {
        st_dev: stat.st_dev,
        st_ino: stat.st_ino,
        st_mode: stat.st_mode,
        st_nlink: stat.st_nlink,
        st_uid: stat.st_uid,
        st_gid: stat.st_gid,
        st_rdev: stat.st_rdev,
        __pad1: stat.__pad,
        st_size: stat.st_size,
        st_blksize: stat.st_blksize,
        __pad2: stat.__pad2,
        st_blocks: stat.st_blocks,
        st_atime_sec: stat.st_atime_sec,
        st_atime_nsec: stat.st_atime_nsec,
        st_mtime_sec: stat.st_mtime_sec,
        st_mtime_nsec: stat.st_mtime_nsec,
        st_ctime_sec: stat.st_ctime_sec,
        st_ctime_nsec: stat.st_ctime_nsec,
        __glibc_reserved: [0; 2],
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Timespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

const UTIME_NOW: i64 = 0x3fff_ffff;
const UTIME_OMIT: i64 = 0x3fff_fffe;

///
#[allow(unused)]
pub fn sys_getcwd(buf: *const u8, len: usize) -> SyscallResult {
    if buf.is_null() {
        return Err(SysError::EFAULT);
    }
    let process = current_process();
    let token = current_user_token();
    let path = process.inner_exclusive_access().cwd.clone().path();
    let cstr = CString::new(path).expect("fail to convert CString");
    let bytes = cstr.as_bytes_with_nul();
    if len < bytes.len() {
        return Err(SysError::ERANGE);
    }
    Ok(copy_to_user(token, buf, bytes))
}

///create a directory with the path, the path is the name of the directory
/// the mode was not used in this function
pub fn sys_mkdirat(dirfd: isize, path: *const u8, _mode: u32) -> SyscallResult {
    let token = current_user_token();
    let path = translated_str(token, path);
    let start_dentry = match get_start_dentry(dirfd, &path) {
        Ok(dentry) => dentry,
        Err(e) => return Err(e),
    };
    let (parent_path, dir_name) = split_parent_and_name(&path);

    let parent = if parent_path == "." || parent_path == "/" {
        start_dentry
    } else {
        match resolve_path(start_dentry, &parent_path) {
            Ok(dentry) => dentry,
            Err(_) => return Err(SysError::ENOENT),
        }
    };
    match parent.create(dir_name.as_str(), InodeMode::DIR) {
        Ok(new_dir) => {
            let new_path = if parent.path() == "/" {
                format!("/{}", dir_name)
            } else {
                format!("{}/{}", parent.path(), dir_name)
            };
            GLOBAL_DCACHE.insert(new_path, new_dir);
            Ok(0)
        }
        Err(_) => Err(SysError::EIO),
    }
}
///
pub fn sys_unlinkat(dirfd: isize, path: *const u8, flags: u32) -> SyscallResult {
    let token = current_user_token();
    let path = translated_str(token, path);
    let start_dentry = match get_start_dentry(dirfd, &path) {
        Ok(dentry) => dentry,
        Err(e) => return Err(e),
    };
    let (parent_path, name) = split_parent_and_name(&path);

    let parent = if parent_path == "." || parent_path == "/" {
        start_dentry
    } else {
        match resolve_path(start_dentry, &parent_path) {
            Ok(dentry) => dentry,
            Err(_) => return Err(SysError::ENOENT),
        }
    };
    if name == "." || name == ".." {
        return Err(SysError::EINVAL);
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
) -> SyscallResult {
    let token = current_user_token();
    let old_path = translated_str(token, oldpath);
    let new_path = translated_str(token, newpath);
    let old_start_dentry = match get_start_dentry(olddirfd, &old_path) {
        Ok(dentry) => dentry,
        Err(e) => return Err(e),
    };
    let new_start_dentry = match get_start_dentry(newdirfd, &new_path) {
        Ok(dentry) => dentry,
        Err(e) => return Err(e),
    };
    let old_dentry = match resolve_path(old_start_dentry, &old_path) {
        Ok(dentry) => dentry,
        Err(_) => return Err(SysError::ENOENT),
    };
    let (new_parent_path, new_name) = split_parent_and_name(&new_path);
    let new_parent = if new_parent_path == "." || new_parent_path == "/" {
        new_start_dentry
    } else {
        match resolve_path(new_start_dentry, &new_parent_path) {
            Ok(dentry) => dentry,
            Err(_) => return Err(SysError::ENOENT),
        }
    };
    if new_parent.find(new_name.as_str()).is_ok() {
        return Err(SysError::EEXIST);
    }
    new_parent.link(new_name.as_str(), old_dentry)
}

pub fn sys_renameat2(
    olddirfd: isize,
    oldpath: *const u8,
    newdirfd: isize,
    newpath: *const u8,
    flags: u32,
) -> SyscallResult {
    // 先实现 Linux 常见路径：flags=0。其余标志可后续补齐。
    if flags != 0 {
        return Err(SysError::EINVAL);
    }

    let token = current_user_token();
    let old_path = translated_str(token, oldpath);
    let new_path = translated_str(token, newpath);

    let old_start_dentry = match get_start_dentry(olddirfd, &old_path) {
        Ok(dentry) => dentry,
        Err(e) => return Err(e),
    };
    let old_dentry = match resolve_path(old_start_dentry, &old_path) {
        Ok(dentry) => dentry,
        Err(_) => return Err(SysError::ENOENT),
    };
    let old_abs = old_dentry.path();

    let new_start_dentry = match get_start_dentry(newdirfd, &new_path) {
        Ok(dentry) => dentry,
        Err(e) => return Err(e),
    };
    let (new_parent_path, new_name) = split_parent_and_name(&new_path);
    let new_parent = if new_parent_path == "." || new_parent_path == "/" {
        new_start_dentry
    } else {
        match resolve_path(new_start_dentry, &new_parent_path) {
            Ok(dentry) => dentry,
            Err(_) => return Err(SysError::ENOENT),
        }
    };
    if new_name.is_empty() {
        return Err(SysError::EINVAL);
    }
    let new_abs = if new_parent.path() == "/" {
        format!("/{}", new_name)
    } else {
        format!("{}/{}", new_parent.path(), new_name)
    };

    let c_old = match CString::new(old_abs.clone()) {
        Ok(v) => v,
        Err(_) => return Err(SysError::EINVAL),
    };
    let c_new = match CString::new(new_abs.clone()) {
        Ok(v) => v,
        Err(_) => return Err(SysError::EINVAL),
    };

    match ExtFS::rename(&c_old, &c_new) {
        Ok(_) => {
            GLOBAL_DCACHE.remove(&old_abs);
            GLOBAL_DCACHE.remove(&new_abs);
            Ok(0)
        }
        Err(code) => {
            Err(code)
        }
    }
}

///假装成功，直接返回 0
pub fn sys_umount2(target: *const u8, _flags: u32) -> SyscallResult {
    let token = current_user_token();
    let _target_path = translated_str(token, target);
    Ok(0)
}

///假挂载，直接返回 0
pub fn sys_mount(
    source: *const u8,
    mount_path: *const u8,
    fstype: *const u8,
    _flags: usize,
    _data: *const u8,
) -> SyscallResult {
    let token = current_user_token();
    let source_path = translated_str(token, source);
    let mount_path = translated_str(token, mount_path);
    let fstype_path = translated_str(token, fstype);
    info!(
        "[sys_mount] source: {}, mount_point: {}, fstype: {}",
        source_path, mount_path, fstype_path
    );
    let cwd = current_process().inner_exclusive_access().cwd.clone();
    let _mount_dentry = resolve_path(cwd, &mount_path)?;
    Ok(0)
}
///
pub fn sys_chdir(path: *const u8) -> SyscallResult {
    let process = current_process();
    let token = current_user_token();
    let path = translated_str(token, path);
    let mut inner = process.inner_exclusive_access();
    let cwd = inner.cwd.clone();
    if let Ok(target_dentry) = resolve_path(cwd, &path) {
        if target_dentry.get_inode().unwrap().get_types() != InodeTypes::EXT4_DE_DIR {
            return Err(SysError::ENOTDIR);
        }
        inner.cwd = target_dentry;
        Ok(0)
    } else {
        Err(SysError::ENOENT)
    }
}
///
pub fn sys_write(fd: usize, buf: *const u8, len: usize) -> SyscallResult {
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
        return Err(SysError::EINVAL);
    }
    if let Some(file) = &inner.fd_table[fd] {
        // warn!("write {} {}", fd, len);
        if !file.writable() {
            return Err(SysError::EINVAL);
        }
        let file = file.clone();
        // release current task TCB manually to avoid multi-borrow
        drop(inner);

        Ok(file.write(UserBuffer::new(translated_byte_buffer(token, buf, len)))?)
    } else {
        Err(SysError::EBADF)
    }
}
///
pub fn sys_fstat(fd: usize, stat_buf: *mut u8) -> SyscallResult {
    if stat_buf.is_null() {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let process = current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return Err(SysError::EBADF);
    }
    if let Some(file) = &inner.fd_table[fd] {
        let file = file.clone();
        drop(inner);
        let mut stat = Kstat::new();
        match file.get_stat(&mut stat) {
            Ok(_) => {
                let user_stat = kstat_to_linux_stat(&stat);
                let stat_bytes = unsafe {
                    core::slice::from_raw_parts(
                        &user_stat as *const _ as *const u8,
                        core::mem::size_of::<LinuxStat>(),
                    )
                };
                copy_to_user(token, stat_buf, stat_bytes);
                Ok(0)
            }
            Err(e) => Err(e),
        }
    } else {
        Err(SysError::EBADF)
    }
}

pub fn sys_statx(fd: isize, pathname: *const u8, _flags: u32, _mask: usize, buf: *mut u8) -> SyscallResult {
    let token = current_user_token();
    let mut stat = Kstat::new();
    let ret = sys_fstatat(fd, pathname, &mut stat as *mut Kstat as *mut u8, _flags);
    if ret.is_err() {
        return ret;
    }
    let statx = kstat_to_statx(&stat);
    let stat_bytes = unsafe {
        core::slice::from_raw_parts(
            &statx as *const _ as *const u8,
            core::mem::size_of::<Statx>(),
        )
    };
    crate::mm::copy_to_user(token, buf, stat_bytes);

    ret
}

pub fn sys_fstatat(dirfd: isize, path: *const u8, stat_buf: *mut u8, flags: u32) -> SyscallResult {
    if stat_buf.is_null() {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let raw_path = translated_str(token, path);
    info!(
        "[DEBUG] sys_fstatat called: dirfd={}, path={}",
        dirfd, raw_path
    );
    // 标准1：AT_EMPTY_PATH (0x1000)
    // 如果路径为空，且 flags 包含了 AT_EMPTY_PATH，说明它想直接查 dirfd 这个句柄的属性
    const AT_EMPTY_PATH: u32 = 0x1000;
    if raw_path.is_empty() {
        if (flags & AT_EMPTY_PATH) != 0 {
            return sys_fstat(dirfd as usize, stat_buf);
        } else {
            return Err(SysError::ENOENT);
        }
    }

    // 标准2：获取路径解析的起点 dentry
    let start_dentry = match get_start_dentry(dirfd, &raw_path) {
        Ok(dentry) => dentry,
        Err(e) => return Err(e),
    };

    // 标准3：临时打开目标文件（不分配 fd，只为了查属性）
    // 注意：传 RDONLY 即可，哪怕是查目录属性底层也能获取到
    if let Ok(file) = open_file(start_dentry, raw_path.as_str(), OpenFlags::RDONLY) {
        let dentry = file.get_dentry();
        if let Some(inode) = dentry.get_inode() {
            // 对目录/普通文件都统一从 inode 同步一次 size。
            let real_size = inode.get_size() as usize;
            inode.set_size(real_size);
        }
        let mut stat = Kstat::new();
        match file.get_stat(&mut stat) {
            Ok(_) => {
                info!(
                    "[DEBUG] fstatat {}: st_mode={:o} (octal), st_size={}, st_ino={}",
                    raw_path, stat.st_mode, stat.st_size, stat.st_ino
                );
                let is_dir = (stat.st_mode & 0o170000) == 0o040000;
                info!(
                    "[DEBUG] is_dir={}, type_bits={:o}",
                    is_dir,
                    stat.st_mode & 0o170000
                );
                let user_stat = kstat_to_linux_stat(&stat);
                let stat_bytes = unsafe {
                    core::slice::from_raw_parts(
                        &user_stat as *const _ as *const u8,
                        core::mem::size_of::<LinuxStat>(),
                    )
                };
                crate::mm::copy_to_user(token, stat_buf, stat_bytes);
                Ok(0)
            }
            Err(e) => Err(e),
        }
    } else {
        Err(SysError::ENOENT)
    }
}

/// readlinkat: read the target of a symbolic link.
/// Currently Kairix does not fully support symlinks, so this returns -EINVAL
/// for non-symlink paths and -ENOENT if the path does not exist.
pub fn sys_readlinkat(dirfd: isize, path: *const u8, buf: *mut u8, bufsiz: usize) -> SyscallResult {
    let token = current_user_token();
    let raw_path = translated_str(token, path);
    let start_dentry = match get_start_dentry(dirfd, &raw_path) {
        Ok(dentry) => dentry,
        Err(e) => return Err(e),
    };

    let target = match resolve_path(start_dentry, &raw_path) {
        Ok(dentry) => dentry,
        Err(_) => return Err(SysError::ENOENT),
    };
    let inode = match target.get_inode() {
        Some(inode) => inode,
        None => return Err(SysError::ENOENT),
    };

    if !inode.get_mode().contains(InodeMode::LINK) {
        return Err(SysError::EINVAL);
    }

    match inode.readlink() {
        Ok(link_target) => {
            let bytes = link_target.as_bytes();
            let len = bytes.len().min(bufsiz);
            copy_to_user(token, buf, &bytes[..len]);
            Ok(len)
        }
        Err(errno) => {
            let errno = if errno < 0 { errno } else { -errno };
            Err(SysError::try_from(errno as i32).unwrap_or(SysError::EINVAL))
        }
    }
}

pub fn sys_utimensat(dirfd: isize, path: *const u8, times: *const Timespec, _flags: i32) -> SyscallResult {
    let token = current_user_token();
    let inode: alloc::sync::Arc<dyn crate::fs::vfs::inode::Inode> = if path.is_null() {
        // futimens 语义：path 为 NULL 时，直接通过 dirfd 操作文件
        if dirfd == crate::fs::vfs::path::AT_FDCWD {
            return Err(SysError::EFAULT);
        }
        let process = current_process();
        let inner = process.inner_exclusive_access();
        let fd = dirfd as usize;
        if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
            return Err(SysError::EBADF);
        }
        let file = inner.fd_table[fd].as_ref().unwrap();
        match file.get_inode() {
            Some(inode) => inode,
            None => return Err(SysError::EBADF),
        }
    } else {
        let raw_path = translated_str(token, path);
        let start_dentry = match get_start_dentry(dirfd, &raw_path) {
            Ok(dentry) => dentry,
            Err(e) => return Err(e),
        };

        let target = match resolve_path(start_dentry, &raw_path) {
            Ok(dentry) => dentry,
            Err(e) => return Err(e),
        };
        match target.get_inode() {
            Some(inode) => inode,
            None => return Err(SysError::ENOENT),
        }
    };

    let now_us = current_time().as_micros() as i64;
    let now_sec = now_us / 1_000_000;
    let now_nsec = (now_us % 1_000_000) * 1000;

    let (old_atime_sec, old_atime_nsec) = inode.get_atime();
    let (old_mtime_sec, old_mtime_nsec) = inode.get_mtime();

    let (new_atime_sec, new_atime_nsec, new_mtime_sec, new_mtime_nsec) = if times.is_null() {
        (now_sec, now_nsec, now_sec, now_nsec)
    } else {
        let at = translated_ref(token, times);
        let mt = translated_ref(token, unsafe { times.add(1) });

        let map_one = |spec: Timespec,
                       old_sec: i64,
                       old_nsec: i64|
         -> core::result::Result<(i64, i64), SysError> {
            match spec.tv_nsec {
                UTIME_NOW => Ok((now_sec, now_nsec)),
                UTIME_OMIT => Ok((old_sec, old_nsec)),
                nsec if (0..1_000_000_000).contains(&nsec) => Ok((spec.tv_sec, nsec)),
                _ => Err(SysError::EINVAL),
            }
        };

        let (at_sec, at_nsec) = match map_one(*at, old_atime_sec, old_atime_nsec) {
            Ok(v) => v,
            Err(e) => return Err(e),
        };
        let (mt_sec, mt_nsec) = match map_one(*mt, old_mtime_sec, old_mtime_nsec) {
            Ok(v) => v,
            Err(e) => return Err(e),
        };
        (at_sec, at_nsec, mt_sec, mt_nsec)
    };

    inode.set_atime(new_atime_sec, new_atime_nsec);
    inode.set_mtime(new_mtime_sec, new_mtime_nsec);
    inode.set_ctime(now_sec, now_nsec);
    Ok(0)
}

///
pub fn sys_read(fd: usize, buf: *const u8, len: usize) -> SyscallResult {
    let token = current_user_token();
    let process = current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return Err(SysError::EBADF); // EBADF
    }
    if let Some(file) = &inner.fd_table[fd] {
        // warn!("read {} {}", fd, len);
        let file = file.clone();
        // release current task TCB manually to avoid multi-borrow
        drop(inner);

        if !file.readable() {
            return Err(SysError::EINVAL);
        }

        let buffers = crate::mm::translated_byte_buffer(token, buf, len);
        let user_buf = UserBuffer::new(buffers);
        Ok(file.read(user_buf)?)
    } else {
        Err(SysError::EBADF)
    }
}

pub fn sys_lseek(fd: usize, offset: isize, whence: i32) -> SyscallResult {
    const SEEK_SET: i32 = 0;
    const SEEK_CUR: i32 = 1;
    const SEEK_END: i32 = 2;

    let process = current_process();
    let file = {
        let inner = process.inner_exclusive_access();
        if fd >= inner.fd_table.len() {
            return Err(SysError::EBADF);
        }
        match inner.fd_table[fd].as_ref() {
            Some(f) => f.clone(),
            None => return Err(SysError::EBADF),
        }
    };

    // 管道等不可定位对象返回 ESPIPE。
    let inode = match file.get_inode() {
        Some(inode) => inode,
        None => return Err(SysError::ESPIPE),
    };

    let is_dir = inode.get_mode().contains(InodeMode::DIR);

    let cur = file.get_offset() as isize;
    let end = inode.get_size() as isize;
    let new_off = match whence {
        SEEK_SET => offset,
        SEEK_CUR => cur.saturating_add(offset),
        SEEK_END => {
            // 目录流偏移是 getdents 返回的 cookie，不等同于 inode size。
            // 对目录禁止 SEEK_END，避免用户态目录遍历状态机被破坏。
            if is_dir {
                return Err(SysError::EINVAL);
            }
            end.saturating_add(offset)
        }
        _ => return Err(SysError::EINVAL),
    };

    if new_off < 0 {
        return Err(SysError::EINVAL);
    }

    file.set_offset(new_off as usize);
    Ok(new_off as usize)
}

// pub const F_OK: i32 = 0;
// pub const X_OK: i32 = 1;
// pub const W_OK: i32 = 2;
// pub const R_OK: i32 = 4;
///
pub fn sys_faccessat(dirfd: isize, path: *const u8, _mode: u32, _flags: u32) -> SyscallResult {
    let token = current_user_token();
    let raw_path = translated_str(token, path);

    const AT_EMPTY_PATH: u32 = 0x1000;
    if raw_path.is_empty() {
        if (_flags & AT_EMPTY_PATH) != 0 {
            return match get_start_dentry(dirfd, &raw_path) {
                Ok(_) => Ok(0),
                Err(e) => Err(e),
            };
        } else {
            return Err(SysError::ENOENT); // ENOENT: 路径为空且没传 AT_EMPTY_PATH，标准规定算找不到
        }
    }

    let start_dentry = match get_start_dentry(dirfd, &raw_path) {
        Ok(dentry) => dentry,
        Err(e) => return Err(e),
    };

    if resolve_path(start_dentry, &raw_path).is_ok() {
        Ok(0)
    } else {
        Err(SysError::ENOENT)
    }
}

///
pub fn sys_openat(dirfd: isize, path: *const u8, flags: u32) -> SyscallResult {
    // error!("[DEBUG] sys_openat called: dirfd={}, path={}, flags={:#x}", dirfd, translated_str(current_user_token(), path), flags);
    let process = current_process();
    let token = current_user_token();
    let raw_path = translated_str(token, path);
    let safe_flags = OpenFlags::from_bits_truncate(flags & 0xFFF); // 只保留低 12 位，去掉 O_CLOEXEC 等不相关的标志

    let start_dentry = match get_start_dentry(dirfd, &raw_path) {
        Ok(dentry) => dentry,
        Err(e) => return Err(e),
    };

    if let Ok(file) = open_file(start_dentry, raw_path.as_str(), safe_flags) {
        let mut inner = process.inner_exclusive_access();
        if let Some(inode) = file.get_inode() {
            let real_size = inode.get_size() as usize;
            inode.set_size(real_size);
        }
        let fd = inner.alloc_fd();
        inner.fd_table[fd] = Some(file);
        Ok(fd)
    } else {
        error!("sys_open failed for path: {}, returning -1", raw_path);
        Err(SysError::ENOENT)
    }
}
///
pub fn sys_close(fd: usize) -> SyscallResult {
    let process = current_process();
    let pid = process.getpid();
    let mut inner = process.inner_exclusive_access();

    if fd >= inner.fd_table.len() {
        return Err(SysError::EBADF);
    }
    if inner.fd_table[fd].is_none() {
        return Err(SysError::EBADF);
    }
    let file = inner.fd_table[fd].take().unwrap();
    drop(inner);

    // 如果该 fd 关联的是 socket，这里同步清理网络 socket 管理器，避免 fd 复用命中陈旧条目。
    let _ = SOCKET_MANAGER.lock().close_socket(fd, pid);

    file.flush();
    Ok(0)
}

pub fn sys_dup(fd: usize) -> SyscallResult {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();

    let file_clone = if let Some(file) = inner.fd_table.get(fd) {
        file.clone()
    } else {
        return Err(SysError::EBADF);
    };

    let new_fd = inner.alloc_fd();
    inner.fd_table[new_fd] = file_clone;
    Ok(new_fd)
}

pub fn sys_dup2(old_fd: usize, new_fd: usize) -> SyscallResult {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();

    // Linux 语义：dup2(x, x) 直接成功返回，不做关闭与复制。
    if old_fd == new_fd {
        if old_fd >= inner.fd_table.len() || inner.fd_table[old_fd].is_none() {
            return Err(SysError::EBADF);
        }
        return Ok(new_fd);
    }

    let file_clone = if let Some(file) = inner.fd_table.get(old_fd) {
        file.clone()
    } else {
        return Err(SysError::EBADF);
    };
    if new_fd >= inner.fd_table.len() {
        inner.fd_table.resize(new_fd + 1, None);
    }

    // Linux 语义：若 new_fd 已打开，应先关闭它。
    // 当前内核 close 语义包含 flush，因此这里显式 flush 再替换。
    if let Some(old_file) = inner.fd_table[new_fd].take() {
        drop(inner);
        old_file.flush();
        inner = process.inner_exclusive_access();
    }

    inner.fd_table[new_fd] = file_clone;
    Ok(new_fd)
}
pub fn sys_getdents64(fd: usize, buf: *mut u8, len: usize) -> SyscallResult {
    info!("[DEBUG] sys_getdents64 called: fd={}, len={}", fd, len);
    const DIRENT64_HEADER_LEN: usize = 19;

    if len < DIRENT64_HEADER_LEN {
        return Err(SysError::EINVAL);
    }

    let process = current_process();
    let token = current_user_token();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
        return Err(SysError::EBADF);
    }
    let file = inner.fd_table[fd].as_ref().unwrap().clone();
    drop(inner);

    // getdents64 只允许目录 fd；否则不能读取目录项。
    let inode = match file.get_inode() {
        Some(inode) => inode,
        None => return Err(SysError::ENOTDIR),
    };
    if !inode.get_mode().contains(InodeMode::DIR) {
        return Err(SysError::ENOTDIR);
    }

    let entries = file.ls();
    info!("[DEBUG] got {} entries", entries.len());
    // 目录流偏移采用 Linux 风格字节 cookie。
    let start_cookie = file.get_offset();
    let mut encoded_entries: Vec<(&str, u64, u8, usize)> = Vec::new();
    let mut total_cookie = 0usize;
    for (name, ino, d_type) in entries.iter() {
        let name_bytes = name.as_bytes();
        let name_len = name_bytes.len() + 1;
        // 固定头(19) + d_name + '\0'，再按 8 字节对齐
        let reclen = (DIRENT64_HEADER_LEN + name_len + 7) & !7;
        if reclen > u16::MAX as usize {
            // 理论上 ext4 文件名长度不会触发该分支；防御性跳过异常项。
            continue;
        }
        encoded_entries.push((name.as_str(), *ino, *d_type, reclen));
        total_cookie = total_cookie.saturating_add(reclen);
    }

    if start_cookie >= total_cookie {
        return Ok(0);
    }

    let mut kernel_buffer: Vec<u8> = Vec::new();
    let mut next_cookie = start_cookie;
    let mut cur_cookie = 0usize;
    let mut wrote_any = false;

    for (name, ino, d_type, reclen) in encoded_entries.into_iter() {
        if cur_cookie < start_cookie {
            cur_cookie = cur_cookie.saturating_add(reclen);
            continue;
        }

        if kernel_buffer.len() + reclen > len {
            if !wrote_any {
                // Linux 语义：缓冲区连一条记录都放不下时返回 EINVAL。
                return Err(SysError::EINVAL);
            }
            break;
        }

        let name_bytes = name.as_bytes();

        // d_ino: u64 (little-endian)
        kernel_buffer.extend_from_slice(&ino.to_le_bytes());
        // d_off: i64，返回“下一条记录”的目录 cookie。
        let entry_next_cookie = cur_cookie.saturating_add(reclen);
        kernel_buffer.extend_from_slice(&(entry_next_cookie as i64).to_le_bytes());
        // d_reclen: u16
        kernel_buffer.extend_from_slice(&(reclen as u16).to_le_bytes());
        // d_type: u8
        kernel_buffer.push(d_type);

        kernel_buffer.extend_from_slice(name_bytes);
        kernel_buffer.push(0);
        let current_len = DIRENT64_HEADER_LEN + name_bytes.len() + 1;
        let padding = reclen - current_len;
        kernel_buffer.extend(vec![0u8; padding]);
        cur_cookie = entry_next_cookie;
        next_cookie = entry_next_cookie;
        wrote_any = true;
    }
    if !kernel_buffer.is_empty() {
        copy_to_user(token, buf, &kernel_buffer);
    }
    file.set_offset(next_cookie);
    info!(
        "[DEBUG] returning {} bytes, next_cookie={}",
        kernel_buffer.len(),
        next_cookie
    );
    Ok(kernel_buffer.len())
}

///
pub fn sys_fsync(fd: usize) -> SyscallResult {
    let process = current_process();
    let inner = process.inner_exclusive_access();

    if fd >= inner.fd_table.len() {
        return Err(SysError::EBADF);
    }
    if inner.fd_table[fd].is_none() {
        return Err(SysError::EBADF);
    }
    let file = inner.fd_table[fd].as_ref().unwrap();
    file.flush();
    Ok(0)
}

//对已打开的文件描述符进行各种操作
const F_DUPFD: usize = 0;
const F_GETFD: usize = 1;
const F_SETFD: usize = 2;
const F_GETFL: usize = 3;
const F_SETFL: usize = 4;
const F_DUPFD_CLOEXEC: usize = 1030;

pub fn sys_fcntl(fd: usize, cmd: usize, arg: usize) -> SyscallResult {
    let process = crate::task::current_process();
    let mut inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
        return Err(SysError::EINVAL);
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
            Ok(new_fd)
        }
        F_GETFD => {
            // 获取 fd 标志。通常只看有没有 FD_CLOEXEC (值为 1)
            Ok(0)
        }
        F_SETFD => {
            // 设置 fd 标志 (比如设置 FD_CLOEXEC)
            Ok(0)
        }
        F_GETFL => {
            // 获取文件状态标志 (O_RDONLY, O_NONBLOCK 等)
            Ok(2)
        }
        F_SETFL => {
            // 设置文件状态标志 (通常是用来设置 O_NONBLOCK 非阻塞模式)
            Ok(0)
        }
        _ => {
            warn!("Unsupported fcntl cmd: {}", cmd);
            Err(SysError::EINVAL)
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
pub fn sys_writev(fd: usize, iov_ptr: usize, iovcnt: usize) -> SyscallResult {
    let process = crate::task::current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
        return Err(SysError::EINVAL);
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
        let written = file.write(user_buffer)?;
        total_written += written;
    }
    Ok(total_written)
}

// 一次性从同一个文件读取数据到多个不连续的用户缓冲区
pub fn sys_readv(fd: usize, iov_ptr: usize, iovcnt: usize) -> SyscallResult {
    let process = crate::task::current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
        return Err(SysError::EINVAL);
    }
    let file = inner.fd_table[fd].as_ref().unwrap().clone();
    if !file.readable() {
        return Err(SysError::EINVAL);
    }
    drop(inner);

    let token = crate::task::current_user_token();
    let page_table = PageTable::from_token(token);
    let mut total_read = 0;

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
        let buffers = crate::mm::translated_byte_buffer(token, base as *mut u8, len);
        let user_buffer = UserBuffer::new(buffers);
        let read = file.read(user_buffer)?;
        total_read += read;
    }
    Ok(total_read)
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
pub fn sys_ppoll(ufds: usize, nfds: usize, _tmo_p: usize, _sigmask: usize) -> SyscallResult {
    let token = crate::task::current_user_token();
    let mut ready_count = 0;
    for i in 0..nfds {
        let ptr = ufds + i * core::mem::size_of::<PollFd>();
        let pollfd = crate::mm::translated_refmut::<PollFd>(token, ptr as *mut PollFd);
        // 无论在等什么事件，都认为已经发生
        pollfd.revents = pollfd.events;
        ready_count += 1;
    }
    Ok(ready_count)
}

pub fn sys_ioctl(fd: usize, request: usize, argp: usize) -> SyscallResult {
    let request = request as u32 as usize;
    log::info!(
        "[DEBUG] sys_ioctl fd: {}, request: {:#x}, argp: {:#x}",
        fd,
        request,
        argp
    );
    let process = current_process();
    let file = {
        let inner = process.inner_exclusive_access();
        if fd >= inner.fd_table.len() {
            return Err(SysError::EBADF);
        }
        match inner.fd_table[fd].as_ref() {
            Some(f) => f.clone(),
            None => return Err(SysError::EBADF),
        }
    };
    file.ioctl(request, argp)
}

/// * out_fd: 目标 fd（通常是 socket）
/// * in_fd: 源 fd（通常是磁盘文件）
/// * offset_ptr: 用户空间的 offset 指针（可空）
/// * count: 要传输的字节数
pub fn sys_sendfile(out_fd: usize, in_fd: usize, offset_ptr: usize, count: usize) -> SyscallResult {
    info!(
        "[DEBUG] sys_sendfile: out_fd={}, in_fd={}, offset_ptr={}, count={}",
        out_fd, in_fd, offset_ptr, count
    );

    let token = current_user_token();
    let process = current_process();
    let inner = process.inner_exclusive_access();

    let (in_file, out_file) = match (inner.fd_table.get(in_fd), inner.fd_table.get(out_fd)) {
        (Some(Some(in_f)), Some(Some(out_f))) => (in_f.clone(), out_f.clone()),
        _ => return Err(SysError::EBADF),
    };
    drop(inner);
    if !in_file.readable() || !out_file.writable() {
        return Err(SysError::EINVAL);
    }
    if in_file.get_inode().is_none() {
        return Err(SysError::EINVAL);
    }
    let file_size = in_file.get_inode().map(|i| i.get_size()).unwrap_or(0);
    let (mut offset, update_fd) = if offset_ptr != 0 {
        (
            *translated_ref(token, offset_ptr as *const isize) as usize,
            false,
        )
    } else {
        (in_file.get_offset(), true)
    };
    let end = (offset + count).min(file_size);
    let mut total = 0;
    while offset < end {
        let page_id = offset / PAGE_SIZE;
        let page_off = offset % PAGE_SIZE;
        let chunk = (end - offset).min(PAGE_SIZE - page_off);
        let Some(frame) = in_file.get_cache_frame(page_id) else {
            return Err(SysError::EINVAL);
        };
        let bytes = frame.ppn.get_bytes_array();
        let slice = &mut bytes[page_off..page_off + chunk];
        let written = out_file.write(UserBuffer::new(vec![slice]))?;
        if written == 0 {
            break;
        }
        total += written;
        offset += written;
        if written < chunk {
            break;
        }
    }
    if offset_ptr != 0 {
        *translated_refmut(token, offset_ptr as *mut isize) = offset as isize;
    } else if update_fd {
        in_file.set_offset(offset);
    }
    info!("[DEBUG] sendfile transferred {} bytes", total);
    Ok(total)
}

// pub fn sys_sendfile(out_fd: usize, in_fd: usize, offset_ptr: usize, count: usize) -> SyscallResult {
//     info!("[DEBUG] sys_sendfile: out_fd={}, in_fd={}, offset_ptr={}, count={}",
//           out_fd, in_fd, offset_ptr, count);
//     let token = current_user_token();
//     let process = current_process();
//     let inner = process.inner_exclusive_access();
//     if in_fd >= inner.fd_table.len() || inner.fd_table[in_fd].is_none()
//         || out_fd >= inner.fd_table.len() || inner.fd_table[out_fd].is_none() {
//         return Err(SysError::EBADF); // EBADF
//     }
//     let in_file = inner.fd_table[in_fd].as_ref().unwrap().clone();
//     let out_file = inner.fd_table[out_fd].as_ref().unwrap().clone();
//     drop(inner);
//     if !in_file.readable() || !out_file.writable() {
//         return Err(SysError::EINVAL);
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
pub fn sys_syslog(_log_type: usize, _bufp: usize, _len: usize) -> SyscallResult {
    Ok(0)
}

pub fn sys_statfs(path: *const u8, buf: *mut u8) -> SyscallResult {
    if path.is_null() || buf.is_null() {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let raw_path = translated_str(token, path);
    let cwd = current_process().inner_exclusive_access().cwd.clone();
    let dentry = match resolve_path(cwd, &raw_path) {
        Ok(d) => d,
        Err(_) => return Err(SysError::ENOENT),
    };
    let abs_path = dentry.path();
    let sb = match find_superblock_by_path(&abs_path) {
        Some(sb) => sb,
        None => return Err(SysError::ENOENT),
    };
    let stat = sb.statfs();

    let stat_bytes = unsafe {
        core::slice::from_raw_parts(
            &stat as *const _ as *const u8,
            core::mem::size_of::<Statfs>(),
        )
    };
    copy_to_user(token, buf, stat_bytes);
    Ok(0)
}
