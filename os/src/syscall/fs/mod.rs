use crate::alloc::string::ToString;
use crate::error::{SysError, SysResult, SyscallResult};
use core::error;
use core::sync::atomic::{AtomicBool, Ordering};
use polyhal::print;
use polyhal::timer::current_time;
// use crate::config::PAGE_SIZE;
use crate::devices::BlockDevice;
use crate::drivers::BLOCK_DEVICE;
use crate::fs::FS_MANAGER;
use crate::fs::config::FD_CLOEXEC_FLAG;
use crate::fs::devfs::loopx::loop_block_device_from_inode;
use crate::fs::find_superblock_by_path;
use crate::fs::notify::fanotify::{
    FAN_ACCESS, FAN_ACCESS_PERM, FAN_ATTRIB, FAN_CLOSE_NOWRITE, FAN_CLOSE_WRITE, FAN_CREATE,
    FAN_MODIFY, FAN_OPEN, FAN_OPEN_PERM, fanotify_check_permission_dentry,
    fanotify_may_have_instances, fanotify_notify_delete_dentry, fanotify_notify_dentry,
    fanotify_notify_move, fanotify_notify_path, fanotify_notify_unmount,
};
use crate::fs::notify::inotify::{
    IN_ACCESS, IN_ATTRIB, IN_CLOSE_NOWRITE, IN_CLOSE_WRITE, IN_CREATE, IN_ISDIR, IN_MODIFY,
    IN_OPEN, inotify_may_have_instances, inotify_notify_delete, inotify_notify_delete_dentry,
    inotify_notify_dentry, inotify_notify_move, inotify_notify_move_dentry, inotify_notify_path,
    inotify_notify_unmount,
};
use crate::fs::notify::{
    NotifyTarget, notify_access, notify_access_permission, notify_attrib, notify_modify,
    notify_path_access, notify_path_modify, notify_target_for_file_if_needed,
};
use crate::fs::tmpfs::dentry::TempDentry;
use crate::fs::tmpfs::file::TempFile;
use crate::fs::tmpfs::inode::F_SEAL_SEAL;
use crate::fs::tmpfs::inode::TempInode;
use crate::fs::vfs::OpenFlags;
use crate::fs::vfs::dcache::GLOBAL_DCACHE;
use crate::fs::vfs::file::FS_IOC_SETFLAGS;
use crate::fs::vfs::file::File;
use crate::fs::vfs::file::create_file_at;
use crate::fs::vfs::file::open_resolved_file;
use crate::fs::vfs::fstype::MountFlags;
use crate::fs::vfs::inode::Inode;
use crate::fs::vfs::inode::InodeMode;
use crate::fs::vfs::path::{get_start_dentry, split_parent_and_name};
use crate::fs::vfs::path::{resolve_path, resolve_path_nofollow_last};
use crate::mm::PageTable;
use crate::mm::VirtAddr;
use crate::mm::copy_to_user;
use crate::mm::translated_ref;
use crate::mm::{UserBuffer, translated_byte_buffer, translated_refmut, translated_str};
use crate::security::landlock::{
    LANDLOCK_ACCESS_FS_IOCTL_DEV, LANDLOCK_ACCESS_FS_MAKE_BLOCK, LANDLOCK_ACCESS_FS_MAKE_CHAR,
    LANDLOCK_ACCESS_FS_MAKE_DIR, LANDLOCK_ACCESS_FS_MAKE_FIFO, LANDLOCK_ACCESS_FS_MAKE_REG,
    LANDLOCK_ACCESS_FS_MAKE_SOCK, LANDLOCK_ACCESS_FS_MAKE_SYM, LANDLOCK_ACCESS_FS_READ_DIR,
    LANDLOCK_ACCESS_FS_READ_FILE, LANDLOCK_ACCESS_FS_REFER, LANDLOCK_ACCESS_FS_REMOVE_DIR,
    LANDLOCK_ACCESS_FS_REMOVE_FILE, LANDLOCK_ACCESS_FS_TRUNCATE, LANDLOCK_ACCESS_FS_WRITE_FILE,
    landlock_check_dentry, landlock_check_path,
};
use crate::sync::mutex::*;
use crate::task::{current_process, current_task, current_user_token};
#[cfg(target_arch = "riscv64")]
use crate::timer::get_time_us;
use crate::timer::realtime_timespec;
use crate::trap::_set_sum_bit;
use alloc::ffi::CString;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use log::*;
use log::{error, warn};
use lwext4_rust::InodeTypes;
use polyhal::consts::*;

mod fanotify;
mod fd;
mod inotify;
mod io;
mod mount;
mod new_mount;
mod poll;
mod record_lock;
mod splice;
mod stat;
mod xattr;
pub use fanotify::*;
pub use fd::*;
pub use inotify::*;
pub use io::*;
pub use mount::*;
pub use new_mount::*;
pub use poll::*;
pub(crate) use record_lock::sys_flock;
pub(crate) use record_lock::{
    release_file_description_flock_if_unreferenced, release_process_file_locks,
    release_process_record_locks,
};
pub use splice::*;
pub use stat::*;
pub use xattr::*;

const PATH_MAX: usize = 4096;
const NAME_MAX: usize = 255;

const OPEN_HOW_SIZE: usize = core::mem::size_of::<OpenHow>();
const O_TMPFILE: u64 = OpenFlags::O_TMPFILE.bits() as u64;
const VALID_OPENAT2_FLAGS: u64 = (OpenFlags::WRONLY.bits()
    | OpenFlags::RDWR.bits()
    | OpenFlags::O_CREAT.bits()
    | OpenFlags::O_EXCL.bits()
    | OpenFlags::O_TRUNC.bits()
    | OpenFlags::O_APPEND.bits()
    | OpenFlags::O_NONBLOCK.bits()
    | OpenFlags::O_DIRECTORY.bits()
    | OpenFlags::O_NOFOLLOW.bits()
    | OpenFlags::O_NOATIME.bits()
    | OpenFlags::O_CLOEXEC.bits()) as u64
    | O_TMPFILE;
const RESOLVE_NO_XDEV: u64 = 0x01;
const RESOLVE_NO_MAGICLINKS: u64 = 0x02;
const RESOLVE_NO_SYMLINKS: u64 = 0x04;
const RESOLVE_BENEATH: u64 = 0x08;
const RESOLVE_IN_ROOT: u64 = 0x10;
const VALID_OPENAT2_RESOLVE: u64 = RESOLVE_NO_XDEV
    | RESOLVE_NO_MAGICLINKS
    | RESOLVE_NO_SYMLINKS
    | RESOLVE_BENEATH
    | RESOLVE_IN_ROOT;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct OpenHow {
    pub flags: u64,
    pub mode: u64,
    pub resolve: u64,
}

pub(super) fn check_open_path_len(path: &str) -> SyscallResult {
    if path.len() >= PATH_MAX {
        return Err(SysError::ENAMETOOLONG);
    }
    if path
        .split('/')
        .filter(|part| !part.is_empty())
        .any(|part| part.len() > NAME_MAX)
    {
        return Err(SysError::ENAMETOOLONG);
    }
    Ok(0)
}

#[derive(Clone, Copy)]
struct FsIdentity {
    euid: u32,
    egid: u32,
    umask: u32,
}

/// Snapshot the calling process's filesystem credentials without spinning
/// with interrupts disabled while another CPU owns the PCB lock. Callers must
/// use this before acquiring VFS/filesystem locks because retrying may yield.
fn current_fs_identity() -> FsIdentity {
    let process = current_process();
    loop {
        if let Some(inner) = process.try_inner_exclusive_access() {
            return FsIdentity {
                euid: inner.euid,
                egid: inner.egid,
                umask: inner.fs_context.lock().umask,
            };
        }
        crate::task::suspend_current_and_run_next();
    }
}

fn apply_new_inode_owner(
    inode: &Arc<dyn Inode>,
    parent: &Arc<dyn crate::fs::vfs::Dentry>,
    identity: FsIdentity,
) {
    inode.set_uid(identity.euid as usize);
    let parent_mode = parent.get_inode().map(|inode| inode.get_mode());
    if parent_mode.is_some_and(|mode| mode.contains(InodeMode::SET_GID)) {
        if let Some(parent_inode) = parent.get_inode() {
            inode.set_gid(parent_inode.get_gid());
        }
    } else {
        inode.set_gid(identity.egid as usize);
    }
}

