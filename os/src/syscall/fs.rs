use crate::error::{SysError, SysResult, SyscallResult};
use crate::alloc::string::ToString;
use core::error;
use polyhal::print;
use polyhal::println;
use polyhal::timer::current_time;
// use crate::config::PAGE_SIZE;
use crate::drivers::BLOCK_DEVICE;
use crate::fs::find_superblock_by_path;
use crate::fs::FS_MANAGER;
use crate::fs::lwext4::ext4::file::ExtFS;
use crate::fs::vfs::OpenFlags;
use crate::fs::vfs::dcache::GLOBAL_DCACHE;
use crate::fs::vfs::file::File;
use crate::fs::vfs::file::open_file;
use crate::fs::vfs::inode::Inode;
use crate::fs::vfs::fstype::MountFlags;
use crate::fs::vfs::inode::InodeMode;
use crate::fs::vfs::kstat::kstat_to_statx;
use crate::fs::vfs::kstat::{Kstat, Statfs, Statx};
use crate::fs::vfs::path::{resolve_path, resolve_path_nofollow_last};
use crate::fs::vfs::path::{get_start_dentry, split_parent_and_name};
use crate::mm::copy_to_user;
use crate::mm::{PageTable, VirtAddr};
use crate::mm::translated_ref;
use crate::mm::{UserBuffer, translated_byte_buffer, translated_refmut, translated_str};
use crate::socket::SOCKET_MANAGER;
use crate::sync::mutex::*;
use crate::sync::mutex::*;
use crate::task::{
    block_current_and_run_next, current_process, current_task, current_user_token,
    suspend_current_and_run_next,
};
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

/// Linux MAX_LFS_FILESIZE for 64-bit: i64::MAX
const MAX_LFS_FILESIZE: usize = i64::MAX as usize;

