use crate::error::{SysError, SyscallResult};
use crate::fs::FS_MANAGER;
use crate::fs::devfs::urandom::fill_random;
use crate::fs::vfs::path::{AT_FDCWD, get_start_dentry};
use crate::fs::vfs::{File, FileInner};
use crate::mm::copy_to_user;
use crate::mm::{
    UserBuffer, get_free_memory, get_total_memory, translated_ref, translated_refmut,
    translated_str,
};
use crate::task::{
    block_current_and_run_next, current_process, current_task, current_user_token,
    exit_current_and_run_next, num_processes, pid2process, suspend_current_and_run_next,
};
use polyhal::consts::PAGE_SIZE;
use polyhal::timer::current_time;

#[cfg(target_arch = "riscv64")]
use crate::timer::*;
use crate::trap::_set_sum_bit;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::mem::size_of;
use spin::Mutex;
use spin::MutexGuard;

const LINUX_CAPABILITY_VERSION_3: u32 = 0x20080522;
const O_CLOEXEC: i32 = 0o2000000;
const O_NONBLOCK: u32 = 0o0004000;
const AT_SYMLINK_NOFOLLOW: u32 = 0x100;
const MOUNT_ATTR_RDONLY: u64 = 0x0000_0001;
const MOUNT_ATTR_NOSUID: u64 = 0x0000_0002;
const MOUNT_ATTR_NODEV: u64 = 0x0000_0004;
const MOUNT_ATTR_NOEXEC: u64 = 0x0000_0008;
const MOUNT_ATTR_NOATIME: u64 = 0x0000_0010;
const MOUNT_ATTR_STRICTATIME: u64 = 0x0000_0020;
const MOUNT_ATTR_NODIRATIME: u64 = 0x0000_0080;
const MOUNT_ATTR_NOSYMFOLLOW: u64 = 0x0020_0000;
const MOUNT_ATTR_SUPPORTED: u64 = MOUNT_ATTR_RDONLY
    | MOUNT_ATTR_NOSUID
    | MOUNT_ATTR_NODEV
    | MOUNT_ATTR_NOEXEC
    | MOUNT_ATTR_NOATIME
    | MOUNT_ATTR_STRICTATIME
    | MOUNT_ATTR_NODIRATIME
    | MOUNT_ATTR_NOSYMFOLLOW;

struct AnonFdFile {
    _name: &'static str,
    status_flags: u32,
}

impl AnonFdFile {
    fn new(name: &'static str, status_flags: u32) -> Self {
        Self {
            _name: name,
            status_flags,
        }
    }
}

impl File for AnonFdFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        panic!("anonymous fd has no FileInner")
    }

    fn get_inode(&self) -> Option<Arc<dyn crate::fs::vfs::inode::Inode>> {
        None
    }

    fn get_offset(&self) -> usize {
        0
    }

    fn set_offset(&self, _new_offset: usize) {}

    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        true
    }

    fn read(&self, _buf: UserBuffer) -> Result<usize, SysError> {
        Err(SysError::EBADF)
    }

    fn write(&self, _buf: UserBuffer) -> Result<usize, SysError> {
        Err(SysError::EBADF)
    }

    fn status_flags(&self) -> u32 {
        self.status_flags
    }
}

fn alloc_anon_fd(name: &'static str, cloexec: bool, status_flags: u32) -> SyscallResult {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    let fd = inner.alloc_fd()?;
    inner.fd_table[fd] = Some(Arc::new(AnonFdFile::new(name, status_flags)));
    if cloexec && fd < inner.fd_flags.len() {
        inner.fd_flags[fd] |= 1;
    }
    Ok(fd)
}

fn cloexec_from_flags(flags: i32) -> bool {
    flags & O_CLOEXEC != 0
}

fn status_from_flags(flags: i32) -> u32 {
    if flags & O_NONBLOCK as i32 != 0 {
        O_NONBLOCK
    } else {
        0
    }
}

pub fn sys_epoll_create1(flags: i32) -> SyscallResult {
    if flags == 1 {
        return alloc_anon_fd("epoll", false, 0);
    }
    if flags & !O_CLOEXEC != 0 {
        return Err(SysError::EINVAL);
    }
    alloc_anon_fd("epoll", cloexec_from_flags(flags), 0)
}

