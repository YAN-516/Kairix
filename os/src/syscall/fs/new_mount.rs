use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::FS_MANAGER;
use crate::fs::vfs::path::{AT_FDCWD, get_start_dentry};
use crate::mm::{translated_ref, translated_str};
use crate::syscall::misc::alloc_anon_fd;
use crate::task::{current_process, current_user_token};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use core::mem::size_of;
use polyhal::consts::PAGE_SIZE;
use spin::Mutex;

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

type FsContextRef = Arc<Mutex<FsContext>>;

static FS_CONTEXTS: Mutex<BTreeMap<(usize, usize), FsContextRef>> = Mutex::new(BTreeMap::new());
static MOUNT_ATTRS: Mutex<BTreeMap<String, u64>> = Mutex::new(BTreeMap::new());

#[derive(Debug, Clone, Copy)]
pub(crate) struct NewMountStats {
    pub fs_contexts: usize,
    pub fs_context_pids: usize,
    pub max_contexts_per_pid: usize,
    pub max_contexts_pid: usize,
    pub mount_attrs: usize,
    pub lock_busy: bool,
}

pub(crate) fn try_new_mount_stats() -> NewMountStats {
    let Some(contexts) = FS_CONTEXTS.try_lock() else {
        return NewMountStats {
            fs_contexts: 0,
            fs_context_pids: 0,
            max_contexts_per_pid: 0,
            max_contexts_pid: 0,
            mount_attrs: 0,
            lock_busy: true,
        };
    };
    let Some(attrs) = MOUNT_ATTRS.try_lock() else {
        return NewMountStats {
            fs_contexts: contexts.len(),
            fs_context_pids: 0,
            max_contexts_per_pid: 0,
            max_contexts_pid: 0,
            mount_attrs: 0,
            lock_busy: true,
        };
    };

    let mut fs_context_pids = 0usize;
    let mut max_contexts_per_pid = 0usize;
    let mut max_contexts_pid = 0usize;
    let mut current_pid = None;
    let mut current_count = 0usize;
    for ((pid, _fd), _) in contexts.iter() {
        if current_pid == Some(*pid) {
            current_count += 1;
        } else {
            if let Some(pid) = current_pid {
                fs_context_pids += 1;
                if current_count > max_contexts_per_pid {
                    max_contexts_per_pid = current_count;
                    max_contexts_pid = pid;
                }
            }
            current_pid = Some(*pid);
            current_count = 1;
        }
    }
    if let Some(pid) = current_pid {
        fs_context_pids += 1;
        if current_count > max_contexts_per_pid {
            max_contexts_per_pid = current_count;
            max_contexts_pid = pid;
        }
    }

    NewMountStats {
        fs_contexts: contexts.len(),
        fs_context_pids,
        max_contexts_per_pid,
        max_contexts_pid,
        mount_attrs: attrs.len(),
        lock_busy: false,
    }
}

fn current_context_key(fd: usize) -> (usize, usize) {
    (current_process().getpid(), fd)
}

pub(crate) fn remove_fs_context(pid: usize, fd: usize) {
    FS_CONTEXTS.lock().remove(&(pid, fd));
}

pub(crate) fn remove_fs_contexts_for_pid(pid: usize) {
    FS_CONTEXTS.lock().retain(|(ctx_pid, _), _| *ctx_pid != pid);
}

pub(crate) fn duplicate_fs_context(pid: usize, old_fd: usize, new_fd: usize) {
    let ctx = FS_CONTEXTS.lock().get(&(pid, old_fd)).cloned();
    if let Some(ctx) = ctx {
        FS_CONTEXTS.lock().insert((pid, new_fd), ctx);
    }
}

fn path_is_same_or_under(path: &str, root: &str) -> bool {
    path == root
        || (root != "/"
            && path
                .strip_prefix(root)
                .is_some_and(|rest| rest.starts_with('/')))
}

pub(crate) fn remove_mount_attrs_under(root: &str) {
    MOUNT_ATTRS
        .lock()
        .retain(|path, _| !path_is_same_or_under(path, root));
}

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
    FS_CONTEXTS.lock().insert(
        current_context_key(fd),
        Arc::new(Mutex::new(FsContext {
            fs_name,
            source: None,
            created: false,
            mount_attrs: 0,
            picked: false,
            legacy_param_size: 0,
            opened_path: None,
        })),
    );
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
    let ctx = FS_CONTEXTS
        .lock()
        .get(&current_context_key(fd))
        .cloned()
        .ok_or(SysError::EBADF)?;
    let mut ctx = ctx.lock();

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
    let ctx = FS_CONTEXTS
        .lock()
        .get(&current_context_key(fd))
        .cloned()
        .ok_or(SysError::EBADF)?;
    let mut ctx = ctx.lock().clone();
    if !ctx.created {
        return Err(SysError::EINVAL);
    }
    ctx.mount_attrs = statvfs_flags_from_mount_attrs(mount_attrs as u64) as u32;
    let mount_fd = alloc_anon_fd("fsmount", flags & FSMOUNT_CLOEXEC != 0, 0)?;
    FS_CONTEXTS
        .lock()
        .insert(current_context_key(mount_fd), Arc::new(Mutex::new(ctx)));
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
        .get(&current_context_key(from_dfd as usize))
        .cloned()
        .ok_or(SysError::EBADF)?;
    let ctx = ctx.lock().clone();
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

    let ret = super::do_mount(source, mount_path.clone(), ctx.fs_name.clone(), 0);
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
    if to_dfd == AT_FDCWD {
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
    FS_CONTEXTS.lock().insert(
        current_context_key(fd),
        Arc::new(Mutex::new(FsContext {
            fs_name: "tmpfs".to_string(),
            source: Some("none".to_string()),
            created: true,
            mount_attrs: 0,
            picked: true,
            legacy_param_size: 0,
            opened_path: None,
        })),
    );
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
    FS_CONTEXTS.lock().insert(
        current_context_key(fd),
        Arc::new(Mutex::new(FsContext {
            fs_name: "tmpfs".to_string(),
            source: Some("none".to_string()),
            created: true,
            mount_attrs: mount_attr_flags_for_path(&opened_path) as u32,
            picked: true,
            legacy_param_size: 0,
            opened_path: Some(opened_path),
        })),
    );
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
        let ctx = FS_CONTEXTS
            .lock()
            .get(&current_context_key(dfd as usize))
            .cloned()
            .ok_or(SysError::EBADF)?;
        let mut ctx = ctx.lock();
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