fn validate_openat2_resolve(dirfd: isize, path: &str, how: &OpenHow) -> SyscallResult {
    let resolve = how.resolve;
    if resolve == 0 {
        return Ok(0);
    }

    if resolve & RESOLVE_NO_XDEV != 0 && path.starts_with("/proc") {
        return Err(SysError::EXDEV);
    }
    if resolve & RESOLVE_NO_MAGICLINKS != 0 && path == "/proc/self/exe" {
        return Err(SysError::ELOOP);
    }
    if resolve & RESOLVE_NO_SYMLINKS != 0 {
        let start = get_start_dentry(dirfd, path)?;
        if resolve_path_nofollow_last(start, path)
            .ok()
            .and_then(|dentry| dentry.get_inode())
            .is_some_and(|inode| inode.get_mode().contains(InodeMode::LINK))
        {
            return Err(SysError::ELOOP);
        }
    }
    if resolve & RESOLVE_BENEATH != 0
        && (path.starts_with('/') || path.split('/').any(|p| p == ".."))
    {
        return Err(SysError::EXDEV);
    }
    if resolve & RESOLVE_IN_ROOT != 0 && path.starts_with('/') {
        return Err(SysError::ENOENT);
    }

    Ok(0)
}

fn tmpfile_mode(
    parent: &Arc<dyn crate::fs::vfs::Dentry>,
    mode: u32,
    identity: FsIdentity,
) -> InodeMode {
    let euid = identity.euid as usize;
    let egid = identity.egid as usize;
    let parent_inode = parent.get_inode();
    let parent_has_setgid = parent_inode
        .as_ref()
        .is_some_and(|inode| inode.get_mode().contains(InodeMode::SET_GID));
    let file_gid = if parent_has_setgid {
        parent_inode
            .as_ref()
            .map(|inode| inode.get_gid())
            .unwrap_or(egid)
    } else {
        egid
    };
    let mut mode_bits = (mode & 0o7777) & !identity.umask;
    if mode_bits & InodeMode::SET_GID.bits() != 0 && euid != 0 && file_gid != egid {
        mode_bits &= !InodeMode::SET_GID.bits();
    }
    InodeMode::from_bits_truncate(mode_bits | InodeMode::FILE.bits())
}

fn alloc_tmpfile_fd(
    dir: Arc<dyn crate::fs::vfs::Dentry>,
    flags: OpenFlags,
    mode: u32,
    identity: FsIdentity,
) -> SyscallResult {
    let inode = dir.get_inode().ok_or(SysError::ENOENT)?;
    if inode.get_mode().get_type() != InodeMode::DIR {
        return Err(SysError::ENOTDIR);
    }
    check_readonly_mount(&dir.path())?;
    if !check_inode_perm_for_ids(&inode, identity.euid, identity.egid, 3) {
        return Err(SysError::EACCES);
    }

    let process = current_process();
    let file_mode = tmpfile_mode(&dir, mode, identity);
    let tmp_dentry = TempDentry::new(".tmpfile", Some(dir.clone()));
    let tmp_inode = Arc::new(TempInode::new(file_mode));
    tmp_inode.set_uid(identity.euid as usize);
    if dir
        .get_inode()
        .is_some_and(|parent_inode| parent_inode.get_mode().contains(InodeMode::SET_GID))
    {
        if let Some(parent_inode) = dir.get_inode() {
            tmp_inode.set_gid(parent_inode.get_gid());
        }
    } else {
        tmp_inode.set_gid(identity.egid as usize);
    }
    tmp_dentry.set_inode(tmp_inode);

    let (readable, writable) = flags.read_write();
    let cloexec = flags.contains(OpenFlags::O_CLOEXEC);
    let file = Arc::new(TempFile::new(
        readable,
        writable,
        flags.contains(OpenFlags::O_APPEND),
        tmp_dentry,
        flags,
    ));

    loop {
        let Some(mut inner) = process.try_inner_exclusive_access() else {
            crate::task::suspend_current_and_run_next();
            continue;
        };
        let fd = inner.alloc_fd()?;
        inner.fd_table[fd] = Some(file);
        if cloexec && fd < inner.fd_flags.len() {
            inner.fd_flags[fd] |= FD_CLOEXEC_FLAG;
        }
        return Ok(fd);
    }
}

fn fd_alias_number(path: &str) -> Option<Result<usize, SysError>> {
    let fd_str = path
        .strip_prefix("/proc/self/fd/")
        .or_else(|| path.strip_prefix("/dev/fd/"))?;
    if fd_str.is_empty() || fd_str.as_bytes().iter().any(|b| !b.is_ascii_digit()) {
        return Some(Err(SysError::ENOENT));
    }
    Some(fd_str.parse::<usize>().map_err(|_| SysError::ENOENT))
}

fn proc_self_fd_file(path: &str) -> Option<Arc<dyn File + Send + Sync>> {
    let fd = fd_alias_number(path)?.ok()?;
    let process = current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return None;
    }
    inner.fd_table[fd].clone()
}

fn open_fd_alias(source_fd: usize, flags: OpenFlags) -> SyscallResult {
    if flags.intersects(
        OpenFlags::O_CREAT
            | OpenFlags::O_EXCL
            | OpenFlags::O_TRUNC
            | OpenFlags::O_DIRECTORY
            | OpenFlags::O_TMPFILE,
    ) {
        return Err(SysError::EINVAL);
    }
    if flags.contains(OpenFlags::O_NOFOLLOW) && !flags.contains(OpenFlags::O_PATH) {
        return Err(SysError::ELOOP);
    }

    let process = current_process();
    let pid = process.getpid();
    let mut inner = process.inner_exclusive_access();
    let file = inner
        .fd_table
        .get(source_fd)
        .and_then(|file| file.as_ref())
        .cloned()
        .ok_or(SysError::ENOENT)?;
    let (read_requested, write_requested) = flags.read_write();
    if (read_requested && !file.readable()) || (write_requested && !file.writable()) {
        return Err(SysError::EACCES);
    }

    let new_fd = inner.alloc_fd()?;
    inner.fd_table[new_fd] = Some(file);
    if flags.contains(OpenFlags::O_CLOEXEC) && new_fd < inner.fd_flags.len() {
        inner.fd_flags[new_fd] |= FD_CLOEXEC_FLAG;
    }
    drop(inner);
    duplicate_fs_context(pid, source_fd, new_fd);
    Ok(new_fd)
}