pub fn sys_eventfd2(_initval: usize, flags: i32) -> SyscallResult {
    const EFD_SEMAPHORE: i32 = 1;
    if flags & !(EFD_SEMAPHORE | O_CLOEXEC | O_NONBLOCK as i32) != 0 {
        return Err(SysError::EINVAL);
    }
    alloc_anon_fd(
        "eventfd",
        cloexec_from_flags(flags),
        status_from_flags(flags),
    )
}

pub fn sys_signalfd4(fd: isize, _mask: usize, _sizemask: usize, flags: i32) -> SyscallResult {
    if flags & !(O_CLOEXEC | O_NONBLOCK as i32) != 0 {
        return Err(SysError::EINVAL);
    }
    if fd >= 0 {
        return Ok(fd as usize);
    }
    alloc_anon_fd(
        "signalfd",
        cloexec_from_flags(flags),
        status_from_flags(flags),
    )
}

pub fn sys_pidfd_open(pid: usize, flags: u32) -> SyscallResult {
    if flags != 0 {
        return Err(SysError::EINVAL);
    }
    if pid2process(pid).is_none() {
        return Err(SysError::ESRCH);
    }
    alloc_anon_fd("pidfd", false, 0)
}

pub fn sys_fanotify_init(flags: u32, event_f_flags: u32) -> SyscallResult {
    const FAN_CLASS_MASK: u32 = 0x3;
    const FAN_CLOEXEC: u32 = 0x0000_0001;
    const FAN_NONBLOCK: u32 = 0x0000_0002;
    let allowed = FAN_CLASS_MASK | FAN_CLOEXEC | FAN_NONBLOCK;
    if flags & !allowed != 0 {
        return Err(SysError::EINVAL);
    }
    let status_flags = if flags & FAN_NONBLOCK != 0 || event_f_flags & O_NONBLOCK != 0 {
        O_NONBLOCK
    } else {
        0
    };
    alloc_anon_fd("fanotify", flags & FAN_CLOEXEC != 0, status_flags)
}

pub fn sys_userfaultfd(flags: i32) -> SyscallResult {
    if flags & !(O_CLOEXEC | O_NONBLOCK as i32) != 0 {
        return Err(SysError::EINVAL);
    }
    alloc_anon_fd(
        "userfaultfd",
        cloexec_from_flags(flags),
        status_from_flags(flags),
    )
}

pub fn sys_perf_event_open(
    _attr: usize,
    _pid: isize,
    _cpu: isize,
    _group_fd: isize,
    flags: u32,
) -> SyscallResult {
    if flags & !O_CLOEXEC as u32 != 0 {
        return Err(SysError::EINVAL);
    }
    alloc_anon_fd("perf_event", flags & O_CLOEXEC as u32 != 0, 0)
}

pub fn sys_io_uring_setup(entries: u32, _params: usize) -> SyscallResult {
    if entries == 0 {
        return Err(SysError::EINVAL);
    }
    alloc_anon_fd("io_uring", false, 0)
}

pub fn sys_bpf(cmd: u32, _attr: usize, _size: u32) -> SyscallResult {
    const BPF_MAP_CREATE: u32 = 0;
    if cmd != BPF_MAP_CREATE {
        return Err(SysError::EINVAL);
    }
    alloc_anon_fd("bpf_map", false, 0)
}

#[derive(Clone)]
struct FsContext {
    fs_name: String,
    source: Option<String>,
    created: bool,
    mount_attrs: u32,
    picked: bool,
    legacy_param_size: usize,
    opened_path: Option<String>,
}

static FS_CONTEXTS: Mutex<BTreeMap<usize, FsContext>> = Mutex::new(BTreeMap::new());
static MOUNT_ATTRS: Mutex<BTreeMap<String, u64>> = Mutex::new(BTreeMap::new());

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MountAttr {
    attr_set: u64,
    attr_clr: u64,
    propagation: u64,
    userns_fd: u64,
}