/// Check whether writing `len` bytes at `offset` would exceed file size limits.
/// Returns EFBIG if it exceeds MAX_LFS_FILESIZE or the process's RLIMIT_FSIZE.
fn check_write_size_limit(offset: usize, len: usize) -> SyscallResult {
    let end = match offset.checked_add(len) {
        Some(v) => v,
        None => return Err(SysError::EFBIG),
    };
    if end > MAX_LFS_FILESIZE {
        return Err(SysError::EFBIG);
    }
    let process = current_process();
    let inner = process.inner_exclusive_access();
    let rlimit_fsize = inner.rlimit_fsize.rlim_cur;
    drop(inner);
    if rlimit_fsize != u64::MAX {
        let limit = rlimit_fsize as usize;
        if end > limit {
            return Err(SysError::EFBIG);
        }
    }
    Ok(0)
}

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
pub fn sys_mkdirat(dirfd: isize, path: *const u8, mode: u32) -> SyscallResult {
    let token = current_user_token();
    let path = translated_str(token, path)?;
    let start_dentry = match get_start_dentry(dirfd, &path) {
        Ok(dentry) => dentry,
        Err(e) => return Err(e),
    };
    let (parent_path, dir_name) = split_parent_and_name(&path);
    if dir_name.is_empty() {
        if path.is_empty() {
            return Err(SysError::ENOENT);
        }
        return Err(SysError::EEXIST);
    }

    let parent = if parent_path == "." || parent_path == "/" {
        start_dentry
    } else {
        match resolve_path(start_dentry, &parent_path) {
            Ok(dentry) => dentry,
            Err(_) => return Err(SysError::ENOENT),
        }
    };
    let process = current_process();
    let umask = process.inner_exclusive_access().umask;
    let effective_mode = InodeMode::from_bits_truncate((mode & 0o7777) & !umask | InodeMode::DIR.bits());
    match parent.create(dir_name.as_str(), effective_mode) {
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

/// Create a special file (device node, fifo, or socket).
pub fn sys_mknodat(dirfd: isize, path: *const u8, mode: u32, dev: u32) -> SyscallResult {
    let token = current_user_token();
    let path = translated_str(token, path)?;
    let start_dentry = match get_start_dentry(dirfd, &path) {
        Ok(dentry) => dentry,
        Err(e) => return Err(e),
    };
    let (parent_path, name) = split_parent_and_name(&path);
    if name.is_empty() {
        if path.is_empty() {
            return Err(SysError::ENOENT);
        }
        return Err(SysError::EEXIST);
    }

    let parent = if parent_path == "." || parent_path == "/" {
        start_dentry
    } else {
        match resolve_path(start_dentry, &parent_path) {
            Ok(dentry) => dentry,
            Err(_) => return Err(SysError::ENOENT),
        }
    };
    let process = current_process();
    let umask = process.inner_exclusive_access().umask;
    let file_type = mode & InodeMode::TYPE_MASK.bits();
    let perm = (mode & 0o7777) & !umask;
    let effective_mode = InodeMode::from_bits_truncate(file_type | perm);
    parent.mknod(name.as_str(), effective_mode, dev)
}

///
pub fn sys_unlinkat(dirfd: isize, path: *const u8, flags: u32) -> SyscallResult {
    let token = current_user_token();
    let path = translated_str(token, path)?;
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
    let old_path = translated_str(token, oldpath)?;
    let new_path = translated_str(token, newpath)?;
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
    let old_path = translated_str(token, oldpath)?;
    let new_path = translated_str(token, newpath)?;

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
        Err(code) => Err(code),
    }
}

/// Unmount a filesystem.
pub fn sys_umount2(target: *const u8, _flags: u32) -> SyscallResult {
    let process = current_process();
    if process.inner_exclusive_access().euid != 0 {
        return Err(SysError::EPERM);
    }
    let token = current_user_token();
    let target_path = translated_str(token, target)?;
    info!("[sys_umount2] target: {}", target_path);

    if target_path == "/" {
        return Err(SysError::EBUSY);
    }

    let cwd = current_process().inner_exclusive_access().cwd.clone();
    let mounted_dentry = resolve_path(cwd.clone(), &target_path)?;

    let (parent_path, name) = split_parent_and_name(&target_path);
    let parent = if parent_path == "/" {
        GLOBAL_DCACHE.get("/").unwrap().clone()
    } else {
        resolve_path(cwd, &parent_path)?
    };

    // Unbind bind-mount fallback
    mounted_dentry.unbind_mount_dentry();
    let mdentry = mounted_dentry.fetch_mount_dentry();

    if let Some(orig) = mdentry {
        let mount_point_abs = if parent.path() == "/" {
            format!("/{}", name)
        } else {
            format!("{}/{}", parent.path(), name)
        };

        // Drop the mounted tree from caches before restoring the covered dentry.
        mounted_dentry.drop_subtree_page_cache();
        mounted_dentry.clear_subtree();
        GLOBAL_DCACHE.remove_subtree(&mount_point_abs);

        // Remove superblock from FsType.supers by mount_point_abs
        {
            let mut fs_mgr = FS_MANAGER.lock();
            for (_name, fstype) in fs_mgr.iter_mut() {
                let mut supers = fstype.inner().supers.lock();
                if supers.remove(&mount_point_abs).is_some() {
                    break;
                }
            }
        }

        // Remove the mounted dentry from parent and restore the original.
        parent.remove_child(&name);
        parent.add_child(orig.clone());
        GLOBAL_DCACHE.insert(mount_point_abs.clone(), orig.clone());

        info!("[sys_umount2] success: restored {} at {}", orig.path(), mount_point_abs);
        Ok(0)
    } else {
        info!("[sys_umount2] fail: no stored mdentry for {}", target_path);
        Err(SysError::EINVAL)
    }
}

fn mount_user_str(token: usize, ptr: *const u8) -> SysResult<String> {
    const PATH_MAX: usize = 4096;

    if ptr.is_null() {
        return Err(SysError::EINVAL);
    }

    let page_table = PageTable::from_token(token);
    let mut string = String::new();
    let mut va = ptr as usize;
    for _ in 0..=PATH_MAX {
        let virt = VirtAddr::from(va);
        let vpn = virt.floor();
        let pte = page_table.translate(vpn).ok_or(SysError::EFAULT)?;
        if !pte.readable() {
            return Err(SysError::EFAULT);
        }
        let pa = page_table.translate_va(virt).ok_or(SysError::EFAULT)?;
        let ch: u8 = *pa.get_mut();
        if ch == 0 {
            return Ok(string);
        }
        string.push(ch as char);
        va += 1;
    }
    Err(SysError::ENAMETOOLONG)
}

/// Mount a filesystem.
pub fn sys_mount(
    source: *const u8,
    mount_path: *const u8,
    fstype: *const u8,
    flags: usize,
    _data: *const u8,
) -> SyscallResult {
    const PATH_MAX: usize = 4096;

    let process = current_process();
    if process.inner_exclusive_access().euid != 0 {
        return Err(SysError::EPERM);
    }
    let token = current_user_token();
    let source_path = mount_user_str(token, source)?;
    let mount_path = mount_user_str(token, mount_path)?;
    let fstype_path = mount_user_str(token, fstype)?;

    if source_path.is_empty() || fstype_path.is_empty() {
        return Err(SysError::EINVAL);
    }
    if source_path.len() > PATH_MAX || mount_path.len() > PATH_MAX || fstype_path.len() > PATH_MAX {
        return Err(SysError::ENAMETOOLONG);
    }

    let flags = MountFlags::from_bits(flags as u32).ok_or(SysError::EINVAL)?;

    info!(
        "[sys_mount] source: {}, mount_point: {}, fstype: {}",
        source_path, mount_path, fstype_path
    );

    let fs_name = match fstype_path.as_str() {
        "ext2" | "ext3" | "ext4" | "vfat" | "fat" | "fat32" | "tmpfs" | "tempfs" => "tmpfs",
        "devfs" => "devfs",
        "proc" | "procfs" => "proc",
        "sysfs" => "sysfs",
        name if FS_MANAGER.lock().contains_key(name) => name,
        _ => return Err(SysError::ENODEV),
    };

    let fs_type = FS_MANAGER.lock().get(fs_name).cloned().ok_or(SysError::ENODEV)?;

    let cwd = current_process().inner_exclusive_access().cwd.clone();
    let mdentry = resolve_path(cwd.clone(), &mount_path)?;
    let mdentry_inode = mdentry.get_inode().ok_or(SysError::ENOENT)?;
    if mdentry_inode.get_mode().get_type() != InodeMode::DIR {
        return Err(SysError::ENOTDIR);
    }

    if flags.contains(MountFlags::MS_REMOUNT) {
        if mdentry.get_mount_dentry().is_none() {
            return Err(SysError::EINVAL);
        }
        return Err(SysError::EBUSY);
    }

    if mdentry.get_mount_dentry().is_some() {
        return Err(SysError::EBUSY);
    }

    let device_backed_fs = matches!(fstype_path.as_str(), "ext2" | "ext3" | "ext4" | "vfat" | "fat" | "fat32");
    let needs_block_device = !matches!(fs_name, "tmpfs" | "devfs" | "proc" | "sysfs");
    if device_backed_fs || needs_block_device {
        let source_dentry = resolve_path(cwd.clone(), &source_path)?;
        let source_inode = source_dentry.get_inode().ok_or(SysError::ENOTBLK)?;
        if source_inode.get_mode().get_type() != InodeMode::BLOCK {
            return Err(SysError::ENOTBLK);
        }
    }

    let (parent_path, name) = split_parent_and_name(&mount_path);
    if name.is_empty() {
        return Err(SysError::EBUSY);
    }
    let parent = if parent_path == "/" {
        GLOBAL_DCACHE.get("/").unwrap().clone()
    } else {
        resolve_path(cwd, &parent_path)?
    };

    let dev = if fs_name == "ext4" || fs_name == "fat32" {
        Some(BLOCK_DEVICE.clone())
    } else {
        None
    };

    let is_bind = flags.contains(MountFlags::MS_BIND);
    let mounted_root = fs_type.mount(&name, Some(parent.clone()), flags, dev)
        .ok_or(SysError::EINVAL)?;

    if is_bind {
        let source_cwd = current_process().inner_exclusive_access().cwd.clone();
        let source_dentry = resolve_path(source_cwd, &source_path)?;
        mounted_root.bind_mount_dentry(source_dentry);
    }
    mounted_root.store_mount_dentry(mdentry.clone());

    let mount_point_abs = if parent.path() == "/" {
        format!("/{}", name)
    } else {
        format!("{}/{}", parent.path(), name)
    };

    GLOBAL_DCACHE.remove_subtree(&mount_point_abs);
    parent.add_child(mounted_root.clone());
    GLOBAL_DCACHE.insert(mount_point_abs.clone(), mounted_root.clone());

    info!("[sys_mount] success: {} mounted at {}", fs_name, mount_point_abs);
    Ok(0)
}
///
pub fn sys_chdir(path: *const u8) -> SyscallResult {
    let process = current_process();
    let token = current_user_token();
    let path = translated_str(token, path)?;
    let mut inner = process.inner_exclusive_access();
    let cwd = inner.cwd.clone();
    info!("[sys_chdir] path={} cwd={}", path, cwd.name());
    if let Ok(target_dentry) = resolve_path(cwd, &path) {
        let types = target_dentry.get_inode().unwrap().get_types();
        info!(
            "[sys_chdir] resolved to {} types={:?}",
            target_dentry.name(),
            types
        );
        if types != InodeTypes::EXT4_DE_DIR {
            return Err(SysError::ENOTDIR);
        }
        inner.cwd = target_dentry;
        Ok(0)
    } else {
        info!("[sys_chdir] resolve_path failed for {}", path);
        Err(SysError::ENOENT)
    }
}
///
pub fn sys_write(fd: usize, buf: *const u8, len: usize) -> SyscallResult {
    // info!("sys_write called for fd: {}", fd);
    let token = current_user_token();

    if fd == 1 || fd == 2 {
        let buffers = translated_byte_buffer(token, buf, len)?;
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
        let offset = file.get_offset();
        // release current task TCB manually to avoid multi-borrow
        drop(inner);

        check_write_size_limit(offset, len)?;
        Ok(file.write(UserBuffer::new(translated_byte_buffer(token, buf, len)?))?)
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

pub fn sys_statx(
    fd: isize,
    pathname: *const u8,
    _flags: u32,
    _mask: usize,
    buf: *mut u8,
) -> SyscallResult {
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

pub fn sys_fchmodat(dirfd: isize, path: *const u8, mode: u32, _flags: i32) -> SyscallResult {
    let token = current_user_token();
    let raw_path = translated_str(token, path)?;

    let start_dentry = match get_start_dentry(dirfd, &raw_path) {
        Ok(dentry) => dentry,
        Err(e) => return Err(e),
    };

    let target = match resolve_path(start_dentry, &raw_path) {
        Ok(dentry) => dentry,
        Err(e) => return Err(e),
    };

    let inode = match target.get_inode() {
        Some(inode) => inode,
        None => return Err(SysError::ENOENT),
    };

    let old_mode = inode.get_mode();
    let new_mode = InodeMode::from_bits_truncate(
        (old_mode.bits() & InodeMode::TYPE_MASK.bits()) | (mode & 0o7777),
    );
    inode.set_mode(new_mode);

    let now_us = current_time().as_micros() as i64;
    inode.set_ctime(now_us / 1_000_000, (now_us % 1_000_000) * 1000);

    Ok(0)
}

pub fn sys_fchownat(dirfd: isize, path: *const u8, owner: u32, group: u32, _flags: i32) -> SyscallResult {
    let token = current_user_token();
    let raw_path = translated_str(token, path)?;

    let start_dentry = match get_start_dentry(dirfd, &raw_path) {
        Ok(dentry) => dentry,
        Err(e) => return Err(e),
    };

    let target = match resolve_path(start_dentry, &raw_path) {
        Ok(dentry) => dentry,
        Err(e) => return Err(e),
    };

    let inode = match target.get_inode() {
        Some(inode) => inode,
        None => return Err(SysError::ENOENT),
    };

    const U32_MAX: u32 = 0xFFFF_FFFF;
    if owner != U32_MAX {
        inode.set_uid(owner as usize);
    }
    if group != U32_MAX {
        inode.set_gid(group as usize);
    }

    let now_us = current_time().as_micros() as i64;
    inode.set_ctime(now_us / 1_000_000, (now_us % 1_000_000) * 1000);

    Ok(0)
}

pub fn sys_fstatat(dirfd: isize, path: *const u8, stat_buf: *mut u8, flags: u32) -> SyscallResult {
    if stat_buf.is_null() {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let raw_path = translated_str(token, path)?;
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
    if let Ok(file) = open_file(start_dentry, raw_path.as_str(), OpenFlags::RDONLY, InodeMode::FILE) {
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
    let raw_path = translated_str(token, path)?;
    let start_dentry = match get_start_dentry(dirfd, &raw_path) {
        Ok(dentry) => dentry,
        Err(e) => return Err(e),
    };

    let target = match resolve_path_nofollow_last(start_dentry, &raw_path) {
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

/// Create a symbolic link.
pub fn sys_symlinkat(target: *const u8, newdirfd: isize, linkpath: *const u8) -> SyscallResult {
    let token = current_user_token();
    let target_str = translated_str(token, target)?;
    let link_path = translated_str(token, linkpath)?;

    let start_dentry = match get_start_dentry(newdirfd, &link_path) {
        Ok(dentry) => dentry,
        Err(e) => return Err(e),
    };

    let (parent_path, name) = split_parent_and_name(&link_path);
    let parent = if parent_path == "." || parent_path == "/" {
        start_dentry
    } else {
        match resolve_path(start_dentry, &parent_path) {
            Ok(dentry) => dentry,
            Err(_) => return Err(SysError::ENOENT),
        }
    };

    if name.is_empty() {
        return Err(SysError::ENOENT);
    }

    if parent.find(name.as_str()).is_ok() {
        return Err(SysError::EEXIST);
    }

    parent.symlink(name.as_str(), target_str.as_str())
}

pub fn sys_utimensat(
    dirfd: isize,
    path: *const u8,
    times: *const Timespec,
    _flags: i32,
) -> SyscallResult {
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
        let raw_path = translated_str(token, path)?;
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
        let at = translated_ref(token, times)?;
        let mt = translated_ref(token, unsafe { times.add(1) })?;

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

        let buffers = translated_byte_buffer(token, buf, len)?;
        let user_buf = UserBuffer::new(buffers);
        Ok(file.read(user_buf)?)
    } else {
        Err(SysError::EBADF)
    }
}

pub fn sys_pread64(fd: usize, buf: *const u8, len: usize, offset: usize) -> SyscallResult {
    let token = current_user_token();
    let process = current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return Err(SysError::EBADF);
    }
    if let Some(file) = &inner.fd_table[fd] {
        let file = file.clone();
        drop(inner);

        if !file.readable() {
            return Err(SysError::EINVAL);
        }
        // pipe/socket 等不支持定位的对象返回 ESPIPE
        if file.get_inode().is_none() {
            return Err(SysError::ESPIPE);
        }

        let old_offset = file.get_offset();
        file.set_offset(offset);

        let buffers = translated_byte_buffer(token, buf, len)?;
        let user_buf = UserBuffer::new(buffers);
        let result = file.read(user_buf);

        file.set_offset(old_offset);
        Ok(result?)
    } else {
        Err(SysError::EBADF)
    }
}

pub fn sys_pwrite64(fd: usize, buf: *const u8, len: usize, offset: usize) -> SyscallResult {
    let token = current_user_token();
    let process = current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return Err(SysError::EBADF);
    }
    if let Some(file) = &inner.fd_table[fd] {
        let file = file.clone();
        drop(inner);

        if !file.writable() {
            return Err(SysError::EINVAL);
        }
        if file.get_inode().is_none() {
            return Err(SysError::ESPIPE);
        }

        check_write_size_limit(offset, len)?;

        let old_offset = file.get_offset();
        file.set_offset(offset);

        let buffers = translated_byte_buffer(token, buf, len)?;
        let user_buf = UserBuffer::new(buffers);
        let result = file.write(user_buf);

        file.set_offset(old_offset);
        Ok(result?)
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

    let is_dir = inode.get_mode().get_type() == InodeMode::DIR;

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

/// 检查当前进程（real uid/gid）对指定 inode 是否有 `mode` 权限。
/// mode: R_OK=4, W_OK=2, X_OK=1
fn check_inode_perm(inode: &Arc<dyn crate::fs::vfs::inode::Inode>, mode: u32) -> bool {
    let file_mode = inode.get_mode();
    let file_uid = inode.get_uid() as u32;
    let file_gid = inode.get_gid() as u32;
    let perm = file_mode.bits() & 0o777;

    let process = current_process();
    let inner = process.inner_exclusive_access();
    let uid = inner.uid;
    let gid = inner.gid;
    drop(inner);
    drop(process);

    if uid == 0 {
        // root: R/W 总是允许；X_OK 要求目录或任意执行位
        if (mode & 1) != 0 {
            let is_dir = file_mode.contains(crate::fs::vfs::inode::InodeMode::DIR);
            let has_exec = (perm & 0o111) != 0;
            return is_dir || has_exec;
        }
        return true;
    } else if uid == file_uid {
        if (mode & 4) != 0 && (perm & 0o400) == 0 { return false; }
        if (mode & 2) != 0 && (perm & 0o200) == 0 { return false; }
        if (mode & 1) != 0 && (perm & 0o100) == 0 { return false; }
    } else if gid == file_gid {
        if (mode & 4) != 0 && (perm & 0o040) == 0 { return false; }
        if (mode & 2) != 0 && (perm & 0o020) == 0 { return false; }
        if (mode & 1) != 0 && (perm & 0o010) == 0 { return false; }
    } else {
        if (mode & 4) != 0 && (perm & 0o004) == 0 { return false; }
        if (mode & 2) != 0 && (perm & 0o002) == 0 { return false; }
        if (mode & 1) != 0 && (perm & 0o001) == 0 { return false; }
    }
    true
}

///
pub fn sys_faccessat(dirfd: isize, path: *const u8, mode: u32, flags: u32) -> SyscallResult {
    let token = current_user_token();
    let raw_path = translated_str(token, path)?;

    // mode 只能是 F_OK(0), X_OK(1), W_OK(2), R_OK(4) 的组合
    if mode > 7 {
        return Err(SysError::EINVAL);
    }

    const AT_EMPTY_PATH: u32 = 0x1000;
    const AT_SYMLINK_NOFOLLOW: u32 = 0x100;
    const PATH_MAX: usize = 4096;

    if raw_path.len() > PATH_MAX {
        return Err(SysError::ENAMETOOLONG);
    }

    if raw_path.is_empty() {
        if (flags & AT_EMPTY_PATH) != 0 {
            return match get_start_dentry(dirfd, &raw_path) {
                Ok(_) => Ok(0),
                Err(e) => Err(e),
            };
        } else {
            return Err(SysError::ENOENT);
        }
    }

    let start_dentry = match get_start_dentry(dirfd, &raw_path) {
        Ok(dentry) => dentry,
        Err(e) => return Err(e),
    };

    let mut parts: Vec<String> = raw_path.split('/').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
    if parts.is_empty() {
        // 路径是 "/"，直接检查 start_dentry 的权限
        let inode = match start_dentry.get_inode() {
            Some(inode) => inode,
            None => return Err(SysError::ENOENT),
        };
        if check_inode_perm(&inode, mode) {
            return Ok(0);
        } else {
            return Err(SysError::EACCES);
        }
    }

    let mut current = start_dentry;
    let mut symlink_count = 0;
    const MAX_SYMLINK_FOLLOWS: usize = 40;

    let mut i = 0;
    while i < parts.len() {
        let part = parts[i].clone();
        let is_last = i == parts.len() - 1;

        match part.as_str() {
            "." => {
                i += 1;
                continue;
            }
            ".." => {
                current = current.parent().unwrap_or(current);
                // 检查新目录的 X_OK（遍历权限）
                if let Some(inode) = current.get_inode() {
                    if !check_inode_perm(&inode, 1) {
                        return Err(SysError::EACCES);
                    }
                }
                i += 1;
                continue;
            }
            name => {
                // 先检查当前目录的 X_OK（需要进入子目录/文件）
                if let Some(inode) = current.get_inode() {
                    if !check_inode_perm(&inode, 1) {
                        return Err(SysError::EACCES);
                    }
                }

                let next_dentry = match current.find(name) {
                    Ok(d) => d,
                    Err(e) => return Err(e),
                };

                // 检查是否为符号链接
                if let Some(inode) = next_dentry.get_inode() {
                    if inode.get_mode().contains(crate::fs::vfs::inode::InodeMode::LINK) {
                        let follow_last = (flags & AT_SYMLINK_NOFOLLOW) == 0;
                        if is_last && !follow_last {
                            // 最后一个组件且不跟随符号链接，检查 symlink 本身的权限
                            if check_inode_perm(&inode, mode) {
                                return Ok(0);
                            } else {
                                return Err(SysError::EACCES);
                            }
                        }

                        if symlink_count >= MAX_SYMLINK_FOLLOWS {
                            return Err(SysError::ELOOP);
                        }
                        symlink_count += 1;

                        let target = inode.readlink().map_err(|e| {
                            let code = if e < 0 { e } else { -e };
                            SysError::try_from(code).unwrap_or(SysError::EINVAL)
                        })?;

                        let is_absolute = target.starts_with('/');

                        let remaining: String = parts[i + 1..].join("/");
                        let new_path = if remaining.is_empty() {
                            target
                        } else if target.ends_with('/') {
                            format!("{}{}", target, remaining)
                        } else {
                            format!("{}/{}", target, remaining)
                        };

                        if is_absolute {
                            current = GLOBAL_DCACHE.get("/").unwrap().clone();
                        }

                        parts = new_path.split('/').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
                        i = 0;
                        continue;
                    }
                }

                if is_last {
                    // 最终目标文件/目录，检查 mode 指定的权限
                    let inode = match next_dentry.get_inode() {
                        Some(inode) => inode,
                        None => return Err(SysError::ENOENT),
                    };

                    // 检查只读文件系统（写权限请求时）
                    if (mode & 2) != 0 {
                        let path_str = next_dentry.path();
                        if let Some(sb) = crate::fs::find_superblock_by_path(&path_str) {
                            if sb.inner().is_readonly() {
                                return Err(SysError::EROFS);
                            }
                        }
                    }

                    if check_inode_perm(&inode, mode) {
                        return Ok(0);
                    } else {
                        return Err(SysError::EACCES);
                    }
                } else {
                    // 中间组件必须是目录
                    if let Some(inode) = next_dentry.get_inode() {
                        if !inode.get_mode().contains(crate::fs::vfs::inode::InodeMode::DIR) {
                            return Err(SysError::ENOTDIR);
                        }
                    }
                    current = next_dentry;
                    i += 1;
                }
            }
        }
    }

    Ok(0)
}

///
pub fn sys_openat(dirfd: isize, path: *const u8, flags: u32, mode: u32) -> SyscallResult {
    // error!("[DEBUG] sys_openat called: dirfd={}, path={}, flags={:#x}", dirfd, translated_str(current_user_token(), path), flags);
    let process = current_process();
    let token = current_user_token();
    let raw_path = translated_str(token, path)?;
    let safe_flags = OpenFlags::from_bits_truncate(flags);
    let has_cloexec = safe_flags.contains(OpenFlags::O_CLOEXEC);

    let start_dentry = match get_start_dentry(dirfd, &raw_path) {
        Ok(dentry) => dentry,
        Err(e) => return Err(e),
    };

    let effective_mode = if safe_flags.contains(OpenFlags::O_CREAT) {
        let inner = process.inner_exclusive_access();
        let umask = inner.umask;
        drop(inner);
        InodeMode::from_bits_truncate((mode & 0o7777) & !umask | InodeMode::FILE.bits())
    } else {
        InodeMode::FILE
    };
    if let Ok(file) = open_file(start_dentry, raw_path.as_str(), safe_flags, effective_mode) {
        let mut inner = process.inner_exclusive_access();
        if let Some(inode) = file.get_inode() {
            let real_size = inode.get_size() as usize;
            inode.set_size(real_size);
        }
        let fd = inner.alloc_fd()?;
        inner.fd_table[fd] = Some(file);
        if has_cloexec {
            if fd < inner.fd_flags.len() {
                inner.fd_flags[fd] |= 1;
            }
        }
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
    if fd < inner.fd_flags.len() {
        inner.fd_flags[fd] = 0;
    }
    drop(inner);
    let _ = SOCKET_MANAGER.lock().close_socket_with_refcount(fd, pid);
    file.flush();
    Ok(0)
}

/// close_range: close all file descriptors in the range [first, last].
/// For now, CLOSE_RANGE_UNSHARE and CLOSE_RANGE_CLOEXEC flags are ignored.
pub fn sys_close_range(first: usize, last: usize, flags: u32) -> SyscallResult {
    const CLOSE_RANGE_UNSHARE: u32 = 1;
    const CLOSE_RANGE_CLOEXEC: u32 = 2;

    if first > last {
        return Err(SysError::EINVAL);
    }
    if flags & !(CLOSE_RANGE_UNSHARE | CLOSE_RANGE_CLOEXEC) != 0 {
        return Err(SysError::EINVAL);
    }

    let process = current_process();
    let pid = process.getpid();
    let mut inner = process.inner_exclusive_access();

    let max_fd = inner.fd_table.len().saturating_sub(1);
    let end = last.min(max_fd);

    // Collect files to close to avoid holding the lock during flush/socket close.
    let mut files_to_close: alloc::vec::Vec<(usize, alloc::sync::Arc<dyn crate::fs::File + Send + Sync>)> = alloc::vec::Vec::new();
    for fd in first..=end {
        if let Some(file) = inner.fd_table[fd].take() {
            files_to_close.push((fd, file));
        }
    }
    drop(inner);

    for (fd, file) in files_to_close {
        let _ = SOCKET_MANAGER.lock().close_socket_with_refcount(fd, pid);
        file.flush();
    }

    Ok(0)
}

pub fn sys_dup(fd: usize) -> SyscallResult {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();

    let file = inner.fd_table.get(fd).ok_or(SysError::EBADF)?;
    let file_clone = file.as_ref().ok_or(SysError::EBADF)?.clone();

    let new_fd = inner.alloc_fd()?;
    inner.fd_table[new_fd] = Some(file_clone);
    Ok(new_fd)
}

pub fn sys_dup2(old_fd: usize, new_fd: usize) -> SyscallResult {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();

    let max_fd = inner.rlimit_nofile.rlim_cur as usize;
    if new_fd >= max_fd {
        return Err(SysError::EBADF);
    }

    // Linux 语义：dup2(x, x) 直接成功返回，不做关闭与复制。
    if old_fd == new_fd {
        if old_fd >= inner.fd_table.len() || inner.fd_table[old_fd].is_none() {
            return Err(SysError::EBADF);
        }
        return Ok(new_fd);
    }

    let file_clone = if let Some(Some(file)) = inner.fd_table.get(old_fd) {
        Some(file.clone())
    } else {
        return Err(SysError::EBADF);
    };
    if new_fd >= inner.fd_table.len() {
        inner.fd_table.resize(new_fd + 1, None);
        inner.fd_flags.resize(new_fd + 1, 0);
    }

    // Linux 语义：若 new_fd 已打开，应先关闭它。
    // 当前内核 close 语义包含 flush，因此这里显式 flush 再替换。
    if let Some(old_file) = inner.fd_table[new_fd].take() {
        drop(inner);
        old_file.flush();
        inner = process.inner_exclusive_access();
    }

    inner.fd_table[new_fd] = file_clone;
    inner.fd_flags[new_fd] = 0;
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

/// sys_sync_file_range: flush a range of a file to disk.
pub fn sys_sync_file_range(fd: usize, offset: i64, nbytes: i64, flags: u32) -> SyscallResult {
    const SYNC_FILE_RANGE_WAIT_BEFORE: u32 = 1;
    const SYNC_FILE_RANGE_WRITE: u32 = 2;
    const SYNC_FILE_RANGE_WAIT_AFTER: u32 = 4;
    const VALID_FLAGS: u32 =
        SYNC_FILE_RANGE_WAIT_BEFORE | SYNC_FILE_RANGE_WRITE | SYNC_FILE_RANGE_WAIT_AFTER;

    if flags & !VALID_FLAGS != 0 {
        return Err(SysError::EINVAL);
    }
    if offset < 0 || nbytes < 0 {
        return Err(SysError::EINVAL);
    }
    if nbytes > 0 {
        if offset.checked_add(nbytes).is_none() {
            return Err(SysError::EINVAL);
        }
    }

    let process = current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
        return Err(SysError::EBADF);
    }
    let file = inner.fd_table[fd].as_ref().unwrap();

    // sync_file_range only works on regular files
    match file.get_inode() {
        Some(inode) => {
            if !inode.get_mode().contains(InodeMode::FILE) {
                return Err(SysError::ESPIPE);
            }
        }
        None => {
            // e.g. pipe
            return Err(SysError::ESPIPE);
        }
    }

    // Current kernel does not support per-range flush;
    // do a full-file flush as best-effort.
    file.flush();
    Ok(0)
}

///
pub fn sys_ftruncate(fd: usize, length: usize) -> SyscallResult {
    let process = current_process();
    let inner = process.inner_exclusive_access();

    if fd >= inner.fd_table.len() {
        return Err(SysError::EBADF);
    }
    if inner.fd_table[fd].is_none() {
        return Err(SysError::EBADF);
    }
    let file = inner.fd_table[fd].as_ref().unwrap().clone();
    drop(inner);
    file.truncate(length as u64)
}

/// sys_fallocate: preallocate or deallocate file space.
/// Currently only supports mode=0 (default) and FALLOC_FL_KEEP_SIZE.
pub fn sys_fallocate(fd: usize, mode: i32, offset: usize, len: usize) -> SyscallResult {
    const FALLOC_FL_KEEP_SIZE: i32 = 0x01;
    let process = current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
        return Err(SysError::EBADF);
    }
    let file = inner.fd_table[fd].as_ref().unwrap().clone();
    drop(inner);

    if !file.writable() {
        return Err(SysError::EBADF);
    }
    let inode = match file.get_inode() {
        Some(inode) => inode,
        None => return Err(SysError::ENODEV),
    };
    if !inode.get_mode().contains(InodeMode::FILE) {
        return Err(SysError::EOPNOTSUPP);
    }
    if len == 0 {
        return Ok(0);
    }
    let end = match offset.checked_add(len) {
        Some(v) => v,
        None => return Err(SysError::EFBIG),
    };
    // 目前仅支持 mode=0 和 FALLOC_FL_KEEP_SIZE
    let supported_modes = FALLOC_FL_KEEP_SIZE;
    if (mode & !supported_modes) != 0 {
        return Err(SysError::EOPNOTSUPP);
    }
    let current_size = inode.get_size();
    if mode == 0 && end > current_size {
        file.truncate(end as u64)
    } else {
        Ok(0)
    }
}

///
pub fn sys_sync() -> SyscallResult {
    let pid_map = crate::task::manager::PID2PCB.lock();
    for (_, process) in pid_map.iter() {
        if let Some(inner) = process.inner_try_access() {
            for fd in 0..inner.fd_table.len() {
                if let Some(file) = inner.fd_table[fd].as_ref() {
                    file.flush();
                }
            }
        }
    }
    Ok(0)
}

//对已打开的文件描述符进行各种操作
const F_DUPFD: usize = 0;
const F_GETFD: usize = 1;
const F_SETFD: usize = 2;
const F_GETFL: usize = 3;
const F_SETFL: usize = 4;
const F_DUPFD_CLOEXEC: usize = 1030;
const F_SETPIPE_SZ: usize = 1031;
const F_GETPIPE_SZ: usize = 1032;

pub fn sys_fcntl(fd: usize, cmd: usize, arg: usize) -> SyscallResult {
    let process = crate::task::current_process();
    let mut inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
        return Err(SysError::EINVAL);
    }

    match cmd {
        F_DUPFD | F_DUPFD_CLOEXEC => {
            let file = inner.fd_table[fd].as_ref().unwrap().clone();
            let max_fd = inner.rlimit_nofile.rlim_cur as usize;
            let mut new_fd = arg;
            // 在 [arg, max_fd) 范围内寻找最小空闲 fd
            while new_fd < max_fd.min(inner.fd_table.len()) && inner.fd_table[new_fd].is_some() {
                new_fd += 1;
            }
            if new_fd >= max_fd {
                return Err(SysError::EMFILE);
            }
            if new_fd >= inner.fd_table.len() {
                inner.fd_table.resize(new_fd + 1, None);
                inner.fd_flags.resize(new_fd + 1, 0);
            }
            inner.fd_table[new_fd] = Some(file);
            if cmd == F_DUPFD_CLOEXEC {
                inner.fd_flags[new_fd] |= 1;
            } else {
                inner.fd_flags[new_fd] &= !1;
            }
            Ok(new_fd)
        }
        F_GETFD => {
            // 获取 fd 标志。通常只看有没有 FD_CLOEXEC (值为 1)
            Ok((inner.fd_flags.get(fd).copied().unwrap_or(0) & 1) as usize)
        }
        F_SETFD => {
            // 设置 fd 标志 (比如设置 FD_CLOEXEC)
            if fd < inner.fd_flags.len() {
                inner.fd_flags[fd] = (inner.fd_flags[fd] & !1) | (arg as u32 & 1);
            }
            // 保持 socket 层同步（部分旧代码通过 socket.flags 判断）
            let pid = process.getpid();
            if let Some(sock) = SOCKET_MANAGER.lock().get_socket_mut(fd, pid) {
                if (arg & 1) != 0 {
                    sock.flags |= 1;
                } else {
                    sock.flags &= !1;
                }
            }
            Ok(0)
        }
        F_GETFL => {
            // 获取文件状态标志 (O_RDONLY, O_NONBLOCK 等)
            let pid = process.getpid();
            if let Some(sock) = SOCKET_MANAGER.lock().get_socket(fd, pid) {
                // socket 默认读写，返回 O_RDWR | flags
                Ok(0o2 | (sock.flags & !1) as usize)
            } else {
                Ok(0o2)
            }
        }
        F_SETFL => {
            // 设置文件状态标志 (通常是用来设置 O_NONBLOCK 非阻塞模式)
            let pid = process.getpid();
            if let Some(sock) = SOCKET_MANAGER.lock().get_socket_mut(fd, pid) {
                // 只允许修改 O_APPEND, O_NONBLOCK, O_ASYNC, O_DIRECT, O_NOATIME, O_DSYNC, O_SYNC
                let settable =
                    0o4000 | 0o2000 | 0o10000 | 0o40000 | 0o100000 | 0o1000000 | 0o4000000;
                sock.flags = (sock.flags & 1) | ((arg as u32) & settable);
            }
            Ok(0)
        }
        F_GETPIPE_SZ => {
            let file = inner.fd_table[fd].as_ref().unwrap().clone();
            drop(inner);
            if let Some(capacity) = file.pipe_capacity() {
                Ok(capacity)
            } else {
                Err(SysError::EINVAL)
            }
        }
        F_SETPIPE_SZ => {
            let file = inner.fd_table[fd].as_ref().unwrap().clone();
            drop(inner);
            file.set_pipe_capacity(arg)?;
            if let Some(capacity) = file.pipe_capacity() {
                Ok(capacity)
            } else {
                Err(SysError::EINVAL)
            }
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
    let mut total_iov_len = 0usize;

    // Calculate total iov length and check limit upfront
    for i in 0..iovcnt {
        let iov_addr = iov_ptr + i * core::mem::size_of::<IoVec>();
        let _base_pa = page_table.translate_va(VirtAddr::from(iov_addr)).unwrap();
        let len_pa = page_table
            .translate_va(VirtAddr::from(iov_addr + 8))
            .unwrap();
        let len = unsafe { *((len_pa.0 + VIRT_ADDR_START) as *const usize) };
        total_iov_len = total_iov_len.saturating_add(len);
    }
    check_write_size_limit(file.get_offset(), total_iov_len)?;

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
        let buffers = translated_byte_buffer(token, base as *const u8, len)?;
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
        let buffers = translated_byte_buffer(token, base as *mut u8, len)?;
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
#[allow(dead_code)]
fn read_user_bytes(token: usize, ptr: *const u8, len: usize) -> SysResult<Vec<u8>> {
    let mut out = Vec::with_capacity(len);
    if len == 0 {
        return Ok(out);
    }
    let parts = translated_byte_buffer(token, ptr, len)?;
    for part in parts {
        out.extend_from_slice(part);
    }
    Ok(out)
}
#[allow(dead_code)]
fn write_user_bytes(token: usize, ptr: *mut u8, src: &[u8]) -> SysResult<()> {
    if src.is_empty() {
        return Ok(());
    }
    let mut copied = 0usize;
    let parts = translated_byte_buffer(token, ptr as *const u8, src.len())?;
    for part in parts {
        let n = part.len();
        part.copy_from_slice(&src[copied..copied + n]);
        copied += n;
    }
    Ok(())
}
#[allow(dead_code)]
fn fd_isset(buf: &[u8], fd: usize) -> bool {
    let byte_idx = fd / 8;
    let bit_idx = fd % 8;
    if byte_idx >= buf.len() {
        return false;
    }
    (buf[byte_idx] & (1u8 << bit_idx)) != 0
}

//暂时"忙轮询"
// ufds: 指向 pollfd 结构体数组的指针
// nfds: 数组的长度

pub fn sys_ppoll(ufds: usize, nfds: usize, tmo_p: usize, _sigmask: usize) -> SyscallResult {
    const POLLIN: i16 = 0x001;
    const POLLOUT: i16 = 0x004;
    const _POLLERR: i16 = 0x008;
    const _POLLHUP: i16 = 0x010;

    let token = crate::task::current_user_token();
    let process = crate::task::current_process();

    // 计算 deadline
    let deadline = if tmo_p != 0 {
        let tmo = *translated_ref(token, tmo_p as *const Timespec)?;
        if tmo.tv_sec < 0 || tmo.tv_nsec < 0 {
            return Err(SysError::EINVAL);
        }
        let timeout_us = tmo.tv_sec as i128 * 1_000_000 + tmo.tv_nsec as i128 / 1_000;
        if timeout_us > 0 {
            Some(current_time().as_micros() as i128 + timeout_us)
        } else {
            Some(current_time().as_micros() as i128)
        }
    } else {
        None
    };

    let mut ready_count;

    loop {
        ready_count = 0;
        for i in 0..nfds {
            let ptr = ufds + i * core::mem::size_of::<PollFd>();
            let pollfd = crate::mm::translated_refmut::<PollFd>(token, ptr as *mut PollFd)?;
            pollfd.revents = 0;
            let fd = pollfd.fd;
            if fd < 0 {
                continue;
            }
            let fd = fd as usize;

            let (readable, writable, _exceptional) = check_fd_ready(&process, fd);
            let events = pollfd.events;
            let mut revents = 0;

            if (events & POLLIN) != 0 && readable {
                revents |= POLLIN;
            }
            if (events & POLLOUT) != 0 && writable {
                revents |= POLLOUT;
            }

            pollfd.revents = revents;
            if revents != 0 {
                ready_count += 1;
            }
        }

        if ready_count > 0 {
            break;
        }

        // 检查是否超时
        if let Some(d) = deadline {
            if (current_time().as_micros() as i128) >= d {
                break;
            }
        }

        // 没有 fd 就绪且未超时：注册 waker 到每个 fd，然后真正阻塞
        let current_task = crate::task::current_task().unwrap();
        for i in 0..nfds {
            let ptr = ufds + i * core::mem::size_of::<PollFd>();
            let pollfd = crate::mm::translated_refmut::<PollFd>(token, ptr as *mut PollFd)?;
            if pollfd.fd < 0 {
                continue;
            }
            let fd = pollfd.fd as usize;
            let inner = process.inner_exclusive_access();
            if fd < inner.fd_table.len() {
                if let Some(file) = &inner.fd_table[fd] {
                    file.register_poll_waker(current_task.clone());
                }
            }
            drop(inner);
        }

        // 如果设置了超时，使用 suspend 轮询而非永久阻塞，
        // 避免内核无法在超时时唤醒任务而导致所有任务死锁。
        if deadline.is_some() {
            crate::task::suspend_current_and_run_next();
        } else {
            block_current_and_run_next();
        }

        // 被唤醒后清除所有 waker 注册
        let current_task = crate::task::current_task().unwrap();
        for i in 0..nfds {
            let ptr = ufds + i * core::mem::size_of::<PollFd>();
            let pollfd = crate::mm::translated_refmut::<PollFd>(token, ptr as *mut PollFd)?;
            if pollfd.fd < 0 {
                continue;
            }
            let fd = pollfd.fd as usize;
            let inner = process.inner_exclusive_access();
            if fd < inner.fd_table.len() {
                if let Some(file) = &inner.fd_table[fd] {
                    file.clear_poll_waker(&current_task);
                }
            }
            drop(inner);
        }
        // 被强制终止信号或被非 SA_RESTART 信号中断后应直接返回 -EINTR
        if process.inner_exclusive_access().is_zombie
            || crate::syscall::signal::should_interrupt_syscall()
        {
            return Err(SysError::EINTR);
        }
    }

    Ok(ready_count)
}

// fd_set helpers for pselect6
const FD_SETSIZE: usize = 1024;

fn fd_set_words(nfds: usize) -> usize {
    (nfds + 63) / 64
}

fn fd_is_set(fds: &[u64], fd: usize) -> bool {
    if fd >= FD_SETSIZE {
        return false;
    }
    (fds[fd / 64] >> (fd % 64)) & 1 != 0
}

fn fd_set_bit(fds: &mut [u64], fd: usize) {
    if fd < FD_SETSIZE {
        fds[fd / 64] |= 1 << (fd % 64);
    }
}


/// 辅助函数：安全地将用户态 fd_set 复制到内核缓冲区
fn copy_fd_set_from_user(token: usize, fds_ptr: *mut u64, words: usize, buf: &mut [u64]) -> SysResult<()> {
    if fds_ptr.is_null() || words == 0 {
        return Ok(());
    }
    let bytes = words * core::mem::size_of::<u64>();
    let user_bufs = translated_byte_buffer(token, fds_ptr as *const u8, bytes)?;
    let mut offset = 0;
    for user_buf in user_bufs {
        for (i, byte) in user_buf.iter().enumerate() {
            let idx = offset + i;
            if idx >= bytes {
                return Ok(());
            }
            let word_idx = idx / 8;
            let byte_idx = idx % 8;
            buf[word_idx] |= (*byte as u64) << (byte_idx * 8);
        }
        offset += user_buf.len();
    }
    Ok(())
}

/// 辅助函数：将内核 fd_set 缓冲区写回用户态
fn copy_fd_set_to_user(token: usize, fds_ptr: *mut u64, words: usize, buf: &[u64]) -> SysResult<()> {
    if fds_ptr.is_null() || words == 0 {
        return Ok(());
    }
    let bytes = words * core::mem::size_of::<u64>();
    let user_bufs = translated_byte_buffer(token, fds_ptr as *const u8, bytes)?;
    let mut offset = 0;
    for user_buf in user_bufs {
        for (i, user_byte) in user_buf.iter_mut().enumerate() {
            let idx = offset + i;
            if idx >= bytes {
                return Ok(());
            }
            let word_idx = idx / 8;
            let byte_idx = idx % 8;
            *user_byte = (buf[word_idx] >> (byte_idx * 8)) as u8;
        }
        offset += user_buf.len();
    }
    Ok(())
}

/// 检查单个 fd 的就绪状态，返回 (readable, writable, exceptional)
fn check_fd_ready(process: &crate::task::ProcessControlBlock, fd: usize) -> (bool, bool, bool) {
    let inner = process.inner_exclusive_access();
    let file = if fd < inner.fd_table.len() {
        inner.fd_table[fd].clone()
    } else {
        None
    };
    drop(inner);

    if let Some(file) = file {
        let mut readable = false;
        let mut writable = false;
        // Socket check
        if file.is_socket() {
            let pid = process.getpid();
            let manager = SOCKET_MANAGER.lock();
            if let Some(sock) = manager.get_socket(fd, pid) {
                match &sock.inner {
                    crate::socket::SocketInner::Tcp(tcp) => {
                        let tcp_guard = tcp.lock();
                        readable = !tcp_guard.receive_queue.lock().is_empty()
                            || matches!(
                                tcp_guard.state,
                                crate::socket::tcp::TcpSocketState::CloseWait
                                    | crate::socket::tcp::TcpSocketState::LastAck
                                    | crate::socket::tcp::TcpSocketState::Closed
                                    | crate::socket::tcp::TcpSocketState::FinWait1
                                    | crate::socket::tcp::TcpSocketState::FinWait2
                            )
                            || (matches!(
                                tcp_guard.state,
                                crate::socket::tcp::TcpSocketState::Listening
                            ) && !tcp_guard.accept_queue.lock().is_empty());
                        writable = !matches!(
                            tcp_guard.state,
                            crate::socket::tcp::TcpSocketState::Closed
                        );
                    }
                    crate::socket::SocketInner::Udp(udp) => {
                        let udp_guard = udp.lock();
                        readable = !udp_guard.receive_queue.lock().is_empty();
                        writable = true;
                    }
                    crate::socket::SocketInner::Raw(_) => {
                        readable = true;
                        writable = true;
                    }
                }
            }
        } else if file.is_pipe() {
            if file.readable() {
                readable = file.pipe_has_data();
            }
            if file.writable() {
                writable = file.pipe_has_space();
            }
        } else {
            // 普通文件：总是就绪
            if file.readable() {
                readable = true;
            }
            if file.writable() {
                writable = true;
            }
        }
        (readable, writable, false)
    } else {
        (false, false, false)
    }
}

/// Simplified pselect6: checks fd validity and handles timeout.
/// If no fds are ready and timeout is non-null, sleeps until timeout.
pub fn sys_pselect6(
    nfds: usize,
    readfds: *mut u64,
    writefds: *mut u64,
    exceptfds: *mut u64,
    timeout: *mut Timespec,
    _sigmask: *mut u8,
) -> SyscallResult {
    if nfds > FD_SETSIZE {
        return Err(SysError::EINVAL);
    }

    let token = current_user_token();
    let process = current_process();
    let words = fd_set_words(nfds);

    // 将用户态 fd_set 复制到内核（输入）
    let mut input_read = vec![0u64; words];
    let mut input_write = vec![0u64; words];
    let mut input_except = vec![0u64; words];
    copy_fd_set_from_user(token, readfds, words, &mut input_read)?;
    copy_fd_set_from_user(token, writefds, words, &mut input_write)?;
    copy_fd_set_from_user(token, exceptfds, words, &mut input_except)?;

    // 输出 fd_set
    let mut output_read = vec![0u64; words];
    let mut output_write = vec![0u64; words];
    let mut output_except = vec![0u64; words];

    let mut ready_count;

    // 计算 deadline
    let deadline = if !timeout.is_null() {
        let ts = *translated_ref(token, timeout)?;
        if ts.tv_sec < 0 || ts.tv_nsec < 0 {
            return Err(SysError::EINVAL);
        }
        let timeout_us = ts.tv_sec as i128 * 1_000_000 + ts.tv_nsec as i128 / 1_000;
        if timeout_us > 0 {
            Some(current_time().as_micros() as i128 + timeout_us)
        } else {
            Some(current_time().as_micros() as i128)
        }
    } else {
        None
    };

    loop {
        ready_count = 0;
        // 清除输出 fd_set
        for i in 0..words {
            output_read[i] = 0;
            output_write[i] = 0;
            output_except[i] = 0;
        }

        for fd in 0..nfds {
            let (readable, writable, _exceptional) = check_fd_ready(&process, fd);
            if readfds != core::ptr::null_mut() && fd_is_set(&input_read, fd) && readable {
                fd_set_bit(&mut output_read, fd);
                ready_count += 1;
            }
            if writefds != core::ptr::null_mut() && fd_is_set(&input_write, fd) && writable {
                fd_set_bit(&mut output_write, fd);
                ready_count += 1;
            }
            if exceptfds != core::ptr::null_mut() && fd_is_set(&input_except, fd) {
                // 简化：不报告异常
            }
        }

        if ready_count > 0 {
            break;
        }

        // 检查是否超时
        if let Some(d) = deadline {
            if (current_time().as_micros() as i128) >= d {
                break;
            }
        }

        // 没有 fd 就绪且未超时：注册 waker 到每个关心的 fd，然后真正阻塞
        let current_task = crate::task::current_task().unwrap();
        for fd in 0..nfds {
            let mut should_register = false;
            if readfds != core::ptr::null_mut() && fd_is_set(&input_read, fd) {
                should_register = true;
            }
            if writefds != core::ptr::null_mut() && fd_is_set(&input_write, fd) {
                should_register = true;
            }
            if exceptfds != core::ptr::null_mut() && fd_is_set(&input_except, fd) {
                should_register = true;
            }
            if should_register {
                let inner = process.inner_exclusive_access();
                if fd < inner.fd_table.len() {
                    if let Some(file) = &inner.fd_table[fd] {
                        file.register_poll_waker(current_task.clone());
                    }
                }
                drop(inner);
            }
        }

        // 如果设置了超时，使用 suspend 轮询而非永久阻塞，
        // 避免内核无法在超时时唤醒任务而导致所有任务死锁。
        if deadline.is_some() {
            crate::task::suspend_current_and_run_next();
        } else {
            block_current_and_run_next();
        }

        // 被唤醒后清除所有 waker 注册
        let current_task = crate::task::current_task().unwrap();
        for fd in 0..nfds {
            let mut should_clear = false;
            if readfds != core::ptr::null_mut() && fd_is_set(&input_read, fd) {
                should_clear = true;
            }
            if writefds != core::ptr::null_mut() && fd_is_set(&input_write, fd) {
                should_clear = true;
            }
            if exceptfds != core::ptr::null_mut() && fd_is_set(&input_except, fd) {
                should_clear = true;
            }
            if should_clear {
                let inner = process.inner_exclusive_access();
                if fd < inner.fd_table.len() {
                    if let Some(file) = &inner.fd_table[fd] {
                        file.clear_poll_waker(&current_task);
                    }
                }
                drop(inner);
            }
        }
        // 被强制终止信号或被非 SA_RESTART 信号中断后应直接返回 -EINTR
        if process.inner_exclusive_access().is_zombie
            || crate::syscall::signal::should_interrupt_syscall()
        {
            return Err(SysError::EINTR);
        }
    }

    // 将结果写回用户态
    copy_fd_set_to_user(token, readfds, words, &output_read)?;
    copy_fd_set_to_user(token, writefds, words, &output_write)?;
    copy_fd_set_to_user(token, exceptfds, words, &output_except)?;

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
            *translated_ref(token, offset_ptr as *const isize)? as usize,
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
        *translated_refmut(token, offset_ptr as *mut isize)? = offset as isize;
    } else if update_fd {
        in_file.set_offset(offset);
    }
    info!("[DEBUG] sendfile transferred {} bytes", total);
    Ok(total)
}

pub fn sys_copy_file_range(
    fd_in: usize,
    off_in: usize,
    fd_out: usize,
    off_out: usize,
    len: usize,
    flags: usize,
) -> SyscallResult {
    let token = current_user_token();
    let process = current_process();
    let inner = process.inner_exclusive_access();

    let (in_file, out_file) = match (inner.fd_table.get(fd_in), inner.fd_table.get(fd_out)) {
        (Some(Some(in_f)), Some(Some(out_f))) => (in_f.clone(), out_f.clone()),
        _ => return Err(SysError::EBADF),
    };
    drop(inner);

    if flags != 0 {
        return Err(SysError::EINVAL);
    }

    // Check file types first (before permissions), matching Linux kernel order
    if in_file.is_pipe() || out_file.is_pipe() {
        return Err(SysError::EINVAL);
    }
    let file_type_ok = |file: &Arc<dyn File + Send + Sync>| -> SyscallResult {
        if let Some(inode) = file.get_inode() {
            let mode = inode.get_mode();
            let ftype = mode & InodeMode::TYPE_MASK;
            if ftype == InodeMode::DIR {
                return Err(SysError::EISDIR);
            }
            if ftype != InodeMode::FILE {
                return Err(SysError::EINVAL);
            }
        } else {
            return Err(SysError::EINVAL);
        }
        Ok(0)
    };
    file_type_ok(&in_file)?;
    file_type_ok(&out_file)?;

    if !in_file.readable() || !out_file.writable() {
        return Err(SysError::EBADF);
    }

    if out_file.is_append() {
        return Err(SysError::EBADF);
    }

    let saved_in_offset = in_file.get_offset();
    let saved_out_offset = out_file.get_offset();

    let current_in_off = if off_in != 0 {
        let off = *translated_ref(token, off_in as *const i64)?;
        if off < 0 {
            return Err(SysError::EINVAL);
        }
        off as usize
    } else {
        saved_in_offset
    };

    let current_out_off = if off_out != 0 {
        let off = *translated_ref(token, off_out as *const i64)?;
        if off < 0 {
            return Err(SysError::EINVAL);
        }
        off as usize
    } else {
        saved_out_offset
    };

    // Check for offset overflow
    if current_in_off.checked_add(len).is_none()
        || current_out_off.checked_add(len).is_none()
    {
        return Err(SysError::EOVERFLOW);
    }

    // Check file size limit for output
    check_write_size_limit(current_out_off, len)?;

    // Check overlapping range for the same file
    if len > 0 {
        if let (Some(in_inode), Some(out_inode)) = (in_file.get_inode(), out_file.get_inode()) {
            if in_inode.get_ino() == out_inode.get_ino() {
                let in_path = in_file.get_dentry().path();
                let out_path = out_file.get_dentry().path();
                if in_path == out_path {
                    if current_in_off < current_out_off + len
                        && current_out_off < current_in_off + len
                    {
                        return Err(SysError::EINVAL);
                    }
                }
            }
        }
    }

    let mut total_copied = 0usize;
    const BUF_SIZE: usize = 4096;
    let mut buffer = [0u8; BUF_SIZE];

    while total_copied < len {
        let chunk = (len - total_copied).min(BUF_SIZE);

        // Read from input file
        let read_off = current_in_off + total_copied;
        in_file.set_offset(read_off);
        let read_buf: &'static mut [u8] =
            unsafe { core::slice::from_raw_parts_mut(buffer.as_mut_ptr(), chunk) };
        let read_bytes = match in_file.read(UserBuffer::new(vec![read_buf])) {
            Ok(n) => n,
            Err(e) => {
                if off_in != 0 {
                    in_file.set_offset(saved_in_offset);
                }
                if off_out != 0 {
                    out_file.set_offset(saved_out_offset);
                }
                return Err(e);
            }
        };
        if read_bytes == 0 {
            break;
        }

        // Write to output file
        let write_off = current_out_off + total_copied;
        out_file.set_offset(write_off);
        let write_buf: &'static mut [u8] =
            unsafe { core::slice::from_raw_parts_mut(buffer.as_mut_ptr(), read_bytes) };
        let written = match out_file.write(UserBuffer::new(vec![write_buf])) {
            Ok(n) => n,
            Err(e) => {
                if off_in != 0 {
                    in_file.set_offset(saved_in_offset);
                }
                if off_out != 0 {
                    out_file.set_offset(saved_out_offset);
                }
                return Err(e);
            }
        };
        total_copied += written;
        if written < read_bytes {
            break;
        }
    }

    // Update offsets according to copy_file_range semantics
    if off_in != 0 {
        *translated_refmut(token, off_in as *mut i64)? = (current_in_off + total_copied) as i64;
        in_file.set_offset(saved_in_offset);
    } else {
        in_file.set_offset(current_in_off + total_copied);
    }

    if off_out != 0 {
        *translated_refmut(token, off_out as *mut i64)? = (current_out_off + total_copied) as i64;
        out_file.set_offset(saved_out_offset);
    } else {
        out_file.set_offset(current_out_off + total_copied);
    }

    Ok(total_copied)
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
//         current_offset = *translated_ref(token, offset_ptr as *const isize)? as usize;
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
//         *translated_refmut(token, offset_ptr as *mut isize)? = (current_offset + total) as isize;
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
    let raw_path = translated_str(token, path)?;
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

/// Set the file mode creation mask and return the old mask.
pub fn sys_umask(mask: u32) -> SyscallResult {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    let old = inner.umask;
    inner.umask = mask & 0o777;
    Ok(old as usize)
}


// ---------- xattr syscalls ----------

/// Helper: get inode from fd, returning EBADF if invalid.
fn fd_to_inode(fd: usize) -> SysResult<Arc<dyn Inode>> {
    let process = current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
        return Err(SysError::EBADF);
    }
    let file = inner.fd_table[fd].as_ref().unwrap().clone();
    drop(inner);
    file.get_inode().ok_or(SysError::EBADF)
}

/// Helper: resolve path to dentry.
fn path_to_dentry(path: *const u8, follow_last_link: bool) -> SysResult<Arc<dyn crate::fs::vfs::Dentry>> {
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

/// syscall: fsetxattr
pub fn sys_fsetxattr(fd: usize, name: *const u8, value: *const u8, size: usize, flags: i32) -> SyscallResult {
    if name.is_null() {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let name_str = translated_str(token, name)?;
    let value_buf = if value.is_null() && size > 0 {
        return Err(SysError::EFAULT);
    } else if size > 0 {
        translated_byte_buffer(token, value, size)?
            .into_iter()
            .flat_map(|b| b.iter().copied())
            .collect::<Vec<u8>>()
    } else {
        Vec::new()
    };
    let inode = fd_to_inode(fd)?;
    inode.setxattr(&name_str, &value_buf, flags)
}

/// syscall: fgetxattr
pub fn sys_fgetxattr(fd: usize, name: *const u8, buf: *mut u8, size: usize) -> SyscallResult {
    if name.is_null() {
        return Err(SysError::EFAULT);
    }
    if buf.is_null() && size > 0 {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let name_str = translated_str(token, name)?;
    let mut dst = if buf.is_null() || size == 0 {
        Vec::new()
    } else {
        vec![0u8; size]
    };
    let inode = fd_to_inode(fd)?;
    let ret = inode.getxattr(&name_str, &mut dst)?;
    if !buf.is_null() && size > 0 {
        copy_to_user(token, buf, &dst[..ret.min(size)]);
    }
    Ok(ret)
}

/// syscall: flistxattr
pub fn sys_flistxattr(fd: usize, buf: *mut u8, size: usize) -> SyscallResult {
    if buf.is_null() && size > 0 {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let mut dst = if buf.is_null() || size == 0 {
        Vec::new()
    } else {
        vec![0u8; size]
    };
    let inode = fd_to_inode(fd)?;
    let ret = inode.listxattr(&mut dst)?;
    if !buf.is_null() && size > 0 {
        copy_to_user(token, buf, &dst[..ret.min(size)]);
    }
    Ok(ret)
}

/// syscall: fremovexattr
pub fn sys_fremovexattr(fd: usize, name: *const u8) -> SyscallResult {
    if name.is_null() {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let name_str = translated_str(token, name)?;
    let inode = fd_to_inode(fd)?;
    inode.removexattr(&name_str)
}

/// syscall: setxattr
pub fn sys_setxattr(path: *const u8, name: *const u8, value: *const u8, size: usize, flags: i32) -> SyscallResult {
    if name.is_null() {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let name_str = translated_str(token, name)?;
    let value_buf = if value.is_null() && size > 0 {
        return Err(SysError::EFAULT);
    } else if size > 0 {
        translated_byte_buffer(token, value, size)?
            .into_iter()
            .flat_map(|b| b.iter().copied())
            .collect::<Vec<u8>>()
    } else {
        Vec::new()
    };
    let dentry = path_to_dentry(path, true)?;
    let inode = dentry.get_inode().ok_or(SysError::ENOENT)?;
    inode.setxattr(&name_str, &value_buf, flags)
}

/// syscall: lsetxattr (does not follow symlink on last component)
pub fn sys_lsetxattr(path: *const u8, name: *const u8, value: *const u8, size: usize, flags: i32) -> SyscallResult {
    if name.is_null() {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let name_str = translated_str(token, name)?;
    let value_buf = if value.is_null() && size > 0 {
        return Err(SysError::EFAULT);
    } else if size > 0 {
        translated_byte_buffer(token, value, size)?
            .into_iter()
            .flat_map(|b| b.iter().copied())
            .collect::<Vec<u8>>()
    } else {
        Vec::new()
    };
    let dentry = path_to_dentry(path, false)?;
    let inode = dentry.get_inode().ok_or(SysError::ENOENT)?;
    inode.setxattr(&name_str, &value_buf, flags)
}

/// syscall: getxattr
pub fn sys_getxattr(path: *const u8, name: *const u8, buf: *mut u8, size: usize) -> SyscallResult {
    if name.is_null() {
        return Err(SysError::EFAULT);
    }
    if buf.is_null() && size > 0 {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let name_str = translated_str(token, name)?;
    let mut dst = if buf.is_null() || size == 0 {
        Vec::new()
    } else {
        vec![0u8; size]
    };
    let dentry = path_to_dentry(path, true)?;
    let inode = dentry.get_inode().ok_or(SysError::ENOENT)?;
    let ret = inode.getxattr(&name_str, &mut dst)?;
    if !buf.is_null() && size > 0 {
        copy_to_user(token, buf, &dst[..ret.min(size)]);
    }
    Ok(ret)
}

/// syscall: lgetxattr (does not follow symlink on last component)
pub fn sys_lgetxattr(path: *const u8, name: *const u8, buf: *mut u8, size: usize) -> SyscallResult {
    if name.is_null() {
        return Err(SysError::EFAULT);
    }
    if buf.is_null() && size > 0 {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let name_str = translated_str(token, name)?;
    let mut dst = if buf.is_null() || size == 0 {
        Vec::new()
    } else {
        vec![0u8; size]
    };
    let dentry = path_to_dentry(path, false)?;
    let inode = dentry.get_inode().ok_or(SysError::ENOENT)?;
    let ret = inode.getxattr(&name_str, &mut dst)?;
    if !buf.is_null() && size > 0 {
        copy_to_user(token, buf, &dst[..ret.min(size)]);
    }
    Ok(ret)
}

/// syscall: listxattr
pub fn sys_listxattr(path: *const u8, buf: *mut u8, size: usize) -> SyscallResult {
    if buf.is_null() && size > 0 {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let mut dst = if buf.is_null() || size == 0 {
        Vec::new()
    } else {
        vec![0u8; size]
    };
    let dentry = path_to_dentry(path, true)?;
    let inode = dentry.get_inode().ok_or(SysError::ENOENT)?;
    let ret = inode.listxattr(&mut dst)?;
    if !buf.is_null() && size > 0 {
        copy_to_user(token, buf, &dst[..ret.min(size)]);
    }
    Ok(ret)
}

/// syscall: llistxattr (does not follow symlink on last component)
pub fn sys_llistxattr(path: *const u8, buf: *mut u8, size: usize) -> SyscallResult {
    if buf.is_null() && size > 0 {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let mut dst = if buf.is_null() || size == 0 {
        Vec::new()
    } else {
        vec![0u8; size]
    };
    let dentry = path_to_dentry(path, false)?;
    let inode = dentry.get_inode().ok_or(SysError::ENOENT)?;
    let ret = inode.listxattr(&mut dst)?;
    if !buf.is_null() && size > 0 {
        copy_to_user(token, buf, &dst[..ret.min(size)]);
    }
    Ok(ret)
}

/// syscall: removexattr
pub fn sys_removexattr(path: *const u8, name: *const u8) -> SyscallResult {
    if name.is_null() {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let name_str = translated_str(token, name)?;
    let dentry = path_to_dentry(path, true)?;
    let inode = dentry.get_inode().ok_or(SysError::ENOENT)?;
    inode.removexattr(&name_str)
}

/// syscall: lremovexattr (does not follow symlink on last component)
pub fn sys_lremovexattr(path: *const u8, name: *const u8) -> SyscallResult {
    if name.is_null() {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let name_str = translated_str(token, name)?;
    let dentry = path_to_dentry(path, false)?;
    let inode = dentry.get_inode().ok_or(SysError::ENOENT)?;
    inode.removexattr(&name_str)
}