fn materialize_tmpfile_link(
    parent: Arc<dyn crate::fs::vfs::Dentry>,
    name: &str,
    old_dentry: Arc<dyn crate::fs::vfs::Dentry>,
) -> SyscallResult {
    let old_inode = old_dentry.get_inode().ok_or(SysError::ENOENT)?;
    if old_inode.get_mode().get_type() != InodeMode::FILE {
        return Err(SysError::EINVAL);
    }

    let new_dentry = parent.create(name, old_inode.get_mode())?;
    let new_inode = new_dentry.get_inode().ok_or(SysError::EIO)?;
    new_inode.set_uid(old_inode.get_uid());
    new_inode.set_gid(old_inode.get_gid());
    new_inode.set_mode(old_inode.get_mode());
    new_inode.set_size(old_inode.get_size());
    let (atime_sec, atime_nsec) = old_inode.get_atime();
    let (mtime_sec, mtime_nsec) = old_inode.get_mtime();
    let (ctime_sec, ctime_nsec) = old_inode.get_ctime();
    new_inode.set_atime(atime_sec, atime_nsec);
    new_inode.set_mtime(mtime_sec, mtime_nsec);
    new_inode.set_ctime(ctime_sec, ctime_nsec);
    Ok(0)
}
fn check_path_name_lengths(path: &str) -> SyscallResult {
    if path.len() > PATH_MAX {
        return Err(SysError::ENAMETOOLONG);
    }
    if path
        .split('/')
        .filter(|name| !name.is_empty())
        .any(|name| name.len() > NAME_MAX)
    {
        return Err(SysError::ENAMETOOLONG);
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
    let process = current_process();
    let token = current_user_token();
    let path = process
        .inner_exclusive_access()
        .fs_context
        .lock()
        .cwd
        .clone()
        .path();
    let cstr = CString::new(path).expect("fail to convert CString");
    let bytes = cstr.as_bytes_with_nul();
    if len < bytes.len() {
        return Err(SysError::ERANGE);
    }
    if buf.is_null() || (buf as usize).checked_add(bytes.len()).is_none() {
        return Err(SysError::EFAULT);
    }

    let mut copied = 0usize;
    for user_buf in translated_byte_buffer(token, buf, bytes.len())? {
        let copy_len = user_buf.len().min(bytes.len() - copied);
        user_buf[..copy_len].copy_from_slice(&bytes[copied..copied + copy_len]);
        copied += copy_len;
        if copied == bytes.len() {
            break;
        }
    }
    Ok(bytes.len())
}

///create a directory with the path, the path is the name of the directory
pub fn sys_mkdirat(dirfd: isize, path: *const u8, mode: u32) -> SyscallResult {
    let token = current_user_token();
    let path = translated_str(token, path)?;
    let identity = current_fs_identity();
    info!("[DEBUG sys_mkdirat] dirfd={} path={}", dirfd, path);
    let start_dentry = match get_start_dentry(dirfd, &path) {
        Ok(dentry) => dentry,
        Err(e) => {
            info!("[DEBUG sys_mkdirat] get_start_dentry failed: {:?}", e);
            return Err(e);
        }
    };
    info!(
        "[DEBUG sys_mkdirat] start_dentry path={}",
        start_dentry.path()
    );
    let (parent_path, dir_name) = split_parent_and_name(&path);
    info!(
        "[DEBUG sys_mkdirat] parent_path={} dir_name={}",
        parent_path, dir_name
    );
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
    landlock_check_dentry(&parent, LANDLOCK_ACCESS_FS_MAKE_DIR)?;
    let mut mode_bits = (mode & 0o7777) & !identity.umask | InodeMode::DIR.bits();
    if parent
        .get_inode()
        .is_some_and(|inode| inode.get_mode().contains(InodeMode::SET_GID))
    {
        mode_bits |= InodeMode::SET_GID.bits();
    }
    let effective_mode = InodeMode::from_bits_truncate(mode_bits);
    check_readonly_mount(&parent.path())?;
    match parent.create(dir_name.as_str(), effective_mode) {
        Ok(new_dir) => {
            if let Some(inode) = new_dir.get_inode() {
                apply_new_inode_owner(&inode, &parent, identity);
            }
            let new_path = if parent.path() == "/" {
                format!("/{}", dir_name)
            } else {
                format!("{}/{}", parent.path(), dir_name)
            };
            inotify_notify_dentry(new_dir.clone(), IN_CREATE | IN_ISDIR);
            fanotify_notify_dentry(new_dir.clone(), FAN_CREATE);
            GLOBAL_DCACHE.insert(new_path, new_dir);
            info!("[DEBUG sys_mkdirat] success");
            Ok(0)
        }
        Err(e) => {
            info!("[DEBUG sys_mkdirat] create failed: {:?}", e);
            Err(e)
        }
    }
}

/// Create a special file (device node, fifo, or socket).
pub fn sys_mknodat(dirfd: isize, path: *const u8, mode: u32, _dev: u32) -> SyscallResult {
    let token = current_user_token();
    let path = translated_str(token, path)?;
    let identity = current_fs_identity();
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
    let landlock_access = match mode & InodeMode::TYPE_MASK.bits() {
        bits if bits == InodeMode::CHAR.bits() => LANDLOCK_ACCESS_FS_MAKE_CHAR,
        bits if bits == InodeMode::BLOCK.bits() => LANDLOCK_ACCESS_FS_MAKE_BLOCK,
        bits if bits == InodeMode::FIFO.bits() => LANDLOCK_ACCESS_FS_MAKE_FIFO,
        bits if bits == InodeMode::SOCKET.bits() => LANDLOCK_ACCESS_FS_MAKE_SOCK,
        _ => LANDLOCK_ACCESS_FS_MAKE_REG,
    };
    landlock_check_dentry(&parent, landlock_access)?;
    let file_type = match mode & InodeMode::TYPE_MASK.bits() {
        0 => InodeMode::FILE.bits(),
        file_type => file_type,
    };
    let mut perm = (mode & 0o7777) & !identity.umask;
    if parent
        .get_inode()
        .is_some_and(|inode| inode.get_mode().contains(InodeMode::SET_GID))
    {
        perm |= InodeMode::SET_GID.bits();
    }
    let effective_mode = InodeMode::from_bits_truncate(file_type | perm);
    check_readonly_mount(&parent.path())?;
    let ret = if effective_mode.get_type() == InodeMode::FILE {
        parent.create(name.as_str(), effective_mode).map(|_| 0)
    } else {
        parent.mknod(name.as_str(), effective_mode, _dev)
    };
    match ret {
        Ok(0) => {
            let new_path = if parent.path() == "/" {
                format!("/{}", name)
            } else {
                format!("{}/{}", parent.path(), name)
            };
            if let Ok(target) = parent.find(name.as_str()) {
                if let Some(inode) = target.get_inode() {
                    apply_new_inode_owner(&inode, &parent, identity);
                }
            }
            if let Ok(target) = parent.find(name.as_str()) {
                inotify_notify_dentry(target.clone(), IN_CREATE);
                fanotify_notify_dentry(target, FAN_CREATE);
            } else {
                inotify_notify_path(&new_path, IN_CREATE);
                fanotify_notify_path(&new_path, FAN_CREATE);
            }
            Ok(0)
        }
        Ok(ret) => Ok(ret),
        Err(err) => Err(err),
    }
}

///
pub fn sys_unlinkat(dirfd: isize, path: *const u8, flags: u32) -> SyscallResult {
    let token = current_user_token();
    let path = translated_str(token, path)?;
    set_current_syscall_stage(1);
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
    set_current_syscall_stage(2);
    if name == "." || name == ".." {
        return Err(SysError::EINVAL);
    }
    let target = parent.find(name.as_str())?;
    set_current_syscall_stage(3);
    let is_dir = target
        .get_inode()
        .is_some_and(|inode| inode.get_mode().get_type() == InodeMode::DIR);
    landlock_check_dentry(
        &target,
        if is_dir {
            LANDLOCK_ACCESS_FS_REMOVE_DIR
        } else {
            LANDLOCK_ACCESS_FS_REMOVE_FILE
        },
    )?;
    let nlink_before = target
        .get_inode()
        .map(|inode| inode.get_nlink())
        .unwrap_or(1);
    set_current_syscall_stage(4);
    match parent.unlink(name.as_str(), flags) {
        Ok(0) => {
            set_current_syscall_stage(5);
            let removed = is_dir || nlink_before <= 1;
            if removed && !is_dir {
                drop_unlinked_file_cache_if_unreferenced(&target);
            }
            set_current_syscall_stage(6);
            inotify_notify_delete_dentry(target.clone(), removed);
            set_current_syscall_stage(7);
            fanotify_notify_delete_dentry(target);
            set_current_syscall_stage(8);
            Ok(0)
        }
        Ok(ret) => Ok(ret),
        Err(err) => Err(err),
    }
}

#[inline]
fn set_current_syscall_stage(stage: usize) {
    if let Some(task) = current_task() {
        task.set_active_syscall_stage(stage);
    }
}

fn drop_unlinked_file_cache_if_unreferenced(target: &Arc<dyn crate::fs::vfs::Dentry>) {
    let Some(inode) = target.get_inode() else {
        return;
    };
    let Some(cache_inode_id) = inode.cache_inode_id() else {
        return;
    };
    let (_discarded, kept_queued) = crate::fs::writeback::discard_closed_inode(cache_inode_id);
    if kept_queued == 0 && Arc::strong_count(target) == 1 {
        crate::fs::page::pagecache::PAGE_CACHE.remove_inode_pages(cache_inode_id);
        inode.clear_punched_holes();
    }
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
    let proc_fd_file = proc_self_fd_file(&old_path);
    let old_dentry = match proc_fd_file.as_ref() {
        Some(file) => file.get_dentry(),
        None => match resolve_path(old_start_dentry, &old_path) {
            Ok(dentry) => dentry,
            Err(_) => return Err(SysError::ENOENT),
        },
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
    if proc_fd_file.as_ref().is_some_and(|file| file.is_tmpfile()) {
        return materialize_tmpfile_link(new_parent, &new_name, old_dentry);
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
    if flags != 0 {
        return Err(SysError::EINVAL);
    }

    let token = current_user_token();
    let old_path = translated_str(token, oldpath)?;
    let new_path = translated_str(token, newpath)?;
    check_path_name_lengths(&old_path)?;
    check_path_name_lengths(&new_path)?;
    set_current_syscall_stage(20);

    let old_start_dentry = match get_start_dentry(olddirfd, &old_path) {
        Ok(dentry) => dentry,
        Err(e) => return Err(e),
    };
    let (old_parent_path, old_name) = split_parent_and_name(&old_path);
    if old_name.is_empty() || old_name == "." || old_name == ".." {
        return Err(SysError::EINVAL);
    }
    let old_parent = if old_parent_path == "." || old_parent_path == "/" {
        old_start_dentry
    } else {
        match resolve_path(old_start_dentry, &old_parent_path) {
            Ok(dentry) => dentry,
            Err(_) => return Err(SysError::ENOENT),
        }
    };
    let old_parent_inode = old_parent.get_inode().ok_or(SysError::ENOENT)?;
    if !old_parent_inode.get_mode().contains(InodeMode::DIR) {
        return Err(SysError::ENOTDIR);
    }
    if !check_inode_perm_effective(&old_parent_inode, 3) {
        return Err(SysError::EACCES);
    }
    let old_dentry = match old_parent.find(&old_name) {
        Ok(dentry) => dentry,
        Err(_) => return Err(SysError::ENOENT),
    };
    set_current_syscall_stage(21);
    let old_is_dir = old_dentry
        .get_inode()
        .is_some_and(|inode| inode.get_mode().contains(InodeMode::DIR));
    let old_abs = old_dentry.path();

    let new_start_dentry = match get_start_dentry(newdirfd, &new_path) {
        Ok(dentry) => dentry,
        Err(e) => return Err(e),
    };
    let (new_parent_path, new_name) = split_parent_and_name(&new_path);
    if new_name.is_empty() || new_name == "." || new_name == ".." {
        return Err(SysError::EINVAL);
    }
    let new_parent = if new_parent_path == "." || new_parent_path == "/" {
        new_start_dentry
    } else {
        match resolve_path(new_start_dentry, &new_parent_path) {
            Ok(dentry) => dentry,
            Err(_) => return Err(SysError::ENOENT),
        }
    };
    set_current_syscall_stage(22);
    let new_parent_inode = new_parent.get_inode().ok_or(SysError::ENOENT)?;
    if !new_parent_inode.get_mode().contains(InodeMode::DIR) {
        return Err(SysError::ENOTDIR);
    }
    if !Arc::ptr_eq(&old_parent, &new_parent) && !check_inode_perm_effective(&new_parent_inode, 3) {
        return Err(SysError::EACCES);
    }
    let new_abs = if new_parent.path() == "/" {
        format!("/{}", new_name)
    } else {
        format!("{}/{}", new_parent.path(), new_name)
    };
    landlock_check_dentry(&old_dentry, LANDLOCK_ACCESS_FS_REFER)?;
    landlock_check_path(&new_abs, LANDLOCK_ACCESS_FS_REFER).map_err(|err| {
        if err == SysError::EACCES {
            SysError::EXDEV
        } else {
            err
        }
    })?;

    let old_sb = find_superblock_by_path(&old_abs).ok_or(SysError::ENOENT)?;
    let new_sb = find_superblock_by_path(&new_parent.path()).ok_or(SysError::ENOENT)?;
    if !Arc::ptr_eq(&old_sb, &new_sb) {
        return Err(SysError::EXDEV);
    }
    if old_sb.inner().is_readonly() {
        return Err(SysError::EROFS);
    }

    set_current_syscall_stage(23);
    match old_parent.rename(&old_name, new_parent, &new_name) {
        Ok(_) => {
            set_current_syscall_stage(24);
            inotify_notify_move_dentry(&old_abs, &new_abs, Some(old_dentry.clone()), old_is_dir);
            set_current_syscall_stage(25);
            fanotify_notify_move(&old_abs, &new_abs, Some(old_dentry), old_is_dir);
            set_current_syscall_stage(26);
            Ok(0)
        }
        Err(code) => {
            if code == SysError::EIO {
                error!(
                    "[FILE_IO_EIO] op=renameat2 pid={} old={} new={} error={:?} writeback_pending={:?} ext4_flush={:?} block_io={:?}",
                    current_process().getpid(),
                    old_abs,
                    new_abs,
                    code,
                    crate::fs::writeback::try_pending_count(),
                    crate::fs::lwext4::file::ext4_flush_stats(),
                    crate::drivers::block::virtio_blk::virtio_block_io_stats(),
                );
            }
            Err(code)
        }
    }
}

///
pub fn sys_chdir(path: *const u8) -> SyscallResult {
    let process = current_process();
    let token = current_user_token();
    let path = translated_str(token, path)?;
    let inner = process.inner_exclusive_access();
    let cwd = inner.fs_context.lock().cwd.clone();
    info!("[sys_chdir] path={} cwd={}", path, cwd.name());
    let target_dentry = match resolve_path(cwd, &path) {
        Ok(dentry) => dentry,
        Err(err) => {
            info!("[sys_chdir] resolve_path failed for {}: {:?}", path, err);
            return Err(err);
        }
    };

    let inode = target_dentry.get_inode().ok_or(SysError::ENOENT)?;
    let mode = inode.get_mode();
    info!(
        "[sys_chdir] resolved to {} mode={:?}",
        target_dentry.name(),
        mode
    );
    if mode.get_type() != InodeMode::DIR {
        return Err(SysError::ENOTDIR);
    }
    if !check_inode_perm_for_ids(&inode, inner.euid, inner.egid, 1) {
        return Err(SysError::EACCES);
    }
    inner.fs_context.lock().cwd = target_dentry;
    Ok(0)
}

pub fn sys_fchdir(fd: usize) -> SyscallResult {
    let process = current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return Err(SysError::EBADF);
    }
    let file = inner.fd_table[fd].as_ref().ok_or(SysError::EBADF)?.clone();
    let target_dentry = file.get_dentry();
    let inode = target_dentry.get_inode().ok_or(SysError::ENOENT)?;
    if inode.get_mode().get_type() != InodeMode::DIR {
        return Err(SysError::ENOTDIR);
    }
    if !check_inode_perm_for_ids(&inode, inner.euid, inner.egid, 1) {
        return Err(SysError::EACCES);
    }
    inner.fs_context.lock().cwd = target_dentry;
    Ok(0)
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

    let (now_sec, now_nsec) = realtime_timespec();
    inode.set_ctime(now_sec, now_nsec);

    notify_attrib(&NotifyTarget::new(target));
    Ok(0)
}

pub fn sys_fchownat(
    dirfd: isize,
    path: *const u8,
    owner: u32,
    group: u32,
    _flags: i32,
) -> SyscallResult {
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

    check_readonly_mount(&target.path())?;
    apply_chown(&inode, owner, group)?;
    notify_attrib(&NotifyTarget::new(target));
    Ok(0)
}

fn apply_chown(inode: &Arc<dyn Inode>, owner: u32, group: u32) -> SyscallResult {
    const ID_UNCHANGED: u32 = u32::MAX;

    let old_uid = inode.get_uid() as u32;
    let old_gid = inode.get_gid() as u32;
    let process = current_process();
    let inner = process.inner_exclusive_access();
    let euid = inner.euid;
    let egid = inner.egid;
    drop(inner);

    // Kairix does not yet track supplementary groups or CAP_CHOWN separately.
    // Model CAP_CHOWN with effective UID 0; an unprivileged file owner may
    // retain the owner and select its effective group, matching Linux's
    // restricted_chown behavior for the credentials currently represented.
    if euid != 0 {
        if euid != old_uid
            || (owner != ID_UNCHANGED && owner != old_uid)
            || (group != ID_UNCHANGED && group != old_gid && group != egid)
        {
            return Err(SysError::EPERM);
        }
    }

    let new_uid = if owner == ID_UNCHANGED {
        old_uid
    } else {
        owner
    };
    let new_gid = if group == ID_UNCHANGED {
        old_gid
    } else {
        group
    };
    let ownership_changed = new_uid != old_uid || new_gid != old_gid;

    if new_uid != old_uid {
        inode.set_uid(new_uid as usize);
    }
    if new_gid != old_gid {
        inode.set_gid(new_gid as usize);
    }

    // Linux clears privilege-on-exec bits when ownership of an executable
    // regular file changes. A non-group-executable setgid bit denotes
    // mandatory locking and is therefore retained.
    if ownership_changed && inode.get_mode().get_type() == InodeMode::FILE {
        let mut mode = inode.get_mode();
        mode.remove(InodeMode::SET_UID);
        if mode.bits() & 0o010 != 0 {
            mode.remove(InodeMode::SET_GID);
        }
        inode.set_mode(mode);
    }

    let (now_sec, now_nsec) = realtime_timespec();
    inode.set_ctime(now_sec, now_nsec);
    Ok(0)
}

pub fn sys_fchown(fd: usize, owner: u32, group: u32) -> SyscallResult {
    let process = current_process();
    let inner = process.inner_exclusive_access();
    let file = inner
        .fd_table
        .get(fd)
        .and_then(|file| file.as_ref())
        .cloned()
        .ok_or(SysError::EBADF)?;
    let notify_target = notify_target_for_file_if_needed(&file);
    drop(inner);

    let inode = file.get_inode().ok_or(SysError::ENOENT)?;
    check_readonly_mount(&file.get_dentry().path())?;
    apply_chown(&inode, owner, group)?;
    if let Some(target) = notify_target.as_ref() {
        notify_attrib(target);
    }
    Ok(0)
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
            copy_to_user(token, buf, &bytes[..len])?;
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
    landlock_check_dentry(&parent, LANDLOCK_ACCESS_FS_MAKE_SYM)?;

    match parent.symlink(name.as_str(), target_str.as_str()) {
        Ok(0) => {
            let new_path = if parent.path() == "/" {
                format!("/{}", name)
            } else {
                format!("{}/{}", parent.path(), name)
            };
            if let Ok(target) = parent.find(name.as_str()) {
                inotify_notify_dentry(target.clone(), IN_CREATE);
                fanotify_notify_dentry(target, FAN_CREATE);
            } else {
                inotify_notify_path(&new_path, IN_CREATE);
                fanotify_notify_path(&new_path, FAN_CREATE);
            }
            Ok(0)
        }
        Ok(ret) => Ok(ret),
        Err(err) => Err(err),
    }
}

pub fn sys_utimensat(
    dirfd: isize,
    path: *const u8,
    times: *const Timespec,
    _flags: i32,
) -> SyscallResult {
    let token = current_user_token();
    let (inode, notify_target): (
        alloc::sync::Arc<dyn crate::fs::vfs::inode::Inode>,
        Option<NotifyTarget>,
    ) = if path.is_null() {
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
        let file = inner.fd_table[fd].as_ref().unwrap().clone();
        match file.get_inode() {
            Some(inode) => (inode, notify_target_for_file_if_needed(&file)),
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
            Some(inode) => (inode, Some(NotifyTarget::new(target))),
            None => return Err(SysError::ENOENT),
        }
    };

    let (now_sec, now_nsec) = realtime_timespec();

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
    if let Some(target) = notify_target.as_ref() {
        notify_attrib(target);
    }
    Ok(0)
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
        if (mode & 4) != 0 && (perm & 0o400) == 0 {
            return false;
        }
        if (mode & 2) != 0 && (perm & 0o200) == 0 {
            return false;
        }
        if (mode & 1) != 0 && (perm & 0o100) == 0 {
            return false;
        }
    } else if gid == file_gid {
        if (mode & 4) != 0 && (perm & 0o040) == 0 {
            return false;
        }
        if (mode & 2) != 0 && (perm & 0o020) == 0 {
            return false;
        }
        if (mode & 1) != 0 && (perm & 0o010) == 0 {
            return false;
        }
    } else {
        if (mode & 4) != 0 && (perm & 0o004) == 0 {
            return false;
        }
        if (mode & 2) != 0 && (perm & 0o002) == 0 {
            return false;
        }
        if (mode & 1) != 0 && (perm & 0o001) == 0 {
            return false;
        }
    }
    true
}

fn check_inode_perm_for_ids(
    inode: &Arc<dyn crate::fs::vfs::inode::Inode>,
    uid: u32,
    gid: u32,
    mode: u32,
) -> bool {
    let file_mode = inode.get_mode();
    let file_uid = inode.get_uid() as u32;
    let file_gid = inode.get_gid() as u32;
    let perm = file_mode.bits() & 0o777;

    if uid == 0 {
        if (mode & 1) != 0 {
            let is_dir = file_mode.contains(crate::fs::vfs::inode::InodeMode::DIR);
            let has_exec = (perm & 0o111) != 0;
            return is_dir || has_exec;
        }
        return true;
    }

    let allowed = if uid == file_uid {
        (perm >> 6) & 0o7
    } else if gid == file_gid {
        (perm >> 3) & 0o7
    } else {
        perm & 0o7
    };
    (allowed & mode) == mode
}

pub(super) fn check_inode_perm_effective(
    inode: &Arc<dyn crate::fs::vfs::inode::Inode>,
    mode: u32,
) -> bool {
    let process = current_process();
    let inner = process.inner_exclusive_access();
    let uid = inner.euid;
    let gid = inner.egid;
    drop(inner);
    check_inode_perm_for_ids(inode, uid, gid, mode)
}

fn check_dir_search_perm_for_ids(
    dentry: &Arc<dyn crate::fs::vfs::Dentry>,
    uid: u32,
    gid: u32,
) -> SysResult<()> {
    let inode = dentry.get_inode().ok_or(SysError::ENOTDIR)?;
    let inode_mode = inode.get_mode();
    if !inode_mode.contains(InodeMode::DIR) {
        return Err(SysError::ENOTDIR);
    }
    let path = dentry.path();
    if inode_mode.bits() & 0o777 == 0
        && (path == "/proc"
            || path.starts_with("/proc/")
            || path == "/sys"
            || path.starts_with("/sys/"))
    {
        return Ok(());
    }
    if !check_inode_perm_for_ids(&inode, uid, gid, 1) {
        return Err(SysError::EACCES);
    }
    Ok(())
}

fn check_access_path_prefix_perm(
    start_dentry: Arc<dyn crate::fs::vfs::Dentry>,
    path: &str,
    follow_last: bool,
    uid: u32,
    gid: u32,
) -> SysResult<()> {
    const MAX_SYMLINK_FOLLOWS: usize = 40;

    let mut current = if path.starts_with('/') {
        GLOBAL_DCACHE.get("/").unwrap().clone()
    } else {
        start_dentry
    };
    let mut parts: Vec<String> = path
        .split('/')
        .filter(|part| !part.is_empty())
        .map(|part| part.to_string())
        .collect();
    let mut i = 0;
    let mut symlink_count = 0;

    while i < parts.len() {
        let part = parts[i].clone();
        let is_last = i == parts.len() - 1;

        match part.as_str() {
            "." => {
                i += 1;
            }
            ".." => {
                check_dir_search_perm_for_ids(&current, uid, gid)?;
                current = current.parent().unwrap_or(current);
                i += 1;
            }
            name => {
                check_dir_search_perm_for_ids(&current, uid, gid)?;
                let next_dentry = current.find(name)?;

                if let Some(inode) = next_dentry.get_inode() {
                    if inode.get_mode().contains(InodeMode::LINK) {
                        if is_last && !follow_last {
                            return Ok(());
                        }
                        if symlink_count >= MAX_SYMLINK_FOLLOWS {
                            return Err(SysError::ELOOP);
                        }
                        symlink_count += 1;

                        let target = inode.readlink().map_err(|e| {
                            let code = if e < 0 { e } else { -e };
                            SysError::try_from(code).unwrap_or(SysError::EINVAL)
                        })?;
                        let remaining = parts[i + 1..].join("/");
                        let new_path = if remaining.is_empty() {
                            target
                        } else if target.ends_with('/') {
                            format!("{}{}", target, remaining)
                        } else {
                            format!("{}/{}", target, remaining)
                        };

                        if new_path.starts_with('/') {
                            current = GLOBAL_DCACHE.get("/").unwrap().clone();
                        }
                        parts = new_path
                            .split('/')
                            .filter(|part| !part.is_empty())
                            .map(|part| part.to_string())
                            .collect();
                        i = 0;
                        continue;
                    }
                }

                current = next_dentry;
                i += 1;
            }
        }
    }

    Ok(())
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
    const AT_EACCESS: u32 = 0x200;
    const VALID_FLAGS: u32 = AT_EMPTY_PATH | AT_SYMLINK_NOFOLLOW | AT_EACCESS;

    if flags & !VALID_FLAGS != 0 {
        return Err(SysError::EINVAL);
    }

    if raw_path.len() > PATH_MAX {
        return Err(SysError::ENAMETOOLONG);
    }

    let target = if raw_path.is_empty() {
        if flags & AT_EMPTY_PATH == 0 {
            return Err(SysError::ENOENT);
        }
        let process = current_process();
        let inner = process.inner_exclusive_access();
        if dirfd == crate::fs::vfs::path::AT_FDCWD {
            inner.fs_context.lock().cwd.clone()
        } else {
            let fd = usize::try_from(dirfd).map_err(|_| SysError::EBADF)?;
            let file = inner
                .fd_table
                .get(fd)
                .and_then(|file| file.as_ref())
                .ok_or(SysError::EBADF)?;
            if file.get_inode().is_none() {
                return Err(SysError::EBADF);
            }
            file.get_dentry()
        }
    } else {
        let start_dentry = get_start_dentry(dirfd, &raw_path)?;
        let follow_last = flags & AT_SYMLINK_NOFOLLOW == 0;
        let process = current_process();
        let inner = process.inner_exclusive_access();
        let (check_uid, check_gid) = if flags & AT_EACCESS != 0 {
            (inner.euid, inner.egid)
        } else {
            (inner.uid, inner.gid)
        };
        drop(inner);
        check_access_path_prefix_perm(
            start_dentry.clone(),
            &raw_path,
            follow_last,
            check_uid,
            check_gid,
        )?;
        if follow_last {
            resolve_path(start_dentry, &raw_path)?
        } else {
            resolve_path_nofollow_last(start_dentry, &raw_path)?
        }
    };
    let inode = target.get_inode().ok_or(SysError::ENOENT)?;

    if (mode & 2) != 0 && check_readonly_mount(&target.path()).is_err() {
        return Err(SysError::EROFS);
    }

    let allowed = if flags & AT_EACCESS != 0 {
        check_inode_perm_effective(&inode, mode)
    } else {
        check_inode_perm(&inode, mode)
    };
    if allowed {
        Ok(0)
    } else {
        Err(SysError::EACCES)
    }
}

/// memfd_create - 创建一个匿名的内存文件描述符
/// 参考 Linux 实现，创建一个在临时文件系统中的匿名文件
pub fn sys_memfd_create(name: *const u8, _flags: u32) -> SyscallResult {
    const MFD_ALLOW_SEALING: u32 = 0x0002;
    let file_flags = OpenFlags::from_bits_truncate(_flags);

    let process = current_process();
    let token = current_user_token();

    // 解析名称（可选，可以为空）
    let name_str = if name.is_null() {
        String::from("memfd")
    } else {
        match translated_str(token, name) {
            Ok(s) => s,
            Err(_) => String::from("memfd"),
        }
    };

    // 生成唯一的文件名（使用进程ID和时间戳）
    let pid = process.getpid();
    let timestamp = polyhal::timer::current_time().as_micros();
    let unique_name = format!("memfd-{}-{}-{}", pid, timestamp, name_str);

    // 在 /dev/shm 中创建临时文件（因为它已经是 tmpfs）
    let shm_dentry = match GLOBAL_DCACHE.get("/dev/shm") {
        Some(d) => d.clone(),
        None => {
            error!("memfd_create: /dev/shm not found");
            return Err(SysError::ENOENT);
        }
    };

    // 创建文件 inode 和 dentry
    let file_mode = InodeMode::FILE | InodeMode::from_bits_truncate(0o600);
    let new_dentry = TempDentry::new(unique_name.as_str(), Some(shm_dentry.clone()));
    let child_inode = Arc::new(TempInode::new(file_mode));
    if (_flags & MFD_ALLOW_SEALING) == 0 {
        child_inode.set_seals(F_SEAL_SEAL).unwrap();
    }
    new_dentry.set_inode(child_inode);

    // 添加到父目录
    {
        let mut children = shm_dentry.get_dentryinner().children.lock();
        children.insert(unique_name.clone(), new_dentry.clone());
    }

    // 更新 dcache
    let target_path = format!("/dev/shm/{}", unique_name);
    GLOBAL_DCACHE.insert(target_path, new_dentry.clone());

    // 创建文件对象
    let file = Arc::new(TempFile::new(true, true, false, new_dentry, file_flags));

    // 分配文件描述符
    let mut inner = process.inner_exclusive_access();
    let fd = inner.alloc_fd()?;
    inner.fd_table[fd] = Some(file);

    Ok(fd)
}

pub fn sys_fchmod(fd: usize, mode: u32) -> SyscallResult {
    let process = current_process();
    let inner = process.inner_exclusive_access();

    // 检查文件描述符有效性
    if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
        return Err(SysError::EBADF);
    }

    let file = inner.fd_table[fd].as_ref().unwrap().clone();
    let notify_target = notify_target_for_file_if_needed(&file);
    drop(inner);

    // 获取文件的 inode
    let inode = match file.get_inode() {
        Some(inode) => inode,
        None => return Err(SysError::ENOENT),
    };

    // 修改文件权限（保留类型位，只修改权限位）
    let old_mode = inode.get_mode();
    let new_mode = InodeMode::from_bits_truncate(
        (old_mode.bits() & InodeMode::TYPE_MASK.bits()) | (mode & 0o7777),
    );
    inode.set_mode(new_mode);

    // 更新修改时间
    let (now_sec, now_nsec) = realtime_timespec();
    inode.set_ctime(now_sec, now_nsec);

    if let Some(target) = notify_target.as_ref() {
        notify_attrib(target);
    }
    Ok(0)
}