pub fn mount_attr_flags_for_path(path: &str) -> u64 {
    let attrs = MOUNT_ATTRS.lock();
    let mut best = 0usize;
    let mut flags = 0u64;
    for (mount_path, mount_flags) in attrs.iter() {
        if path.starts_with(mount_path) {
            let matched = mount_path.ends_with('/')
                || path.len() == mount_path.len()
                || path.as_bytes().get(mount_path.len()) == Some(&b'/');
            if matched && mount_path.len() >= best {
                best = mount_path.len();
                flags = *mount_flags;
            }
        }
    }
    flags
}

fn statvfs_flags_from_mount_attrs(attrs: u64) -> u64 {
    const ST_RDONLY: u64 = 1;
    const ST_NOSUID: u64 = 2;
    const ST_NODEV: u64 = 4;
    const ST_NOEXEC: u64 = 8;
    const ST_NOATIME: u64 = 1024;
    const ST_NODIRATIME: u64 = 2048;
    const ST_NOSYMFOLLOW: u64 = 8192;

    let mut flags = 0;
    if attrs & MOUNT_ATTR_RDONLY != 0 {
        flags |= ST_RDONLY;
    }
    if attrs & MOUNT_ATTR_NOSUID != 0 {
        flags |= ST_NOSUID;
    }
    if attrs & MOUNT_ATTR_NODEV != 0 {
        flags |= ST_NODEV;
    }
    if attrs & MOUNT_ATTR_NOEXEC != 0 {
        flags |= ST_NOEXEC;
    }
    if attrs & MOUNT_ATTR_NOATIME != 0 {
        flags |= ST_NOATIME;
    }
    if attrs & MOUNT_ATTR_NODIRATIME != 0 {
        flags |= ST_NODIRATIME;
    }
    if attrs & MOUNT_ATTR_NOSYMFOLLOW != 0 {
        flags |= ST_NOSYMFOLLOW;
    }
    flags
}

fn fsopen_supported(fs_name: &str) -> bool {
    match fs_name {
        "ext2" | "ext3" | "ext4" | "vfat" | "fat" | "fat32" | "tmpfs" | "tempfs" | "devfs"
        | "proc" | "procfs" | "sysfs" => true,
        name => FS_MANAGER.lock().contains_key(name),
    }
}

fn get_anon_fd(fd: usize) -> SyscallResult {
    let process = current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
        return Err(SysError::EBADF);
    }
    Ok(0)
}

pub fn sys_fsopen(fs_name: *const u8, flags: u32) -> SyscallResult {
    const FSOPEN_CLOEXEC: u32 = 0x1;
    if fs_name.is_null() {
        return Err(SysError::EFAULT);
    }
    if flags & !FSOPEN_CLOEXEC != 0 {
        return Err(SysError::EINVAL);
    }
    let fs_name = translated_str(current_user_token(), fs_name)?;
    if !fsopen_supported(&fs_name) {
        return Err(SysError::ENODEV);
    }
    let fd = alloc_anon_fd("fsopen", flags & FSOPEN_CLOEXEC != 0, 0)?;
    FS_CONTEXTS.lock().insert(fd, FsContext {
        fs_name,
        source: None,
        created: false,
        mount_attrs: 0,
        picked: false,
        legacy_param_size: 0,
        opened_path: None,
    });
    Ok(fd)
}

pub fn sys_fsconfig(
    fd: usize,
    cmd: u32,
    key: *const u8,
    value: *const u8,
    aux: i32,
) -> SyscallResult {
    const FSCONFIG_SET_FLAG: u32 = 0;
    const FSCONFIG_SET_STRING: u32 = 1;
    const FSCONFIG_SET_BINARY: u32 = 2;
    const FSCONFIG_SET_PATH: u32 = 3;
    const FSCONFIG_SET_PATH_EMPTY: u32 = 4;
    const FSCONFIG_SET_FD: u32 = 5;
    const FSCONFIG_CMD_CREATE: u32 = 6;
    const FSCONFIG_CMD_RECONFIGURE: u32 = 7;
    const FSCONFIG_CMD_CREATE_EXCL: u32 = 8;

    if fd == usize::MAX {
        return Err(SysError::EINVAL);
    }
    get_anon_fd(fd)?;
    let token = current_user_token();
    let mut contexts = FS_CONTEXTS.lock();
    let ctx = contexts.get_mut(&fd).ok_or(SysError::EBADF)?;

    match cmd {
        FSCONFIG_SET_FLAG => {
            if key.is_null() || !value.is_null() || aux != 0 {
                return Err(SysError::EINVAL);
            }
            let _ = translated_str(token, key)?;
        }
        FSCONFIG_SET_STRING => {
            if key.is_null() || value.is_null() || aux != 0 {
                return Err(SysError::EINVAL);
            }
            let key = translated_str(token, key)?;
            let value = translated_str(token, value)?;
            if key.is_empty() {
                let next_size = if ctx.legacy_param_size == 0 {
                    value.len() + 3
                } else {
                    ctx.legacy_param_size + value.len() + 2
                };
                if next_size > PAGE_SIZE {
                    return Err(SysError::EINVAL);
                }
                ctx.legacy_param_size = next_size;
                return Ok(0);
            }
            if key == "source" {
                ctx.source = Some(value);
            }
        }
        FSCONFIG_SET_PATH | FSCONFIG_SET_PATH_EMPTY => {
            if key.is_null() || value.is_null() || (aux < 0 && aux != AT_FDCWD as i32) {
                return Err(SysError::EINVAL);
            }
            let key = translated_str(token, key)?;
            let value = translated_str(token, value)?;
            if key == "source" {
                ctx.source = Some(value);
            }
        }
        FSCONFIG_SET_BINARY => {
            if key.is_null() || value.is_null() || aux <= 0 {
                return Err(SysError::EINVAL);
            }
            let _ = translated_str(token, key)?;
        }
        FSCONFIG_SET_FD => {
            if key.is_null() || !value.is_null() || aux < 0 {
                return Err(SysError::EINVAL);
            }
            let _ = translated_str(token, key)?;
            get_anon_fd(aux as usize)?;
        }
        FSCONFIG_CMD_CREATE | FSCONFIG_CMD_CREATE_EXCL => {
            if !key.is_null() || !value.is_null() || aux != 0 {
                return Err(SysError::EINVAL);
            }
            ctx.created = true;
        }
        FSCONFIG_CMD_RECONFIGURE => {
            if !key.is_null() || !value.is_null() || aux != 0 {
                return Err(SysError::EINVAL);
            }
            if !ctx.picked {
                return Err(SysError::EOPNOTSUPP);
            }
        }
        _ => return Err(SysError::EOPNOTSUPP),
    }
    Ok(0)
}

pub fn sys_fsmount(fd: usize, flags: u32, mount_attrs: u32) -> SyscallResult {
    const FSMOUNT_CLOEXEC: u32 = 0x1;

    if flags & !FSMOUNT_CLOEXEC != 0 || (mount_attrs as u64) & !MOUNT_ATTR_SUPPORTED != 0 {
        return Err(SysError::EINVAL);
    }
    get_anon_fd(fd)?;
    let mut ctx = FS_CONTEXTS
        .lock()
        .get(&fd)
        .cloned()
        .ok_or(SysError::EBADF)?;
    if !ctx.created {
        return Err(SysError::EINVAL);
    }
    ctx.mount_attrs = statvfs_flags_from_mount_attrs(mount_attrs as u64) as u32;
    let mount_fd = alloc_anon_fd("fsmount", flags & FSMOUNT_CLOEXEC != 0, 0)?;
    FS_CONTEXTS.lock().insert(mount_fd, ctx);
    Ok(mount_fd)
}