///
pub fn sys_openat(dirfd: isize, path: *const u8, flags: u32, mode: u32) -> SyscallResult {
    let process = current_process();
    let token = current_user_token();
    let raw_path = translated_str(token, path)?;
    check_open_path_len(&raw_path)?;
    let safe_flags = OpenFlags::from_bits_truncate(flags);
    if let Some(source_fd) = fd_alias_number(&raw_path) {
        return open_fd_alias(source_fd?, safe_flags);
    }
    // Take one coherent credential snapshot before path resolution or any
    // filesystem operation. If munmap currently owns the PCB while waiting
    // for a remote TLB acknowledgement, the retry path yields with IRQs
    // restored instead of becoming a SpinNoIrq waiter.
    let identity = current_fs_identity();
    let has_cloexec = safe_flags.contains(OpenFlags::O_CLOEXEC);
    let has_noatime = safe_flags.contains(OpenFlags::O_NOATIME);
    let has_tmpfile = safe_flags.contains(OpenFlags::O_TMPFILE);
    let has_trunc = safe_flags.contains(OpenFlags::O_TRUNC);
    let write_requested = safe_flags.writable()
        || safe_flags.contains(OpenFlags::O_CREAT)
        || has_trunc
        || has_tmpfile;

    let start_dentry = match get_start_dentry(dirfd, &raw_path) {
        Ok(dentry) => dentry,
        Err(e) => return Err(e),
    };
    if has_tmpfile {
        if !safe_flags.writable() {
            return Err(SysError::EINVAL);
        }
        let dir = resolve_path(start_dentry, &raw_path)?;
        return alloc_tmpfile_fd(dir, safe_flags, mode, identity);
    }
    let create_requested = safe_flags.contains(OpenFlags::O_CREAT);
    let parent_for_create = if create_requested {
        let (parent_path, name) = split_parent_and_name(&raw_path);
        if name.is_empty() {
            None
        } else if parent_path == "." || parent_path == "/" {
            Some(start_dentry.clone())
        } else {
            Some(resolve_path(start_dentry.clone(), &parent_path)?)
        }
    } else {
        None
    };
    let effective_mode = if create_requested {
        InodeMode::from_bits_truncate((mode & 0o7777) & !identity.umask | InodeMode::FILE.bits())
    } else {
        InodeMode::FILE
    };
    // Resolve the final component once.  The previous code discarded lookup
    // errors with `.ok()` and retried the same path up to three times.  A
    // normal failed shared-library probe therefore repeated the filesystem
    // lookup (and, on ext4, mount-gate acquisition) for no semantic benefit.
    let nofollow_lookup = if create_requested {
        let (_parent_path, name) = split_parent_and_name(&raw_path);
        match parent_for_create.as_ref() {
            Some(parent) if !name.is_empty() => parent.find(&name),
            _ => resolve_path_nofollow_last(start_dentry.clone(), &raw_path),
        }
    } else {
        resolve_path_nofollow_last(start_dentry.clone(), &raw_path)
    };
    let target_lookup = match nofollow_lookup {
        Ok(nofollow_target) => {
            let is_symlink = dentry_is_symlink(&nofollow_target);
            if is_symlink {
                check_nosymfollow_mount(&nofollow_target.path(), &nofollow_target)?;
            }
            if is_symlink
                && !safe_flags.contains(OpenFlags::O_NOFOLLOW)
                && !(create_requested && safe_flags.contains(OpenFlags::O_EXCL))
            {
                resolve_path(start_dentry.clone(), &raw_path)
            } else {
                Ok(nofollow_target)
            }
        }
        Err(err) => Err(err),
    };
    if create_requested && safe_flags.contains(OpenFlags::O_EXCL) && target_lookup.is_ok() {
        return Err(SysError::EEXIST);
    }
    let (target_for_checks, target_lookup_error) = match target_lookup {
        Ok(target) => (Some(target), None),
        Err(err) => (None, Some(err)),
    };
    let new_file_parent = if create_requested {
        let (_parent_path, name) = split_parent_and_name(&raw_path);
        if name.is_empty()
            || target_for_checks.is_some()
            || target_lookup_error != Some(SysError::ENOENT)
        {
            None
        } else {
            parent_for_create.clone()
        }
    } else {
        None
    };
    if let Some(parent) = new_file_parent.as_ref() {
        check_readonly_mount(&parent.path())?;
    } else if write_requested {
        if let Some(target) = target_for_checks.as_ref() {
            check_readonly_mount(&target.path())?;
        }
    }
    if let Some(target) = target_for_checks.as_ref() {
        let inode = target.get_inode().ok_or(SysError::EIO)?;
        let mode = inode.get_mode();
        let file_type = mode.get_type();
        if safe_flags.contains(OpenFlags::O_NOFOLLOW) && file_type == InodeMode::LINK {
            return Err(SysError::ELOOP);
        }
        if safe_flags.contains(OpenFlags::O_DIRECTORY) && file_type != InodeMode::DIR {
            return Err(SysError::ENOTDIR);
        }
        if write_requested && file_type == InodeMode::DIR {
            return Err(SysError::EISDIR);
        }
        if safe_flags.contains(OpenFlags::O_NONBLOCK)
            && safe_flags.read_write() == (false, true)
            && file_type == InodeMode::FIFO
        {
            return Err(SysError::ENXIO);
        }
        let requested_perm = match safe_flags.read_write() {
            (true, true) => 4 | 2,
            (false, true) => 2,
            _ => 4,
        };
        if !check_inode_perm_for_ids(&inode, identity.euid, identity.egid, requested_perm) {
            return Err(SysError::EACCES);
        }
        let mut landlock_access = 0;
        if safe_flags.read_write().0 {
            landlock_access |= if file_type == InodeMode::DIR {
                LANDLOCK_ACCESS_FS_READ_DIR
            } else {
                LANDLOCK_ACCESS_FS_READ_FILE
            };
        }
        if safe_flags.writable() {
            landlock_access |= LANDLOCK_ACCESS_FS_WRITE_FILE;
        }
        if has_trunc {
            landlock_access |= LANDLOCK_ACCESS_FS_TRUNCATE;
        }
        landlock_check_dentry(target, landlock_access)?;
    }
    if target_for_checks.is_none() && has_trunc {
        let (_parent_path, name) = split_parent_and_name(&raw_path);
        if let Some(parent) = parent_for_create.as_ref() {
            let new_path = if parent.path() == "/" {
                format!("/{}", name)
            } else {
                format!("{}/{}", parent.path(), name)
            };
            landlock_check_path(&new_path, LANDLOCK_ACCESS_FS_TRUNCATE)?;
        }
    }
    if let Some(parent) = new_file_parent.as_ref() {
        landlock_check_dentry(parent, LANDLOCK_ACCESS_FS_MAKE_REG)?;
    }
    let created_path = new_file_parent.as_ref().map(|parent| {
        let (_parent_path, name) = split_parent_and_name(&raw_path);
        if parent.path() == "/" {
            format!("/{}", name)
        } else {
            format!("{}/{}", parent.path(), name)
        }
    });
    if has_noatime {
        if let Some(target) = target_for_checks.as_ref() {
            let inode = target.get_inode().ok_or(SysError::EIO)?;
            let owner_uid = inode.get_uid() as u32;
            if identity.euid != 0 && identity.euid != owner_uid {
                return Err(SysError::EPERM);
            }
        }
    }
    let open_result = if let Some(parent) = new_file_parent.clone() {
        let (_parent_path, name) = split_parent_and_name(&raw_path);
        create_file_at(
            parent,
            name.as_str(),
            OpenFlags::from_bits_truncate(flags),
            effective_mode,
        )
    } else if let Some(target) = target_for_checks.as_ref() {
        open_resolved_file(target.clone(), OpenFlags::from_bits_truncate(flags))
    } else {
        Err(target_lookup_error.unwrap_or(SysError::ENOENT))
    };
    // The target may be removed after the successful lookup but before its
    // file object is opened.  Preserve O_CREAT's race recovery without
    // resolving the path again.
    let open_result = match open_result {
        Err(SysError::ENOENT) if create_requested => {
            let (_parent_path, name) = split_parent_and_name(&raw_path);
            if let Some(parent) = parent_for_create.clone() {
                create_file_at(
                    parent,
                    name.as_str(),
                    OpenFlags::from_bits_truncate(flags),
                    effective_mode,
                )
            } else {
                Err(SysError::ENOENT)
            }
        }
        other => other,
    };
    let file = match open_result {
        Ok(file) => file,
        Err(e) => {
            let cwd_path = loop {
                if let Some(inner) = process.try_inner_exclusive_access() {
                    break inner.fs_context.lock().cwd.path();
                }
                crate::task::suspend_current_and_run_next();
            };
            error!(
                "sys_open failed for path: {}, dirfd={}, flags={:#o}, safe_flags={:#o}, mode={:#o}, cwd={}, err={:?}",
                raw_path,
                dirfd,
                flags,
                safe_flags.bits(),
                mode,
                cwd_path,
                e
            );
            return Err(e);
        }
    };
    if let Some(parent) = new_file_parent.as_ref() {
        if let Some(inode) = file.get_inode() {
            apply_new_inode_owner(&inode, parent, identity);
        }
    }
    let file_inode = file.get_inode();
    let file_type = file_inode.as_ref().map(|inode| inode.get_mode().get_type());
    if write_requested
        || file_type.is_some_and(|mode| mode == InodeMode::CHAR || mode == InodeMode::BLOCK)
    {
        let target_path = file.get_dentry().path();
        if write_requested {
            check_readonly_mount(&target_path)?;
        }
        if file_type.is_some_and(|mode| mode == InodeMode::CHAR || mode == InodeMode::BLOCK)
            && mount_flags_for_path(&target_path)
                .is_some_and(|flags| flags.contains(MountFlags::MS_NODEV))
        {
            return Err(SysError::EACCES);
        }
    }
    let notify_target = if inotify_may_have_instances() || fanotify_may_have_instances() {
        file_inode.as_ref().map(|_| file.get_dentry())
    } else {
        None
    };
    if fanotify_may_have_instances() {
        if let Some(target) = notify_target.as_ref() {
            fanotify_check_permission_dentry(target.clone(), FAN_OPEN_PERM)?;
        }
    }
    let fd = loop {
        let Some(mut inner) = process.try_inner_exclusive_access() else {
            crate::task::suspend_current_and_run_next();
            continue;
        };
        if let Some(inode) = file_inode.as_ref() {
            let real_size = inode.get_size() as usize;
            inode.set_size(real_size);
        }
        let fd = inner.alloc_fd()?;
        inner.fd_table[fd] = Some(file);
        if has_cloexec && fd < inner.fd_flags.len() {
            inner.fd_flags[fd] |= FD_CLOEXEC_FLAG;
        }
        break fd;
    };
    if let Some(target) = notify_target {
        let path = target.path();
        if created_path.as_deref() == Some(path.as_str()) {
            inotify_notify_dentry(target.clone(), IN_CREATE);
            fanotify_notify_dentry(target.clone(), FAN_CREATE);
        }
        if has_trunc && !created_path.as_deref().is_some_and(|p| p == path.as_str()) {
            notify_modify(&NotifyTarget::new(target.clone()));
        }
        inotify_notify_dentry(target.clone(), IN_OPEN);
        fanotify_notify_dentry(target, FAN_OPEN);
    }
    Ok(fd)
}