pub fn sys_move_mount(
    from_dfd: isize,
    from_path: *const u8,
    _to_dfd: isize,
    to_path: *const u8,
    flags: u32,
) -> SyscallResult {
    const MOVE_MOUNT_F_SYMLINKS: u32 = 0x0000_0001;
    const MOVE_MOUNT_F_AUTOMOUNTS: u32 = 0x0000_0002;
    const MOVE_MOUNT_F_EMPTY_PATH: u32 = 0x0000_0004;
    const MOVE_MOUNT_T_SYMLINKS: u32 = 0x0000_0010;
    const MOVE_MOUNT_T_AUTOMOUNTS: u32 = 0x0000_0020;
    const MOVE_MOUNT_T_EMPTY_PATH: u32 = 0x0000_0040;
    const MOVE_MOUNT_SET_GROUP: u32 = 0x0000_0100;
    const MOVE_MOUNT_BENEATH: u32 = 0x0000_0200;
    const MOVE_MOUNT_MASK: u32 = MOVE_MOUNT_F_SYMLINKS
        | MOVE_MOUNT_F_AUTOMOUNTS
        | MOVE_MOUNT_F_EMPTY_PATH
        | MOVE_MOUNT_T_SYMLINKS
        | MOVE_MOUNT_T_AUTOMOUNTS
        | MOVE_MOUNT_T_EMPTY_PATH
        | MOVE_MOUNT_SET_GROUP
        | MOVE_MOUNT_BENEATH;

    if flags & !MOVE_MOUNT_MASK != 0 || to_path.is_null() {
        return Err(SysError::EINVAL);
    }
    if from_path.is_null() {
        return Err(SysError::EFAULT);
    }
    if from_dfd < 0 {
        return Err(SysError::EBADF);
    }
    if !mount_path_is_absolute_or_cwd(_to_dfd, to_path) {
        return Err(SysError::EBADF);
    }

    let token = current_user_token();
    let from_path = translated_str(token, from_path)?;
    let mount_path = translated_str(token, to_path)?;
    if !from_path.is_empty() {
        return Err(SysError::ENOENT);
    }
    if flags & MOVE_MOUNT_F_EMPTY_PATH == 0 {
        return Err(SysError::EINVAL);
    }

    get_anon_fd(from_dfd as usize)?;
    let ctx = FS_CONTEXTS
        .lock()
        .get(&(from_dfd as usize))
        .cloned()
        .ok_or(SysError::EBADF)?;
    if !ctx.created {
        return Err(SysError::EINVAL);
    }

    let source = ctx
        .source
        .clone()
        .unwrap_or_else(|| match ctx.fs_name.as_str() {
            "tmpfs" | "tempfs" => "none".to_string(),
            _ => String::new(),
        });
    if source.is_empty() {
        return Err(SysError::EINVAL);
    }

    let ret = super::fs::do_mount(source, mount_path.clone(), ctx.fs_name.clone(), 0);
    if ret.is_ok() {
        let cwd = current_process().inner_exclusive_access().cwd.clone();
        let mount_path = crate::fs::vfs::path::resolve_path(cwd, &mount_path)
            .map(|dentry| dentry.path())
            .unwrap_or(mount_path);
        MOUNT_ATTRS
            .lock()
            .insert(mount_path, ctx.mount_attrs as u64);
    }
    ret
}

fn mount_path_is_absolute_or_cwd(to_dfd: isize, to_path: *const u8) -> bool {
    if to_dfd == crate::fs::vfs::path::AT_FDCWD {
        return true;
    }
    if to_dfd < 0 {
        return false;
    }
    if to_path.is_null() {
        return false;
    }
    true
}

pub fn sys_fspick(_dfd: isize, path: *const u8, flags: u32) -> SyscallResult {
    const FSPICK_CLOEXEC: u32 = 0x1;
    const FSPICK_SYMLINK_NOFOLLOW: u32 = 0x2;
    const FSPICK_NO_AUTOMOUNT: u32 = 0x4;
    const FSPICK_EMPTY_PATH: u32 = 0x8;
    if path.is_null() {
        return Err(SysError::EFAULT);
    }
    if flags & !(FSPICK_CLOEXEC | FSPICK_SYMLINK_NOFOLLOW | FSPICK_NO_AUTOMOUNT | FSPICK_EMPTY_PATH)
        != 0
    {
        return Err(SysError::EINVAL);
    }
    let path = translated_str(current_user_token(), path)?;
    if path.is_empty() && flags & FSPICK_EMPTY_PATH == 0 {
        return Err(SysError::EINVAL);
    }
    let start = get_start_dentry(_dfd, &path)?;
    let _ = crate::fs::vfs::path::resolve_path(start, &path)?;
    let fd = alloc_anon_fd("fspick", flags & FSPICK_CLOEXEC != 0, 0)?;
    FS_CONTEXTS.lock().insert(fd, FsContext {
        fs_name: "tmpfs".to_string(),
        source: Some("none".to_string()),
        created: true,
        mount_attrs: 0,
        picked: true,
        legacy_param_size: 0,
        opened_path: None,
    });
    Ok(fd)
}

pub fn sys_open_tree(dfd: isize, path: *const u8, flags: u32) -> SyscallResult {
    const OPEN_TREE_CLOEXEC: u32 = 0x0008_0000;
    const OPEN_TREE_CLONE: u32 = 1;
    const AT_EMPTY_PATH: u32 = 0x1000;
    const AT_RECURSIVE: u32 = 0x8000;
    if path.is_null() {
        return Err(SysError::EFAULT);
    }
    if flags
        & !(OPEN_TREE_CLONE
            | OPEN_TREE_CLOEXEC
            | AT_EMPTY_PATH
            | AT_RECURSIVE
            | AT_SYMLINK_NOFOLLOW)
        != 0
    {
        return Err(SysError::EINVAL);
    }
    let path = translated_str(current_user_token(), path)?;
    if path.is_empty() && flags & AT_EMPTY_PATH == 0 {
        return Err(SysError::ENOENT);
    }
    let start = get_start_dentry(dfd, &path)?;
    let dentry = crate::fs::vfs::path::resolve_path(start, &path)?;
    let opened_path = dentry.path();
    let fd = alloc_anon_fd("open_tree", flags & OPEN_TREE_CLOEXEC != 0, 0)?;
    FS_CONTEXTS.lock().insert(fd, FsContext {
        fs_name: "tmpfs".to_string(),
        source: Some("none".to_string()),
        created: true,
        mount_attrs: mount_attr_flags_for_path(&opened_path) as u32,
        picked: true,
        legacy_param_size: 0,
        opened_path: Some(opened_path),
    });
    Ok(fd)
}

pub fn sys_mount_setattr(
    dfd: isize,
    path: *const u8,
    flags: u32,
    attr: *const MountAttr,
    size: usize,
) -> SyscallResult {
    const AT_EMPTY_PATH: u32 = 0x1000;
    const AT_RECURSIVE: u32 = 0x8000;
    if path.is_null() || attr.is_null() {
        return Err(SysError::EFAULT);
    }
    if flags & !(AT_EMPTY_PATH | AT_RECURSIVE | AT_SYMLINK_NOFOLLOW) != 0 {
        return Err(SysError::EINVAL);
    }
    if size < size_of::<MountAttr>() {
        return Err(SysError::EINVAL);
    }
    let token = current_user_token();
    let mount_attr = *translated_ref(token, attr)?;
    if mount_attr.propagation != 0 || mount_attr.userns_fd != 0 {
        return Err(SysError::EINVAL);
    }
    if (mount_attr.attr_set | mount_attr.attr_clr) & !MOUNT_ATTR_SUPPORTED != 0 {
        return Err(SysError::EINVAL);
    }
    if mount_attr.attr_set & mount_attr.attr_clr != 0 {
        return Err(SysError::EINVAL);
    }

    let path = translated_str(token, path)?;
    if path.is_empty() {
        if flags & AT_EMPTY_PATH == 0 || dfd < 0 {
            return Err(SysError::EINVAL);
        }
        get_anon_fd(dfd as usize)?;
        let mut contexts = FS_CONTEXTS.lock();
        let ctx = contexts.get_mut(&(dfd as usize)).ok_or(SysError::EBADF)?;
        let current = ctx.mount_attrs as u64;
        let next = (current & !statvfs_flags_from_mount_attrs(mount_attr.attr_clr))
            | statvfs_flags_from_mount_attrs(mount_attr.attr_set);
        ctx.mount_attrs = next as u32;
        if let Some(path) = ctx.opened_path.clone() {
            MOUNT_ATTRS.lock().insert(path, next);
        }
        return Ok(0);
    }

    let start = get_start_dentry(dfd, &path)?;
    let dentry = crate::fs::vfs::path::resolve_path(start, &path)?;
    let mount_path = dentry.path();
    let mut attrs = MOUNT_ATTRS.lock();
    let current = attrs.get(&mount_path).cloned().unwrap_or(0);
    let next = (current & !statvfs_flags_from_mount_attrs(mount_attr.attr_clr))
        | statvfs_flags_from_mount_attrs(mount_attr.attr_set);
    attrs.insert(mount_path, next);
    Ok(0)
}