pub fn sys_openat2(
    dirfd: isize,
    path: *const u8,
    how_ptr: *const OpenHow,
    size: usize,
) -> SyscallResult {
    if size == 0 || size < OPEN_HOW_SIZE {
        return Err(SysError::EINVAL);
    }
    if path.is_null() {
        return Err(SysError::EFAULT);
    }
    if how_ptr.is_null() {
        return Err(SysError::EFAULT);
    }

    let token = current_user_token();
    let how = read_open_how(token, how_ptr, size)?;

    if how.flags & !VALID_OPENAT2_FLAGS != 0 {
        return Err(SysError::EINVAL);
    }
    if how.resolve & !VALID_OPENAT2_RESOLVE != 0 {
        return Err(SysError::EINVAL);
    }
    if how.flags & O_TMPFILE != O_TMPFILE && how.mode & !0o7777 != 0 {
        return Err(SysError::EINVAL);
    }
    if how.mode != 0 && how.flags & (OpenFlags::O_CREAT.bits() as u64 | O_TMPFILE) == 0 {
        return Err(SysError::EINVAL);
    }

    let raw_path = translated_str(token, path)?;
    check_open_path_len(&raw_path)?;
    validate_openat2_resolve(dirfd, &raw_path, &how)?;

    sys_openat(dirfd, path, how.flags as u32, how.mode as u32)
}

pub fn sys_getdents64(fd: usize, buf: *mut u8, len: usize) -> SyscallResult {
    info!("[DEBUG] sys_getdents64 called: fd={}, len={}", fd, len);
    const DIRENT64_HEADER_LEN: usize = 19;
    const DT_DIR: u8 = 4;

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
    if inode.get_mode().get_type() != InodeMode::DIR {
        return Err(SysError::ENOTDIR);
    }
    if inode.get_nlink() == 0 {
        return Err(SysError::ENOENT);
    }

    let dentry = file.get_dentry();
    let current_ino = inode.get_ino() as u64;
    let parent_ino = dentry
        .parent()
        .and_then(|parent| parent.get_inode())
        .map(|parent_inode| parent_inode.get_ino() as u64)
        .unwrap_or(current_ino);

    let raw_entries = file.ls();
    let mut entries = Vec::with_capacity(raw_entries.len() + 2);
    entries.push((".".to_string(), current_ino, DT_DIR));
    entries.push(("..".to_string(), parent_ino, DT_DIR));
    entries.extend(
        raw_entries
            .into_iter()
            .filter(|(name, _, _)| name != "." && name != ".."),
    );
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
        copy_to_user(token, buf, &kernel_buffer)?;
        maybe_update_atime(&dentry.path(), &inode, true);
    }
    file.set_offset(next_cookie);
    info!(
        "[DEBUG] returning {} bytes, next_cookie={}",
        kernel_buffer.len(),
        next_cookie
    );
    Ok(kernel_buffer.len())
}