// pub fn sys_memfd_create(name: *const u8, flags: u32) -> SyscallResult {
//     const MFD_CLOEXEC: u32 = 0x0001;
//     const MFD_ALLOW_SEALING: u32 = 0x0002;
//     if name.is_null() {
//         return Err(SysError::EFAULT);
//     }
//     if flags & !(MFD_CLOEXEC | MFD_ALLOW_SEALING) != 0 {
//         return Err(SysError::EINVAL);
//     }
//     let _ = translated_str(current_user_token(), name)?;
//     alloc_anon_fd("memfd", flags & MFD_CLOEXEC != 0, 0)
// }

pub fn sys_memfd_secret(flags: u32) -> SyscallResult {
    if flags != 0 {
        return Err(SysError::EINVAL);
    }
    alloc_anon_fd("memfd_secret", false, 0)
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct CapUserHeader {
    version: u32,
    pid: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct CapUserData {
    effective: u32,
    permitted: u32,
    inheritable: u32,
}

/// capget: get process capabilities.
/// For now, all processes are treated as having full capabilities (root).
pub fn sys_capget(hdrp: usize, datap: usize) -> SyscallResult {
    if hdrp == 0 || datap == 0 {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let header = translated_refmut(token, hdrp as *mut CapUserHeader)?;

    if header.version != LINUX_CAPABILITY_VERSION_3 {
        header.version = LINUX_CAPABILITY_VERSION_3;
        return Err(SysError::EINVAL);
    }

    let pid = header.pid;
    if pid < 0 {
        return Err(SysError::EINVAL);
    }
    if pid != 0 {
        let current_pid = current_task()
            .and_then(|t| t.process.upgrade().map(|p| p.getpid() as i32))
            .unwrap_or(0);
        if pid != current_pid {
            return Err(SysError::ESRCH);
        }
    }

    // V3 requires two CapUserData structs (64 capabilities)
    let data0 = translated_refmut(token, datap as *mut CapUserData)?;
    data0.effective = !0u32;
    data0.permitted = !0u32;
    data0.inheritable = !0u32;

    let data1 = translated_refmut(token, unsafe { (datap as *mut CapUserData).add(1) })?;
    data1.effective = !0u32;
    data1.permitted = !0u32;
    data1.inheritable = !0u32;

    Ok(0)
}

/// capset: set process capabilities.
/// For now, accepts but ignores the request (stub implementation).
pub fn sys_capset(hdrp: usize, _datap: usize) -> SyscallResult {
    if hdrp == 0 {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let header = translated_refmut(token, hdrp as *mut CapUserHeader)?;

    if header.version != LINUX_CAPABILITY_VERSION_3 {
        header.version = LINUX_CAPABILITY_VERSION_3;
        return Err(SysError::EINVAL);
    }

    let pid = header.pid;
    if pid < 0 {
        return Err(SysError::EINVAL);
    }
    if pid != 0 {
        let current_pid = current_task()
            .and_then(|t| t.process.upgrade().map(|p| p.getpid() as i32))
            .unwrap_or(0);
        if pid != current_pid {
            return Err(SysError::EPERM);
        }
    }

    // Stub: ignore actual capability changes.
    Ok(0)
}

/// getrandom: fill user buffer with pseudo-random bytes.
/// Since Kairix has no hardware RNG, we use a simple xorshift64 PRNG.
/// 现在复用 /dev/urandom 的 fill_random 实现，避免逐字节拷贝。
pub fn sys_getrandom(buf: *mut u8, buflen: usize, _flags: u32) -> SyscallResult {
    if buflen == 0 {
        return Ok(0);
    }
    if buf.is_null() {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let mut local_buf = Vec::with_capacity(buflen);
    local_buf.resize(buflen, 0u8);
    fill_random(&mut local_buf);
    copy_to_user(token, buf as *const u8, &local_buf);
    Ok(buflen)
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SysInfo {
    pub uptime: i64,
    pub loads: [u64; 3],
    pub totalram: u64,
    pub freeram: u64,
    pub sharedram: u64,
    pub bufferram: u64,
    pub totalswap: u64,
    pub freeswap: u64,
    pub procs: u16,
    pub pad: u16,
    pub totalhigh: u64,
    pub freehigh: u64,
    pub mem_unit: u32,
    pub _f: [u8; 4],
}

impl SysInfo {
    pub fn new() -> Self {
        Self {
            uptime: 0,
            loads: [0; 3],
            totalram: 0,
            freeram: 0,
            sharedram: 0,
            bufferram: 0,
            totalswap: 0,
            freeswap: 0,
            procs: 0,
            pad: 0,
            totalhigh: 0,
            freehigh: 0,
            mem_unit: 1,
            _f: [0; 4],
        }
    }
}

pub fn sys_sysinfo(info: *mut SysInfo) -> SyscallResult {
    if info.is_null() {
        return Err(SysError::EFAULT);
    }
    _set_sum_bit();
    let token = current_user_token();
    let mut sysinfo = SysInfo::new();
    sysinfo.uptime = (current_time().as_micros() / 1_000_000) as i64;
    sysinfo.totalram = get_total_memory() as u64;
    sysinfo.freeram = get_free_memory() as u64;
    sysinfo.procs = num_processes() as u16;
    sysinfo.mem_unit = 1;

    let src_bytes = unsafe {
        core::slice::from_raw_parts(&sysinfo as *const _ as *const u8, size_of::<SysInfo>())
    };
    copy_to_user(token, info as *const u8, src_bytes);
    Ok(0)
}

/// membarrier: issue memory barriers on a set of CPUs.
/// This provides a way to synchronize memory accesses across CPUs.
/// For simplicity, we implement a basic version that supports the query command
/// and performs a full memory barrier for other commands.
pub fn sys_membarrier(cmd: i32, flags: i32, _cpu_mask: *mut u64) -> SyscallResult {
    // membarrier command constants
    const MEMBARRIER_CMD_QUERY: i32 = 0;
    const MEMBARRIER_CMD_GLOBAL: i32 = 1;
    const MEMBARRIER_CMD_GLOBAL_EXPEDITED: i32 = 2;
    const MEMBARRIER_CMD_REGISTER_GLOBAL_EXPEDITED: i32 = 3;
    const MEMBARRIER_CMD_PRIVATE_EXPEDITED: i32 = 4;
    const MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED: i32 = 5;

    // Check flags - only flag currently defined is MEMBARRIER_FLAG_CPU_MASK
    if flags != 0 {
        return Err(SysError::EINVAL);
    }

    match cmd {
        MEMBARRIER_CMD_QUERY => {
            // Return supported commands
            // We support: QUERY, GLOBAL, GLOBAL_EXPEDITED
            let supported = (1 << MEMBARRIER_CMD_GLOBAL) | (1 << MEMBARRIER_CMD_GLOBAL_EXPEDITED);
            Ok(supported)
        }
        MEMBARRIER_CMD_GLOBAL | MEMBARRIER_CMD_GLOBAL_EXPEDITED => {
            // Perform a full memory barrier
            // On RISC-V, we use sfence.vma for TLB flush and fence for memory ordering
            #[cfg(target_arch = "riscv64")]
            unsafe {
                core::arch::asm!("fence", options(nomem, nostack));
            }
            #[cfg(target_arch = "loongarch64")]
            unsafe {
                // LoongArch: dbar 0 performs a full memory barrier
                core::arch::asm!("dbar 0", options(nomem, nostack));
            }
            Ok(0)
        }
        MEMBARRIER_CMD_REGISTER_GLOBAL_EXPEDITED => {
            // Register for global expedited membarrier
            // In our simple implementation, we just return success
            Ok(0)
        }
        MEMBARRIER_CMD_PRIVATE_EXPEDITED | MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED => {
            // Private expedited commands require PRIV_CAP_MEMBARRIER capability
            // which we don't support in this simple implementation
            Err(SysError::EPERM)
        }
        _ => Err(SysError::EINVAL),
    }
}