#[allow(dead_code)]
pub(super) fn read_user_bytes(token: usize, ptr: *const u8, len: usize) -> SysResult<Vec<u8>> {
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

fn read_open_how(token: usize, ptr: *const OpenHow, size: usize) -> SysResult<OpenHow> {
    if size < OPEN_HOW_SIZE {
        return Err(SysError::EINVAL);
    }

    let bytes = read_user_bytes(token, ptr as *const u8, OPEN_HOW_SIZE)?;
    let flags = u64::from_ne_bytes(bytes[0..8].try_into().map_err(|_| SysError::EFAULT)?);
    let mode = u64::from_ne_bytes(bytes[8..16].try_into().map_err(|_| SysError::EFAULT)?);
    let resolve = u64::from_ne_bytes(bytes[16..24].try_into().map_err(|_| SysError::EFAULT)?);

    if size > OPEN_HOW_SIZE {
        if size == OPEN_HOW_SIZE + 1 {
            return Err(SysError::EFAULT);
        }
        let extra = read_user_bytes(
            token,
            unsafe { (ptr as *const u8).add(OPEN_HOW_SIZE) },
            size - OPEN_HOW_SIZE,
        )?;
        if extra.iter().any(|byte| *byte != 0) {
            return Err(SysError::E2BIG);
        }
    }

    Ok(OpenHow {
        flags,
        mode,
        resolve,
    })
}

pub fn sys_ioctl(fd: usize, request: usize, argp: usize) -> SyscallResult {
    let request = request as u32 as usize;
    const FIOCLEX: usize = 0x5451;
    const FIONCLEX: usize = 0x5450;
    const FIONBIO: usize = 0x5421;
    const FIOASYNC: usize = 0x5452;
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
    if request == FIONBIO {
        if argp == 0 {
            return Err(SysError::EFAULT);
        }
        let enabled = *translated_ref(current_user_token(), argp as *const i32)? != 0;
        let mut flags = file.status_flags();
        if enabled {
            flags |= OpenFlags::O_NONBLOCK.bits();
        } else {
            flags &= !OpenFlags::O_NONBLOCK.bits();
        }
        file.set_status_flags(flags);
        return Ok(0);
    }
    if let Some(inode) = file.get_inode() {
        let mode = inode.get_mode().get_type();
        if (mode == InodeMode::CHAR || mode == InodeMode::BLOCK)
            && !matches!(request, FIOCLEX | FIONCLEX | FIONBIO | FIOASYNC)
        {
            let target = file.get_dentry();
            landlock_check_dentry(&target, LANDLOCK_ACCESS_FS_IOCTL_DEV)?;
        }
    }
    let notify_target = if request == FS_IOC_SETFLAGS {
        notify_target_for_file_if_needed(&file)
    } else {
        None
    };
    let ret = file.ioctl(request, argp)?;
    if request == FS_IOC_SETFLAGS {
        if let Some(target) = notify_target.as_ref() {
            notify_attrib(target);
        }
    }
    Ok(ret)
}

/// syscall: syslog
/// TODO: unimplement
pub fn sys_syslog(_log_type: usize, _bufp: usize, _len: usize) -> SyscallResult {
    Ok(0)
}

/// Set the file mode creation mask and return the old mask.
pub fn sys_umask(mask: u32) -> SyscallResult {
    let process = current_process();
    let inner = process.inner_exclusive_access();
    let mut fs_context = inner.fs_context.lock();
    let old = fs_context.umask;
    fs_context.umask = mask & 0o777;
    Ok(old as usize)
}
