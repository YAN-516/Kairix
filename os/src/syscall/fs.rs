use crate::alloc::string::ToString;
use crate::error::{SysError, SysResult, SyscallResult};
use core::error;
use polyhal::print;
use polyhal::println;
use polyhal::timer::current_time;
// use crate::config::PAGE_SIZE;
use crate::devices::BlockDevice;
use crate::drivers::BLOCK_DEVICE;
use crate::fs::FS_MANAGER;
use crate::fs::devfs::loopx::loop_block_device_from_inode;
use crate::fs::find_superblock_by_path;
use crate::fs::tmpfs::dentry::TempDentry;
use crate::fs::tmpfs::file::TempFile;
use crate::fs::tmpfs::inode::F_SEAL_GROW;
use crate::fs::tmpfs::inode::F_SEAL_SEAL;
use crate::fs::tmpfs::inode::F_SEAL_SHRINK;
use crate::fs::tmpfs::inode::F_SEAL_WRITE;
use crate::fs::tmpfs::inode::TempInode;
use crate::fs::vfs::OpenFlags;
use crate::fs::vfs::dcache::GLOBAL_DCACHE;
use crate::fs::vfs::file::File;
use crate::fs::vfs::file::open_file;
use crate::fs::vfs::fstype::MountFlags;
use crate::fs::vfs::inode::Inode;
use crate::fs::vfs::inode::InodeMode;
use crate::fs::vfs::kstat::STATX_ATTR_MOUNT_ROOT;
use crate::fs::vfs::kstat::kstat_to_statx;
use crate::fs::vfs::kstat::{Kstat, Statfs, Statx};
use crate::fs::vfs::path::{get_start_dentry, split_parent_and_name};
use crate::fs::vfs::path::{resolve_path, resolve_path_nofollow_last};
use crate::mm::PageTable;
use crate::mm::VirtAddr;
use crate::mm::copy_to_user;
use crate::mm::translated_ref;
use crate::mm::{UserBuffer, translated_byte_buffer, translated_refmut, translated_str};
use crate::socket::SOCKET_MANAGER;
use crate::sync::mutex::*;
use crate::sync::mutex::*;
use crate::syscall::fanotify::{
    FAN_ACCESS, FAN_ACCESS_PERM, FAN_ATTRIB, FAN_CLOSE_NOWRITE, FAN_CLOSE_WRITE, FAN_CREATE,
    FAN_MODIFY, FAN_OPEN, FAN_OPEN_PERM, fanotify_check_permission_dentry,
    fanotify_notify_delete_dentry, fanotify_notify_dentry, fanotify_notify_move,
    fanotify_notify_path, fanotify_notify_unmount,
};
use crate::syscall::inotify::{
    IN_ACCESS, IN_ATTRIB, IN_CLOSE_NOWRITE, IN_CLOSE_WRITE, IN_CREATE, IN_ISDIR, IN_MODIFY,
    IN_OPEN, inotify_notify_delete, inotify_notify_move, inotify_notify_path,
    inotify_notify_unmount,
};
use crate::syscall::landlock::{
    LANDLOCK_ACCESS_FS_IOCTL_DEV, LANDLOCK_ACCESS_FS_MAKE_BLOCK, LANDLOCK_ACCESS_FS_MAKE_CHAR,
    LANDLOCK_ACCESS_FS_MAKE_DIR, LANDLOCK_ACCESS_FS_MAKE_FIFO, LANDLOCK_ACCESS_FS_MAKE_REG,
    LANDLOCK_ACCESS_FS_MAKE_SOCK, LANDLOCK_ACCESS_FS_MAKE_SYM, LANDLOCK_ACCESS_FS_READ_DIR,
    LANDLOCK_ACCESS_FS_READ_FILE, LANDLOCK_ACCESS_FS_REFER, LANDLOCK_ACCESS_FS_REMOVE_DIR,
    LANDLOCK_ACCESS_FS_REMOVE_FILE, LANDLOCK_ACCESS_FS_TRUNCATE, LANDLOCK_ACCESS_FS_WRITE_FILE,
    landlock_check_dentry, landlock_check_path,
};
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
use log::*;
use log::{error, warn};
use lwext4_rust::InodeTypes;
use polyhal::consts::*;

/// Linux MAX_LFS_FILESIZE for 64-bit: i64::MAX
const MAX_LFS_FILESIZE: usize = i64::MAX as usize;
const PATH_MAX: usize = 4096;
const NAME_MAX: usize = 255;
pub(crate) const FD_CLOEXEC_FLAG: u32 = 1;

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

fn check_open_path_len(path: &str) -> SyscallResult {
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

fn apply_new_inode_owner(inode: &Arc<dyn Inode>, parent: &Arc<dyn crate::fs::vfs::Dentry>) {
    let process = current_process();
    let inner = process.inner_exclusive_access();
    inode.set_uid(inner.euid as usize);
    let parent_mode = parent.get_inode().map(|inode| inode.get_mode());
    if parent_mode.is_some_and(|mode| mode.contains(InodeMode::SET_GID)) {
        if let Some(parent_inode) = parent.get_inode() {
            inode.set_gid(parent_inode.get_gid());
        }
    } else {
        inode.set_gid(inner.egid as usize);
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

fn tmpfile_mode(parent: &Arc<dyn crate::fs::vfs::Dentry>, mode: u32) -> InodeMode {
    let process = current_process();
    let inner = process.inner_exclusive_access();
    let umask = inner.umask;
    let euid = inner.euid as usize;
    let egid = inner.egid as usize;
    drop(inner);

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
    let mut mode_bits = (mode & 0o7777) & !umask;
    if mode_bits & InodeMode::SET_GID.bits() != 0 && euid != 0 && file_gid != egid {
        mode_bits &= !InodeMode::SET_GID.bits();
    }
    InodeMode::from_bits_truncate(mode_bits | InodeMode::FILE.bits())
}

fn alloc_tmpfile_fd(
    dir: Arc<dyn crate::fs::vfs::Dentry>,
    flags: OpenFlags,
    mode: u32,
) -> SyscallResult {
    let inode = dir.get_inode().ok_or(SysError::ENOENT)?;
    if inode.get_mode().get_type() != InodeMode::DIR {
        return Err(SysError::ENOTDIR);
    }
    check_readonly_mount(&dir.path())?;
    if !check_inode_perm_effective(&inode, 3) {
        return Err(SysError::EACCES);
    }

    let process = current_process();
    let file_mode = tmpfile_mode(&dir, mode);
    let tmp_dentry = TempDentry::new(".tmpfile", Some(dir.clone()));
    let tmp_inode = Arc::new(TempInode::new(file_mode));
    tmp_inode.set_uid(process.inner_exclusive_access().euid as usize);
    if dir
        .get_inode()
        .is_some_and(|parent_inode| parent_inode.get_mode().contains(InodeMode::SET_GID))
    {
        if let Some(parent_inode) = dir.get_inode() {
            tmp_inode.set_gid(parent_inode.get_gid());
        }
    } else {
        tmp_inode.set_gid(process.inner_exclusive_access().egid as usize);
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

    let mut inner = process.inner_exclusive_access();
    let fd = inner.alloc_fd()?;
    inner.fd_table[fd] = Some(file);
    if cloexec && fd < inner.fd_flags.len() {
        inner.fd_flags[fd] |= FD_CLOEXEC_FLAG;
    }
    Ok(fd)
}

fn proc_self_fd_file(path: &str) -> Option<Arc<dyn File + Send + Sync>> {
    let fd_str = path.strip_prefix("/proc/self/fd/")?;
    if fd_str.is_empty() || fd_str.as_bytes().iter().any(|b| !b.is_ascii_digit()) {
        return None;
    }
    let fd = fd_str.parse::<usize>().ok()?;
    let process = current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return None;
    }
    inner.fd_table[fd].clone()
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
pub(crate) const FD_FANOTIFY_EVENT: u32 = 1 << 31;
pub(crate) const FILE_HANDLE_BYTES: u32 = 8;
pub(crate) const FILE_HANDLE_TYPE_INO: i32 = 1;
const ST_RDONLY: i64 = 1;
const ST_NOSUID: i64 = 2;
const ST_NODEV: i64 = 4;
const ST_NOEXEC: i64 = 8;
const ST_VALID: i64 = 32;
const ST_NOATIME: i64 = 1024;
const ST_NODIRATIME: i64 = 2048;
const ST_NOSYMFOLLOW: i64 = 8192;

#[repr(C)]
pub struct FileHandleHeader {
    pub handle_bytes: u32,
    pub handle_type: i32,
}

fn statfs_flags_from_mount_flags(flags: MountFlags) -> i64 {
    let mut stat_flags = ST_VALID;
    if flags.contains(MountFlags::MS_RDONLY) {
        stat_flags |= ST_RDONLY;
    }
    if flags.contains(MountFlags::MS_NOSUID) {
        stat_flags |= ST_NOSUID;
    }
    if flags.contains(MountFlags::MS_NODEV) {
        stat_flags |= ST_NODEV;
    }
    if flags.contains(MountFlags::MS_NOEXEC) {
        stat_flags |= ST_NOEXEC;
    }
    if flags.contains(MountFlags::MS_NOATIME) {
        stat_flags |= ST_NOATIME;
    }
    if flags.contains(MountFlags::MS_NODEIRATIME) {
        stat_flags |= ST_NODIRATIME;
    }
    if flags.contains(MountFlags::MS_NOSYMFOLLOW) {
        stat_flags |= ST_NOSYMFOLLOW;
    }
    stat_flags
}

fn mount_flags_for_path(path: &str) -> Option<MountFlags> {
    find_superblock_by_path(path).map(|sb| sb.inner().flags())
}

fn check_readonly_mount(path: &str) -> SyscallResult {
    if mount_flags_for_path(path).is_some_and(|flags| flags.contains(MountFlags::MS_RDONLY)) {
        Err(SysError::EROFS)
    } else {
        Ok(0)
    }
}

fn check_nosymfollow_mount(path: &str, dentry: &Arc<dyn crate::fs::vfs::Dentry>) -> SyscallResult {
    if !mount_flags_for_path(path).is_some_and(|flags| flags.contains(MountFlags::MS_NOSYMFOLLOW)) {
        return Ok(0);
    }

    if dentry
        .get_inode()
        .is_some_and(|inode| inode.get_mode().contains(InodeMode::LINK))
    {
        Err(SysError::ELOOP)
    } else {
        Ok(0)
    }
}

fn has_writable_file_on_superblock(target_sb: &Arc<dyn crate::fs::vfs::SuperBlock>) -> bool {
    for process in crate::task::all_processes() {
        let files: Vec<Arc<dyn File + Send + Sync>> = {
            let inner = process.inner_exclusive_access();
            inner
                .fd_table
                .iter()
                .filter_map(|file| file.as_ref().cloned())
                .collect()
        };

        for file in files {
            if !file.writable() || file.get_inode().is_none() {
                continue;
            }

            let path = file.get_dentry().path();
            if find_superblock_by_path(&path).is_some_and(|sb| Arc::ptr_eq(&sb, target_sb)) {
                return true;
            }
        }
    }
    false
}

pub(crate) fn maybe_update_atime(path: &str, inode: &Arc<dyn Inode>, is_dir: bool) {
    let Some(flags) = mount_flags_for_path(path) else {
        return;
    };
    if flags.contains(MountFlags::MS_NOATIME) {
        return;
    }
    if is_dir && flags.contains(MountFlags::MS_NODEIRATIME) {
        return;
    }
    let now_us = current_time().as_micros() as i64;
    inode.set_atime(now_us / 1_000_000, (now_us % 1_000_000) * 1000);
}

fn insert_dentry_subtree(root: Arc<dyn crate::fs::vfs::Dentry>) {
    GLOBAL_DCACHE.insert(root.path(), root.clone());
    for child in root.children().values() {
        insert_dentry_subtree(child.clone());
    }
}

fn clone_dentry_tree_for_mount(
    source: Arc<dyn crate::fs::vfs::Dentry>,
    parent: Option<Arc<dyn crate::fs::vfs::Dentry>>,
    name: &str,
) -> Arc<dyn crate::fs::vfs::Dentry> {
    let cloned = TempDentry::new(name, parent);
    if let Some(inode) = source.get_inode() {
        cloned.set_inode(inode);
    }
    cloned.bind_mount_dentry(source.clone());
    for (child_name, child) in source.children() {
        let cloned_child = clone_dentry_tree_for_mount(child, Some(cloned.clone()), &child_name);
        cloned.add_child(cloned_child);
    }
    cloned
}

fn register_bind_mount_superblock(source_path: &str, mount_point_abs: &str) -> SyscallResult {
    let source_sb = find_superblock_by_path(source_path).ok_or(SysError::EINVAL)?;
    let fs_mgr = FS_MANAGER.lock();
    for (_name, fstype) in fs_mgr.iter() {
        let mut supers = fstype.inner().supers.lock();
        if supers.values().any(|sb| Arc::ptr_eq(sb, &source_sb)) {
            supers.insert(mount_point_abs.to_string(), source_sb.clone());
            return Ok(0);
        }
    }
    Err(SysError::EINVAL)
}

fn move_mount_superblock(old_mount_abs: &str, new_mount_abs: &str) -> SyscallResult {
    let fs_mgr = FS_MANAGER.lock();
    for (_name, fstype) in fs_mgr.iter() {
        let mut supers = fstype.inner().supers.lock();
        if let Some(sb) = supers.remove(old_mount_abs) {
            supers.insert(new_mount_abs.to_string(), sb);
            return Ok(0);
        }
    }
    Err(SysError::EINVAL)
}

fn is_mount_propagation_change(flags: MountFlags) -> bool {
    flags.contains(MountFlags::MS_PRIVATE)
        || flags.contains(MountFlags::MS_SHARED)
        || flags.contains(MountFlags::MS_SLAVE)
        || flags.contains(MountFlags::MS_UNBINDABLE)
}

fn do_mount_propagation_change(mount_path: String) -> SyscallResult {
    let cwd = current_process().inner_exclusive_access().cwd.clone();
    let dentry = resolve_path(cwd, &mount_path)?;
    find_superblock_by_path(&dentry.path()).ok_or(SysError::EINVAL)?;
    Ok(0)
}

fn do_move_mount(source_path: String, mount_path: String) -> SyscallResult {
    if source_path.is_empty() {
        return Err(SysError::EINVAL);
    }

    let cwd = current_process().inner_exclusive_access().cwd.clone();
    let source_dentry = resolve_path(cwd.clone(), &source_path)?;
    let target_dentry = resolve_path(cwd, &mount_path)?;
    let old_mount_abs = source_dentry.path();
    let new_mount_abs = target_dentry.path();

    if old_mount_abs == new_mount_abs {
        return Err(SysError::EINVAL);
    }

    let source_parent = source_dentry.parent().ok_or(SysError::EINVAL)?;
    let target_parent = target_dentry.parent().ok_or(SysError::EINVAL)?;
    let source_name = source_dentry.name().to_string();
    let target_name = target_dentry.name().to_string();

    let source_original = source_dentry.get_mount_dentry().ok_or(SysError::EINVAL)?;
    let target_inode = target_dentry.get_inode().ok_or(SysError::ENOENT)?;
    if target_inode.get_mode().get_type() != InodeMode::DIR {
        return Err(SysError::ENOTDIR);
    }
    if target_dentry.get_mount_dentry().is_some() {
        return Err(SysError::EBUSY);
    }

    move_mount_superblock(&old_mount_abs, &new_mount_abs)?;

    let moved_root = clone_dentry_tree_for_mount(
        source_dentry.clone(),
        Some(target_parent.clone()),
        &target_name,
    );
    moved_root.store_mount_dentry(target_dentry.clone());
    source_dentry.fetch_mount_dentry();

    GLOBAL_DCACHE.remove_subtree(&old_mount_abs);
    source_parent.remove_child(&source_name);
    source_parent.add_child(source_original.clone());
    GLOBAL_DCACHE.insert(old_mount_abs, source_original);

    GLOBAL_DCACHE.remove_subtree(&new_mount_abs);
    target_parent.remove_child(&target_name);
    target_parent.add_child(moved_root.clone());
    insert_dentry_subtree(moved_root);
    GLOBAL_DCACHE.pin(new_mount_abs.clone());

    info!(
        "[sys_mount] move success: {} moved to {}",
        source_path, new_mount_abs
    );
    Ok(0)
}

fn do_bind_mount(source_path: String, mount_path: String, _flags: MountFlags) -> SyscallResult {
    if source_path.is_empty() {
        return Err(SysError::EINVAL);
    }

    let cwd = current_process().inner_exclusive_access().cwd.clone();
    let source_dentry = resolve_path(cwd.clone(), &source_path)?;
    let covered_dentry = resolve_path(cwd.clone(), &mount_path)?;
    let covered_inode = covered_dentry.get_inode().ok_or(SysError::ENOENT)?;
    if covered_inode.get_mode().get_type() != InodeMode::DIR {
        return Err(SysError::ENOTDIR);
    }

    let (parent_path, name) = split_parent_and_name(&mount_path);
    if name.is_empty() {
        return Err(SysError::EBUSY);
    }
    let parent = if parent_path == "/" {
        GLOBAL_DCACHE.get("/").unwrap().clone()
    } else {
        resolve_path(cwd.clone(), &parent_path)?
    };

    let mounted_root =
        clone_dentry_tree_for_mount(source_dentry.clone(), Some(parent.clone()), &name);
    mounted_root.store_mount_dentry(covered_dentry.clone());

    let mount_point_abs = if parent.path() == "/" {
        format!("/{}", name)
    } else {
        format!("{}/{}", parent.path(), name)
    };

    GLOBAL_DCACHE.remove_subtree(&mount_point_abs);
    parent.remove_child(&name);
    parent.add_child(mounted_root.clone());
    insert_dentry_subtree(mounted_root.clone());
    GLOBAL_DCACHE.pin(mount_point_abs.clone());
    register_bind_mount_superblock(&source_dentry.path(), &mount_point_abs)?;

    info!(
        "[sys_mount] bind success: {} mounted at {}",
        source_path, mount_point_abs
    );
    Ok(0)
}

fn block_device_for_mount_source(
    cwd: Arc<dyn crate::fs::vfs::Dentry>,
    source_path: &str,
) -> SysResult<Arc<dyn BlockDevice>> {
    match source_path {
        "/dev/vda" | "/dev/vda1" | "/dev/sda" | "/dev/sda1" | "/dev/xvda" | "/dev/xvda1" => {
            return Ok(BLOCK_DEVICE.clone());
        }
        _ => {}
    }

    let source_dentry = resolve_path(cwd, source_path)?;
    let source_inode = source_dentry.get_inode().ok_or(SysError::ENOTBLK)?;
    if source_inode.get_mode().get_type() != InodeMode::BLOCK {
        return Err(SysError::ENOTBLK);
    }
    if source_path.starts_with("/dev/loop") {
        return loop_block_device_from_inode(source_inode).ok_or(SysError::ENXIO);
    }

    Ok(BLOCK_DEVICE.clone())
}

fn should_fake_vfat_partition_mount(
    cwd: Arc<dyn crate::fs::vfs::Dentry>,
    source_path: &str,
    fs_name: &str,
    flags: MountFlags,
) -> bool {
    fs_name == "fat32"
        && !flags.contains(MountFlags::MS_REMOUNT)
        && source_path == "/dev/vda2"
        && matches!(resolve_path(cwd, source_path), Err(SysError::ENOENT))
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
    let process = current_process();
    let token = current_user_token();
    let path = process.inner_exclusive_access().cwd.clone().path();
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
    let process = current_process();
    let umask = process.inner_exclusive_access().umask;
    let mut mode_bits = (mode & 0o7777) & !umask | InodeMode::DIR.bits();
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
                apply_new_inode_owner(&inode, &parent);
            }
            let new_path = if parent.path() == "/" {
                format!("/{}", dir_name)
            } else {
                format!("{}/{}", parent.path(), dir_name)
            };
            inotify_notify_path(&new_path, IN_CREATE | IN_ISDIR);
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
    let process = current_process();
    let umask = process.inner_exclusive_access().umask;
    let file_type = match mode & InodeMode::TYPE_MASK.bits() {
        0 => InodeMode::FILE.bits(),
        file_type => file_type,
    };
    let mut perm = (mode & 0o7777) & !umask;
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
                    apply_new_inode_owner(&inode, &parent);
                }
            }
            inotify_notify_path(&new_path, IN_CREATE);
            if let Ok(target) = parent.find(name.as_str()) {
                fanotify_notify_dentry(target, FAN_CREATE);
            } else {
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
    let target = parent.find(name.as_str())?;
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
    let target_path = target.path();
    match parent.unlink(name.as_str(), flags) {
        Ok(0) => {
            let removed = is_dir || nlink_before <= 1;
            inotify_notify_delete(&target_path, is_dir, removed);
            fanotify_notify_delete_dentry(target);
            Ok(0)
        }
        Ok(ret) => Ok(ret),
        Err(err) => Err(err),
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
    if proc_fd_file
        .as_ref()
        .is_some_and(|file| file.get_fileinner().flags.contains(OpenFlags::O_TMPFILE))
    {
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

    match old_parent.rename(&old_name, new_parent, &new_name) {
        Ok(_) => {
            inotify_notify_move(&old_abs, &new_abs, old_is_dir);
            fanotify_notify_move(&old_abs, &new_abs, Some(old_dentry), old_is_dir);
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
    debug!("[sys_umount2] resolving target: {}", target_path);
    let mounted_dentry = resolve_path(cwd.clone(), &target_path)?;
    debug!("[sys_umount2] resolved target: {}", mounted_dentry.path());

    let (parent_path, name) = split_parent_and_name(&target_path);
    debug!(
        "[sys_umount2] resolving parent: parent_path={}, name={}",
        parent_path, name
    );
    let parent = if parent_path == "/" {
        GLOBAL_DCACHE.get("/").unwrap().clone()
    } else {
        resolve_path(cwd.clone(), &parent_path)?
    };
    debug!("[sys_umount2] resolved parent: {}", parent.path());

    // Unbind bind-mount fallback
    debug!("[sys_umount2] unbinding fallback for {}", target_path);
    mounted_dentry.unbind_mount_dentry();
    debug!("[sys_umount2] fetching covered dentry for {}", target_path);
    let mdentry = mounted_dentry.fetch_mount_dentry();

    if let Some(orig) = mdentry {
        let mount_point_abs = if parent.path() == "/" {
            format!("/{}", name)
        } else {
            format!("{}/{}", parent.path(), name)
        };
        debug!(
            "[sys_umount2] begin unmount mount_point={}, mounted={}, covered={}",
            mount_point_abs,
            mounted_dentry.path(),
            orig.path()
        );
        debug!(
            "[sys_umount2] before drain_all queued={}",
            crate::fs::writeback::pending_count()
        );
        let flushed = crate::fs::writeback::drain_all();
        debug!("[sys_umount2] after drain_all flushed={}", flushed);
        debug!("[sys_umount2] notifying unmount: {}", mount_point_abs);
        inotify_notify_unmount(&mount_point_abs);
        debug!("[sys_umount2] inotify notified: {}", mount_point_abs);
        fanotify_notify_unmount(&mount_point_abs);
        debug!("[sys_umount2] fanotify notified: {}", mount_point_abs);

        // Drop the mounted tree from caches before restoring the covered dentry.
        debug!(
            "[sys_umount2] dropping subtree page cache: {}",
            mount_point_abs
        );
        mounted_dentry.drop_subtree_page_cache();
        debug!(
            "[sys_umount2] clearing mounted subtree: {}",
            mount_point_abs
        );
        mounted_dentry.clear_subtree();
        debug!("[sys_umount2] removing dcache subtree: {}", mount_point_abs);
        GLOBAL_DCACHE.remove_subtree(&mount_point_abs);

        // Remove superblock from FsType.supers by mount_point_abs. Keep the
        // removed Arc outside the lock scope so filesystem Drop/unmount logic
        // cannot run while FS_MANAGER or a supers lock is still held.
        debug!("[sys_umount2] removing superblock: {}", mount_point_abs);
        let removed_sb = {
            let fs_mgr = FS_MANAGER.lock();
            let mut removed = None;
            for (fs_name, fstype) in fs_mgr.iter() {
                debug!(
                    "[sys_umount2] checking superblock table: fs={}, mount_point={}",
                    fs_name, mount_point_abs
                );
                let mut supers = fstype.inner().supers.lock();
                if let Some(sb) = supers.remove(&mount_point_abs) {
                    debug!(
                        "[sys_umount2] removed superblock entry: fs={}, mount_point={}",
                        fs_name, mount_point_abs
                    );
                    removed = Some(sb);
                    break;
                }
            }
            removed
        };
        debug!(
            "[sys_umount2] superblock table removal done: mount_point={}, removed={}",
            mount_point_abs,
            removed_sb.is_some()
        );

        // Remove the mounted dentry from parent and restore the original.
        debug!(
            "[sys_umount2] restoring covered dentry: {}",
            mount_point_abs
        );
        parent.remove_child(&name);
        parent.add_child(orig.clone());
        GLOBAL_DCACHE.insert(mount_point_abs.clone(), orig.clone());
        drop(removed_sb);
        debug!(
            "[sys_umount2] dropped removed superblock: {}",
            mount_point_abs
        );
        let flushed_after_drop = crate::fs::writeback::drain_all();
        debug!(
            "[sys_umount2] after superblock drop drain_all flushed={}",
            flushed_after_drop
        );

        info!(
            "[sys_umount2] success: restored {} at {}",
            orig.path(),
            mount_point_abs
        );
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

/// Read ahead to populate the page cache.
/// This is a simple implementation that returns success without actual prefetch.
pub fn sys_readahead(fd: usize, _offset: usize, _count: usize) -> SyscallResult {
    let process = current_process();
    let inner = process.inner_exclusive_access();
    let file = match inner.fd_table.get(fd) {
        Some(Some(f)) => f,
        _ => {
            drop(inner);
            return Err(SysError::EBADF);
        }
    };

    // Verify the file is readable
    if !file.readable() {
        drop(inner);
        return Err(SysError::EBADF);
    }

    // Check if this is a pipe (read end should fail with EINVAL)
    // Must check before get_fileinner() since Pipe doesn't support it
    if file.is_pipe() {
        drop(inner);
        return Err(SysError::EINVAL);
    }

    // Check if this is a socket
    if file.is_socket() {
        drop(inner);
        return Err(SysError::EINVAL);
    }

    // Check inode type - readahead only works on regular files
    let inode = match file.get_inode() {
        Some(i) => i,
        None => {
            // Special files like epoll, eventfd, signalfd, etc. have no inode
            drop(inner);
            return Err(SysError::EINVAL);
        }
    };

    // Check if the file was opened with O_PATH flag
    // Must check after get_inode() check since special files don't have fileinner
    if file.get_fileinner().flags.contains(OpenFlags::O_PATH) {
        drop(inner);
        return Err(SysError::EINVAL);
    }

    let mode = inode.get_mode();
    let file_type = mode.get_type();

    // readahead is only valid for regular files
    if file_type != InodeMode::FILE {
        drop(inner);
        return Err(SysError::EINVAL);
    }

    drop(inner);
    // For now, just return success without actual prefetch
    // In a real implementation, this would read ahead and populate the page cache
    info!("[DEBUG sys_readahead] fd={}", fd);
    Ok(0)
}

/// Mount a filesystem.
pub fn sys_mount(
    source: *const u8,
    mount_path: *const u8,
    fstype: *const u8,
    flags: usize,
    _data: *const u8,
) -> SyscallResult {
    let process = current_process();
    if process.inner_exclusive_access().euid != 0 {
        return Err(SysError::EPERM);
    }
    let token = current_user_token();
    let source_path = if source.is_null() {
        String::new()
    } else {
        mount_user_str(token, source)?
    };
    let mount_path = mount_user_str(token, mount_path)?;
    let fstype_path = mount_user_str(token, fstype)?;

    do_mount(source_path, mount_path, fstype_path, flags)
}

pub(crate) fn do_mount(
    source_path: String,
    mount_path: String,
    fstype_path: String,
    flags: usize,
) -> SyscallResult {
    if fstype_path.is_empty() {
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

    if flags.contains(MountFlags::MS_MOVE) {
        return do_move_mount(source_path, mount_path);
    }

    if flags.contains(MountFlags::MS_BIND) {
        return do_bind_mount(source_path, mount_path, flags);
    }

    if is_mount_propagation_change(flags) {
        return do_mount_propagation_change(mount_path);
    }

    let mut fs_name = match fstype_path.as_str() {
        "ext2" => "ext2",
        "ext3" => "ext3",
        "ext4" => "ext4",
        "vfat" | "fat" | "fat32" => "fat32",
        "tmpfs" | "tempfs" => "tmpfs",
        "devfs" => "devfs",
        "proc" | "procfs" => "proc",
        "sysfs" => "sysfs",
        name if FS_MANAGER.lock().contains_key(name) => name,
        _ => return Err(SysError::ENODEV),
    };

    let cwd = current_process().inner_exclusive_access().cwd.clone();
    if should_fake_vfat_partition_mount(cwd.clone(), &source_path, fs_name, flags) {
        info!(
            "[sys_mount] fake vfat partition mount: source={} target={}",
            source_path, mount_path
        );
        fs_name = "tmpfs";
    }

    let fs_type = FS_MANAGER
        .lock()
        .get(fs_name)
        .cloned()
        .ok_or(SysError::ENODEV)?;

    let is_remount = flags.contains(MountFlags::MS_REMOUNT);
    let device_backed_fs = matches!(fs_name, "ext4" | "fat32");
    let source_required = !is_remount
        && (device_backed_fs || !matches!(fs_name, "tmpfs" | "devfs" | "proc" | "sysfs"));
    if source_path.is_empty() && source_required {
        return Err(SysError::EINVAL);
    }

    let mdentry = resolve_path(cwd.clone(), &mount_path)?;
    let mdentry_inode = mdentry.get_inode().ok_or(SysError::ENOENT)?;
    if mdentry_inode.get_mode().get_type() != InodeMode::DIR {
        return Err(SysError::ENOTDIR);
    }

    if is_remount {
        let mount_path_abs = mdentry.path();
        if mdentry.get_mount_dentry().is_none()
            || crate::fs::find_superblock_by_path(&mount_path_abs)
                .is_none_or(|sb| sb.root().path() != mount_path_abs)
        {
            return Err(SysError::EINVAL);
        }
        if let Some(sb) = crate::fs::find_superblock_by_path(&mount_path_abs) {
            if flags.contains(MountFlags::MS_RDONLY) && has_writable_file_on_superblock(&sb) {
                return Err(SysError::EBUSY);
            }

            let mut new_flags = flags;
            new_flags.remove(MountFlags::MS_REMOUNT);
            sb.inner().set_flags(new_flags);
            info!(
                "[sys_mount] remount success: {} flags={:#x}",
                mount_path_abs,
                new_flags.bits()
            );
            return Ok(0);
        }
        return Err(SysError::EINVAL);
    }

    if mdentry.get_mount_dentry().is_some() {
        return Err(SysError::EBUSY);
    }

    let needs_block_device = !matches!(fs_name, "tmpfs" | "devfs" | "proc" | "sysfs");

    let (parent_path, name) = split_parent_and_name(&mount_path);
    if name.is_empty() {
        return Err(SysError::EBUSY);
    }
    let parent = if parent_path == "/" {
        GLOBAL_DCACHE.get("/").unwrap().clone()
    } else {
        resolve_path(cwd.clone(), &parent_path)?
    };
    let mount_point_abs = if parent.path() == "/" {
        format!("/{}", name)
    } else {
        format!("{}/{}", parent.path(), name)
    };

    let dev = if device_backed_fs || needs_block_device {
        Some(block_device_for_mount_source(cwd.clone(), &source_path)?)
    } else {
        None
    };

    let mounted_root = fs_type.mount(&name, Some(parent.clone()), flags, dev.clone())?;

    mounted_root.store_mount_dentry(mdentry.clone());

    GLOBAL_DCACHE.remove_subtree(&mount_point_abs);
    parent.add_child(mounted_root.clone());
    insert_dentry_subtree(mounted_root.clone());
    GLOBAL_DCACHE.pin(mount_point_abs.clone());

    info!(
        "[sys_mount] success: {} mounted at {}",
        fs_name, mount_point_abs
    );
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
    inner.cwd = target_dentry;
    Ok(0)
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
        return Err(SysError::EBADF);
    }
    if let Some(file) = &inner.fd_table[fd] {
        // warn!("write {} {}", fd, len);
        if !file.writable() {
            return Err(SysError::EBADF);
        }

        // 新增：检查 memfd seal: F_SEAL_WRITE 禁止写入
        if let Some(inode) = file.get_inode() {
            if (inode.get_seals() & F_SEAL_WRITE) != 0 {
                return Err(SysError::EPERM);
            }
        }

        let file = file.clone();
        let notify_target = file.get_inode().map(|_| file.get_dentry());
        let offset = file.get_offset();
        // release current task TCB manually to avoid multi-borrow
        drop(inner);

        if let Some(target) = notify_target.as_ref() {
            landlock_check_dentry(target, LANDLOCK_ACCESS_FS_WRITE_FILE)?;
        }
        check_write_size_limit(offset, len)?;
        let written = file.write(UserBuffer::new(translated_byte_buffer(token, buf, len)?))?;
        if written > 0 {
            if let Some(target) = notify_target {
                let path = target.path();
                inotify_notify_path(&path, IN_MODIFY);
                fanotify_notify_dentry(target, FAN_MODIFY);
            }
        }
        Ok(written)
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
                copy_to_user(token, stat_buf, stat_bytes)?;
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
    flags: u32,
    mask: usize,
    buf: *mut u8,
) -> SyscallResult {
    if buf.is_null() {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let raw_path = translated_str(token, pathname)?;
    const AT_EMPTY_PATH: u32 = 0x1000;
    const AT_SYMLINK_NOFOLLOW: u32 = 0x100;
    const AT_NO_AUTOMOUNT: u32 = 0x800;
    const AT_STATX_SYNC_TYPE: u32 = 0x6000;
    const STATX_RESERVED: usize = 0x8000_0000;
    const VALID_STATX_FLAGS: u32 =
        AT_EMPTY_PATH | AT_SYMLINK_NOFOLLOW | AT_NO_AUTOMOUNT | AT_STATX_SYNC_TYPE;

    if flags & !VALID_STATX_FLAGS != 0 || mask & STATX_RESERVED != 0 {
        return Err(SysError::EINVAL);
    }
    if !raw_path.is_empty() {
        check_open_path_len(&raw_path)?;
    }

    let stat = if raw_path.is_empty() {
        if (flags & AT_EMPTY_PATH) == 0 {
            return Err(SysError::ENOENT);
        }
        let process = current_process();
        if fd == crate::fs::vfs::path::AT_FDCWD {
            let inner = process.inner_exclusive_access();
            let cwd = inner.cwd.clone();
            let inode = cwd.get_inode().ok_or(SysError::ENOENT)?;
            drop(inner);
            let mut stat = Kstat::new();
            fill_kstat_from_inode(&inode, &mut stat);
            mark_statx_mount_root(&cwd, &mut stat);
            stat
        } else {
            let inner = process.inner_exclusive_access();
            let fd = fd as usize;
            if fd >= inner.fd_table.len() {
                return Err(SysError::EBADF);
            }
            let file = match inner.fd_table[fd].as_ref() {
                Some(file) => file.clone(),
                None => return Err(SysError::EBADF),
            };
            drop(inner);
            let mut stat = Kstat::new();
            file.get_stat(&mut stat)?;
            if file.get_inode().is_some() {
                let dentry = file.get_dentry();
                mark_statx_mount_root(&dentry, &mut stat);
            }
            stat
        }
    } else {
        let start_dentry = get_start_dentry(fd, &raw_path)?;
        let target = if flags & AT_SYMLINK_NOFOLLOW != 0 {
            resolve_path_nofollow_last(start_dentry, &raw_path)?
        } else {
            resolve_path(start_dentry, &raw_path)?
        };
        let inode = target.get_inode().ok_or(SysError::ENOENT)?;
        let mut stat = Kstat::new();
        fill_kstat_from_inode(&inode, &mut stat);
        mark_statx_mount_root(&target, &mut stat);
        stat
    };

    copy_statx_to_user(token, buf, &stat)
}

fn mark_statx_mount_root(dentry: &Arc<dyn crate::fs::vfs::Dentry>, stat: &mut Kstat) {
    let path = dentry.path();
    if find_superblock_by_path(&path).is_some_and(|sb| {
        let root = sb.root();
        Arc::ptr_eq(&root, dentry)
    }) {
        stat.stx_attributes |= STATX_ATTR_MOUNT_ROOT;
    }
}

fn fill_kstat_from_inode(inode: &Arc<dyn Inode>, stat: &mut Kstat) {
    stat.st_ino = inode.get_ino() as u64;
    stat.st_nlink = inode.get_nlink() as u32;
    stat.st_size = inode.get_size() as i64;
    stat.st_mode = inode.get_mode().bits();
    stat.st_uid = inode.get_uid() as u32;
    stat.st_gid = inode.get_gid() as u32;
    stat.st_rdev = inode.get_rdev() as u64;
    stat.st_blksize = 512;
    stat.st_blocks = ((stat.st_size as u64 + 511) / 512)
        .saturating_sub(inode.get_punched_hole_pages() as u64 * 8);
    stat.st_fs_flags = inode.get_fs_flags();
    stat.st_mnt_id = 1;
    let (atime_sec, atime_nsec) = inode.get_atime();
    let (mtime_sec, mtime_nsec) = inode.get_mtime();
    let (ctime_sec, ctime_nsec) = inode.get_ctime();
    stat.st_atime_sec = atime_sec;
    stat.st_atime_nsec = atime_nsec;
    stat.st_mtime_sec = mtime_sec;
    stat.st_mtime_nsec = mtime_nsec;
    stat.st_ctime_sec = ctime_sec;
    stat.st_ctime_nsec = ctime_nsec;
}

fn copy_statx_to_user(token: usize, buf: *mut u8, stat: &Kstat) -> SyscallResult {
    let statx = kstat_to_statx(&stat);
    let stat_bytes = unsafe {
        core::slice::from_raw_parts(
            &statx as *const _ as *const u8,
            core::mem::size_of::<Statx>(),
        )
    };
    crate::mm::copy_to_user(token, buf, stat_bytes)?;

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

    let now_us = current_time().as_micros() as i64;
    inode.set_ctime(now_us / 1_000_000, (now_us % 1_000_000) * 1000);

    let mask = IN_ATTRIB
        | if inode.get_mode().contains(InodeMode::DIR) {
            IN_ISDIR
        } else {
            0
        };
    let notify_path = target.path();
    inotify_notify_path(&notify_path, mask);
    fanotify_notify_dentry(target, FAN_ATTRIB);
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
    if let Ok(file) = open_file(
        start_dentry,
        raw_path.as_str(),
        OpenFlags::RDONLY,
        InodeMode::FILE,
    ) {
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
                crate::mm::copy_to_user(token, stat_buf, stat_bytes)?;
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
            inotify_notify_path(&new_path, IN_CREATE);
            fanotify_notify_path(&new_path, FAN_CREATE);
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
        let notify_target = file.get_inode().map(|_| file.get_dentry());
        // release current task TCB manually to avoid multi-borrow
        drop(inner);

        if !file.readable() {
            return Err(SysError::EBADF);
        }
        if let Some(target) = notify_target.as_ref() {
            landlock_check_dentry(target, LANDLOCK_ACCESS_FS_READ_FILE)?;
            fanotify_check_permission_dentry(target.clone(), FAN_ACCESS_PERM)?;
        }

        let buffers = translated_byte_buffer(token, buf, len)?;
        let user_buf = UserBuffer::new(buffers);
        let read_len = file.read(user_buf)?;
        if read_len > 0 {
            if let Some(target) = notify_target {
                let path = target.path();
                inotify_notify_path(&path, IN_ACCESS);
                fanotify_notify_dentry(target, FAN_ACCESS);
            }
        }
        Ok(read_len)
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
        let notify_target = file.get_inode().map(|_| file.get_dentry());
        drop(inner);

        if !file.readable() {
            return Err(SysError::EBADF);
        }
        // pipe/socket 等不支持定位的对象返回 ESPIPE
        if file.get_inode().is_none() {
            return Err(SysError::ESPIPE);
        }
        if let Some(target) = notify_target.as_ref() {
            fanotify_check_permission_dentry(target.clone(), FAN_ACCESS_PERM)?;
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
        let notify_target = file.get_inode().map(|_| file.get_dentry());
        drop(inner);

        if !file.writable() {
            return Err(SysError::EBADF);
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
        let written = result?;
        if written > 0 {
            if let Some(target) = notify_target {
                let path = target.path();
                inotify_notify_path(&path, IN_MODIFY);
                fanotify_notify_dentry(target, FAN_MODIFY);
            }
        }
        Ok(written)
    } else {
        Err(SysError::EBADF)
    }
}

pub fn sys_lseek(fd: usize, offset: isize, whence: i32) -> SyscallResult {
    const SEEK_SET: i32 = 0;
    const SEEK_CUR: i32 = 1;
    const SEEK_END: i32 = 2;
    const SEEK_DATA: i32 = 3;
    const SEEK_HOLE: i32 = 4;

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

    if whence == SEEK_DATA || whence == SEEK_HOLE {
        if is_dir {
            return Err(SysError::EINVAL);
        }
        if offset < 0 {
            return Err(SysError::EINVAL);
        }
        let start = offset as usize;
        let size = inode.get_size();
        if start >= size {
            return Err(SysError::ENXIO);
        }
        let new_off = find_data_or_hole_offset(file.clone(), start, size, whence == SEEK_HOLE)?;
        file.set_offset(new_off);
        return Ok(new_off);
    }

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

fn find_data_or_hole_offset(
    file: Arc<dyn File>,
    start: usize,
    size: usize,
    find_hole: bool,
) -> SysResult<usize> {
    let mut pos = start;
    let old_offset = file.get_offset();
    let mut buf = [0u8; PAGE_SIZE];
    let inode = file.get_inode();

    while pos < size {
        if let Some(inode) = inode.as_ref() {
            let page_id = pos / PAGE_SIZE;
            if inode.is_punched_hole_page(page_id) {
                if find_hole {
                    file.set_offset(old_offset);
                    return Ok(pos);
                }
                pos = ((page_id + 1) * PAGE_SIZE).min(size);
                continue;
            }
        }

        let page_end = ((pos / PAGE_SIZE) + 1) * PAGE_SIZE;
        let len = (page_end.min(size) - pos).min(buf.len());
        let static_buf: &'static mut [u8] =
            unsafe { core::slice::from_raw_parts_mut(buf.as_mut_ptr(), len) };

        file.set_offset(pos);
        let read_len = match file.read(UserBuffer::new(vec![static_buf])) {
            Ok(read_len) => read_len,
            Err(err) => {
                file.set_offset(old_offset);
                return Err(err);
            }
        };

        let matched = if read_len == 0 {
            find_hole
        } else {
            let read_slice = &buf[..read_len];
            let all_zero = read_slice.iter().all(|byte| *byte == 0);
            if find_hole { all_zero } else { !all_zero }
        };
        if matched {
            file.set_offset(old_offset);
            return Ok(pos);
        }

        if read_len == 0 {
            break;
        }
        pos += read_len;
    }

    file.set_offset(old_offset);
    if find_hole {
        Ok(size)
    } else {
        Err(SysError::ENXIO)
    }
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

fn check_inode_perm_effective(inode: &Arc<dyn crate::fs::vfs::inode::Inode>, mode: u32) -> bool {
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
    let (check_uid, check_gid) = {
        let process = current_process();
        let inner = process.inner_exclusive_access();
        if flags & AT_EACCESS != 0 {
            (inner.euid, inner.egid)
        } else {
            (inner.uid, inner.gid)
        }
    };
    let follow_last = flags & AT_SYMLINK_NOFOLLOW == 0;
    check_access_path_prefix_perm(
        start_dentry.clone(),
        &raw_path,
        follow_last,
        check_uid,
        check_gid,
    )?;

    let target = if !follow_last {
        resolve_path_nofollow_last(start_dentry, &raw_path)?
    } else {
        resolve_path(start_dentry, &raw_path)?
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

    let file = inner.fd_table[fd].as_ref().unwrap();

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
    let now_us = current_time().as_micros() as i64;
    inode.set_ctime(now_us / 1_000_000, (now_us % 1_000_000) * 1000);

    Ok(0)
}

///
pub fn sys_openat(dirfd: isize, path: *const u8, flags: u32, mode: u32) -> SyscallResult {
    // error!("[DEBUG] sys_openat called: dirfd={}, path={}, flags={:#x}", dirfd, translated_str(current_user_token(), path), flags);
    let process = current_process();
    let token = current_user_token();
    let raw_path = translated_str(token, path)?;
    check_open_path_len(&raw_path)?;
    let safe_flags = OpenFlags::from_bits_truncate(flags);
    let has_cloexec = safe_flags.contains(OpenFlags::O_CLOEXEC);
    let has_noatime = safe_flags.contains(OpenFlags::O_NOATIME);
    let has_tmpfile = safe_flags.contains(OpenFlags::O_TMPFILE);
    let write_requested = safe_flags.writable()
        || safe_flags.contains(OpenFlags::O_CREAT)
        || safe_flags.contains(OpenFlags::O_TRUNC)
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
        return alloc_tmpfile_fd(dir, safe_flags, mode);
    }
    let parent_for_create = if safe_flags.contains(OpenFlags::O_CREAT) {
        let (parent_path, name) = split_parent_and_name(&raw_path);
        if name.is_empty() {
            None
        } else if parent_path == "." || parent_path == "/" {
            Some(start_dentry.clone())
        } else {
            resolve_path(start_dentry.clone(), &parent_path).ok()
        }
    } else {
        None
    };
    let created_path = if safe_flags.contains(OpenFlags::O_CREAT) {
        let (_parent_path, name) = split_parent_and_name(&raw_path);
        if name.is_empty() {
            None
        } else {
            parent_for_create
                .clone()
                .and_then(|parent| match parent.find(name.as_str()) {
                    Ok(_) => None,
                    Err(_) => Some(if parent.path() == "/" {
                        format!("/{}", name)
                    } else {
                        format!("{}/{}", parent.path(), name)
                    }),
                })
        }
    } else {
        None
    };
    if let Some(_path) = created_path.as_ref() {
        if let Some(parent) = parent_for_create.as_ref() {
            check_readonly_mount(&parent.path())?;
        }
    } else if write_requested {
        let target = if safe_flags.contains(OpenFlags::O_NOFOLLOW) {
            resolve_path_nofollow_last(start_dentry.clone(), &raw_path)
        } else {
            resolve_path(start_dentry.clone(), &raw_path)
        };
        if let Ok(target) = target {
            check_readonly_mount(&target.path())?;
        }
    }

    let effective_mode = if safe_flags.contains(OpenFlags::O_CREAT) {
        let inner = process.inner_exclusive_access();
        let umask = inner.umask;
        drop(inner);
        InodeMode::from_bits_truncate((mode & 0o7777) & !umask | InodeMode::FILE.bits())
    } else {
        InodeMode::FILE
    };
    if !safe_flags.contains(OpenFlags::O_CREAT) {
        if let Ok(target) = resolve_path_nofollow_last(start_dentry.clone(), &raw_path) {
            check_nosymfollow_mount(&target.path(), &target)?;
        }
    }
    let existing_target = if safe_flags.contains(OpenFlags::O_CREAT) {
        if safe_flags.contains(OpenFlags::O_NOFOLLOW) {
            resolve_path_nofollow_last(start_dentry.clone(), &raw_path).ok()
        } else {
            resolve_path(start_dentry.clone(), &raw_path).ok()
        }
    } else {
        None
    };
    if safe_flags.contains(OpenFlags::O_CREAT)
        && safe_flags.contains(OpenFlags::O_EXCL)
        && existing_target.is_some()
    {
        return Err(SysError::EEXIST);
    }
    let target_for_checks = if let Some(target) = existing_target {
        Some(target)
    } else if safe_flags.contains(OpenFlags::O_NOFOLLOW) {
        resolve_path_nofollow_last(start_dentry.clone(), &raw_path).ok()
    } else {
        resolve_path(start_dentry.clone(), &raw_path).ok()
    };
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
        if !check_inode_perm_effective(&inode, requested_perm) {
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
        if safe_flags.contains(OpenFlags::O_TRUNC) {
            landlock_access |= LANDLOCK_ACCESS_FS_TRUNCATE;
        }
        landlock_check_dentry(target, landlock_access)?;
    }
    if target_for_checks.is_none() && safe_flags.contains(OpenFlags::O_TRUNC) {
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
    let new_file_parent = if safe_flags.contains(OpenFlags::O_CREAT) {
        let (_parent_path, name) = split_parent_and_name(&raw_path);
        if name.is_empty() || target_for_checks.is_some() {
            None
        } else {
            parent_for_create.clone()
        }
    } else {
        None
    };
    if let Some(parent) = new_file_parent.as_ref() {
        landlock_check_dentry(parent, LANDLOCK_ACCESS_FS_MAKE_REG)?;
    }
    if has_noatime {
        let target = if safe_flags.contains(OpenFlags::O_NOFOLLOW) {
            resolve_path_nofollow_last(start_dentry.clone(), &raw_path)
        } else {
            resolve_path(start_dentry.clone(), &raw_path)
        };
        if let Ok(target) = target {
            let inode = target.get_inode().ok_or(SysError::EIO)?;
            let owner_uid = inode.get_uid() as u32;
            let euid = {
                let inner = process.inner_exclusive_access();
                inner.euid
            };
            if euid != 0 && euid != owner_uid {
                return Err(SysError::EPERM);
            }
        }
    }
    let file = match open_file(start_dentry, raw_path.as_str(), safe_flags, effective_mode) {
        Ok(file) => file,
        Err(e) => {
            error!("sys_open failed for path: {}, err={:?}", raw_path, e);
            return Err(e);
        }
    };
    if let Some(parent) = new_file_parent.as_ref() {
        if let Some(inode) = file.get_inode() {
            apply_new_inode_owner(&inode, parent);
        }
    }
    let target_dentry = file.get_dentry();
    let target_path = target_dentry.path();
    if write_requested {
        check_readonly_mount(&target_path)?;
    }
    if file.get_inode().is_some_and(|inode| {
        let mode = inode.get_mode().get_type();
        mode == InodeMode::CHAR || mode == InodeMode::BLOCK
    }) && mount_flags_for_path(&target_path)
        .is_some_and(|flags| flags.contains(MountFlags::MS_NODEV))
    {
        return Err(SysError::EACCES);
    }
    let notify_target = file.get_inode().map(|_| file.get_dentry());
    if let Some(target) = notify_target.as_ref() {
        fanotify_check_permission_dentry(target.clone(), FAN_OPEN_PERM)?;
    }
    let fd = {
        let mut inner = process.inner_exclusive_access();
        if let Some(inode) = file.get_inode() {
            let real_size = inode.get_size() as usize;
            inode.set_size(real_size);
        }
        let fd = inner.alloc_fd()?;
        inner.fd_table[fd] = Some(file);
        if has_cloexec {
            if fd < inner.fd_flags.len() {
                inner.fd_flags[fd] |= FD_CLOEXEC_FLAG;
            }
        }
        fd
    };
    if let Some(target) = notify_target {
        let path = target.path();
        if created_path.as_deref() == Some(path.as_str()) {
            inotify_notify_path(&path, IN_CREATE);
            fanotify_notify_dentry(target.clone(), FAN_CREATE);
        }
        inotify_notify_path(&path, IN_OPEN);
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
    let fd_flags = inner.fd_flags.get(fd).copied().unwrap_or(0);
    let notify = file.get_inode().map(|_| {
        let target = file.get_dentry();
        let mask = if file.writable() {
            IN_CLOSE_WRITE
        } else {
            IN_CLOSE_NOWRITE
        };
        (target, mask)
    });
    if fd < inner.fd_flags.len() {
        inner.fd_flags[fd] = 0;
    }
    drop(inner);
    let _ = SOCKET_MANAGER.lock().close_socket_with_refcount(fd, pid);
    crate::fs::writeback::queue_file(file);
    if fd_flags & FD_FANOTIFY_EVENT == 0 {
        if let Some((target, mask)) = notify {
            let path = target.path();
            inotify_notify_path(&path, mask);
            let fan_mask = if mask == IN_CLOSE_WRITE {
                FAN_CLOSE_WRITE
            } else {
                FAN_CLOSE_NOWRITE
            };
            fanotify_notify_dentry(target, fan_mask);
        }
    }
    Ok(0)
}

/// close_range: close or mark file descriptors in the range [first, last].
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

    if flags & CLOSE_RANGE_CLOEXEC != 0 {
        let fd_table_len = inner.fd_table.len();
        if inner.fd_flags.len() < fd_table_len {
            inner.fd_flags.resize(fd_table_len, 0);
        }
        for fd in first..=end {
            if inner.fd_table[fd].is_some() {
                inner.fd_flags[fd] |= FD_CLOEXEC_FLAG;
            }
        }
        return Ok(0);
    }

    // Collect files to close to avoid holding the lock during socket close.
    let mut files_to_close: alloc::vec::Vec<(
        usize,
        alloc::sync::Arc<dyn crate::fs::File + Send + Sync>,
    )> = alloc::vec::Vec::new();
    for fd in first..=end {
        if let Some(file) = inner.fd_table[fd].take() {
            if fd < inner.fd_flags.len() {
                inner.fd_flags[fd] = 0;
            }
            files_to_close.push((fd, file));
        }
    }
    drop(inner);

    for (fd, file) in files_to_close {
        let _ = SOCKET_MANAGER.lock().close_socket_with_refcount(fd, pid);
        crate::fs::writeback::queue_file(file);
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

pub fn sys_dup3(old_fd: usize, new_fd: usize, flags: usize) -> SyscallResult {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();

    // Linux 语义：若 new_fd 超出资源限制，返回 EBADF。
    let max_fd = inner.rlimit_nofile.rlim_cur as usize;
    if new_fd >= max_fd {
        return Err(SysError::EBADF);
    }

    // dup3 语义：old_fd == new_fd 时强制返回 EINVAL（在 fd 有效性检查之前）。
    if old_fd == new_fd {
        return Err(SysError::EINVAL);
    }

    // dup3 只支持 flags == 0 或 flags == O_CLOEXEC
    const O_CLOEXEC: usize = 0o2000000;
    if flags != 0 && flags != O_CLOEXEC {
        return Err(SysError::EINVAL);
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
    let old_file = inner.fd_table[new_fd].take();

    inner.fd_table[new_fd] = file_clone;
    if flags == O_CLOEXEC {
        inner.fd_flags[new_fd] = FD_CLOEXEC_FLAG;
    } else {
        inner.fd_flags[new_fd] = 0;
    }
    drop(inner);
    if let Some(old_file) = old_file {
        crate::fs::writeback::queue_file(old_file);
    }
    Ok(new_fd)
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
    if !inode.get_mode().contains(InodeMode::DIR) {
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
    let file = inner.fd_table[fd].as_ref().unwrap().clone();
    drop(inner);
    if file.is_pipe() || file.is_socket() {
        return Err(SysError::EINVAL);
    }
    if file
        .get_inode()
        .is_some_and(|inode| inode.get_mode().get_type() == InodeMode::FIFO)
    {
        return Err(SysError::EINVAL);
    }
    file.flush();
    Ok(0)
}

pub fn sys_syncfs(fd: usize) -> SyscallResult {
    let process = current_process();
    let inner = process.inner_exclusive_access();

    if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
        return Err(SysError::EBADF);
    }
    let file = inner.fd_table[fd].as_ref().unwrap().clone();
    drop(inner);
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
    let file = inner.fd_table[fd].as_ref().unwrap().clone();
    drop(inner);

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

    if length > MAX_LFS_FILESIZE {
        return Err(SysError::EINVAL);
    }
    if file.is_socket() || file.is_pipe() {
        return Err(SysError::EINVAL);
    }
    if !file.writable() {
        return Err(SysError::EINVAL);
    }
    let inode = file.get_inode().ok_or(SysError::EINVAL)?;
    if !inode.get_mode().contains(InodeMode::FILE) {
        return Err(SysError::EINVAL);
    }

    // 检查 memfd seals
    let seals = inode.get_seals();
    let current_size = inode.get_size();

    // F_SEAL_SHRINK: 防止缩小文件
    if length < current_size && (seals & F_SEAL_SHRINK) != 0 {
        return Err(SysError::EPERM);
    }

    // F_SEAL_GROW: 防止扩大文件
    if length > current_size && (seals & F_SEAL_GROW) != 0 {
        return Err(SysError::EPERM);
    }

    let target = file.get_dentry();
    landlock_check_dentry(&target, LANDLOCK_ACCESS_FS_TRUNCATE)?;
    file.truncate(length as u64)
}

///
pub fn sys_truncate(path: *const u8, length: usize) -> SyscallResult {
    check_write_size_limit(0, length)?;
    let token = current_user_token();
    let path_str = translated_str(token, path)?;
    let cwd = current_process().inner_exclusive_access().cwd.clone();
    let file = open_file(cwd, &path_str, OpenFlags::WRONLY, InodeMode::FILE)?;
    let inode = file.get_inode().ok_or(SysError::ENOENT)?;
    if !inode.get_mode().contains(InodeMode::FILE) {
        return Err(SysError::EINVAL);
    }
    let target = file.get_dentry();
    landlock_check_dentry(&target, LANDLOCK_ACCESS_FS_TRUNCATE)?;
    file.truncate(length as u64)?;
    let path = target.path();
    inotify_notify_path(&path, IN_MODIFY);
    fanotify_notify_dentry(target, FAN_MODIFY);
    Ok(0)
}

/// sys_fallocate: preallocate or deallocate file space.
/// Supports mode=0 (default), FALLOC_FL_KEEP_SIZE, and FALLOC_FL_PUNCH_HOLE.
pub fn sys_fallocate(fd: usize, mode: i32, offset: usize, len: usize) -> SyscallResult {
    const FALLOC_FL_KEEP_SIZE: i32 = 0x01;
    const FALLOC_FL_PUNCH_HOLE: i32 = 0x02;
    const FALLOC_FL_COLLAPSE_RANGE: i32 = 0x08;
    const FALLOC_FL_ZERO_RANGE: i32 = 0x10;
    const FALLOC_FL_INSERT_RANGE: i32 = 0x20;
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
    if mode == 0 || (mode & FALLOC_FL_ZERO_RANGE) != 0 {
        check_write_size_limit(offset, len)?;
    }
    let end = match offset.checked_add(len) {
        Some(v) => v,
        None => return Err(SysError::EFBIG),
    };
    // 支持常见 LTP 覆盖的 fallocate 模式。
    let supported_modes = FALLOC_FL_KEEP_SIZE
        | FALLOC_FL_PUNCH_HOLE
        | FALLOC_FL_COLLAPSE_RANGE
        | FALLOC_FL_ZERO_RANGE
        | FALLOC_FL_INSERT_RANGE;
    if (mode & !supported_modes) != 0 {
        return Err(SysError::EOPNOTSUPP);
    }
    if (mode & FALLOC_FL_COLLAPSE_RANGE) != 0 {
        if mode & !FALLOC_FL_COLLAPSE_RANGE != 0 {
            return Err(SysError::EINVAL);
        }
        if offset % PAGE_SIZE != 0 || len % PAGE_SIZE != 0 || end > inode.get_size() {
            return Err(SysError::EINVAL);
        }
        let current_size = inode.get_size();
        shift_file_range(file.clone(), end, offset, current_size - end)?;
        inode.set_size(current_size - len);
        inode.clear_punched_holes();
        touch_modified_inode(inode.clone());
        let path = file.get_dentry().path();
        inotify_notify_path(&path, IN_MODIFY);
        fanotify_notify_path(&path, FAN_MODIFY);
        return Ok(0);
    }
    if (mode & FALLOC_FL_INSERT_RANGE) != 0 {
        if mode & !FALLOC_FL_INSERT_RANGE != 0 {
            return Err(SysError::EINVAL);
        }
        if offset % PAGE_SIZE != 0 || len % PAGE_SIZE != 0 || offset > inode.get_size() {
            return Err(SysError::EINVAL);
        }
        let current_size = inode.get_size();
        inode.set_size(current_size + len);
        inode.clear_punched_holes();
        shift_file_range_reverse(file.clone(), offset, offset + len, current_size - offset)?;
        zero_file_range(file.clone(), offset, len)?;
        touch_modified_inode(inode.clone());
        let path = file.get_dentry().path();
        inotify_notify_path(&path, IN_MODIFY);
        fanotify_notify_path(&path, FAN_MODIFY);
        return Ok(0);
    }
    if (mode & FALLOC_FL_ZERO_RANGE) != 0 {
        if mode & !(FALLOC_FL_ZERO_RANGE | FALLOC_FL_KEEP_SIZE) != 0 {
            return Err(SysError::EINVAL);
        }
        zero_file_range(file.clone(), offset, len)?;
        if (mode & FALLOC_FL_KEEP_SIZE) == 0 && end > inode.get_size() {
            file.truncate(end as u64)?;
        }
        touch_modified_inode(inode.clone());
        let path = file.get_dentry().path();
        inotify_notify_path(&path, IN_MODIFY);
        fanotify_notify_path(&path, FAN_MODIFY);
        return Ok(0);
    }

    // FALLOC_FL_PUNCH_HOLE: 打孔操作，将指定范围清零
    if (mode & FALLOC_FL_PUNCH_HOLE) != 0 {
        if (mode & FALLOC_FL_KEEP_SIZE) == 0 {
            return Err(SysError::EOPNOTSUPP);
        }
        // 检查 F_SEAL_WRITE seal
        if inode.get_seals() & F_SEAL_WRITE != 0 {
            return Err(SysError::EPERM);
        }

        // punch hole 只需要将指定范围清零，不需要改变文件大小
        let current_size = inode.get_size();
        let punch_end = end.min(current_size);
        if offset < punch_end {
            // 通过文件系统的 inode 直接清零
            use crate::fs::page::pagecache::PAGE_CACHE;
            let ino = inode.cache_inode_id().unwrap_or_else(|| inode.get_ino());
            let start_page = offset / PAGE_SIZE;
            let end_page = (punch_end + PAGE_SIZE - 1) / PAGE_SIZE;
            for page_id in start_page..end_page {
                if let Some(page) = PAGE_CACHE.lock().get_page(ino, page_id) {
                    let mut page_writer = page.write();
                    let page_start = page_id * PAGE_SIZE;
                    let page_end = (page_id + 1) * PAGE_SIZE;
                    let data_start = if page_start < offset {
                        offset - page_start
                    } else {
                        0
                    };
                    let data_end = if page_end > punch_end {
                        punch_end - page_start
                    } else {
                        PAGE_SIZE
                    };
                    if data_start < data_end {
                        page_writer.frame.ppn.get_bytes_array()[data_start..data_end].fill(0);
                        page_writer.dirty = true;
                    }
                }
                let full_page_start = page_id * PAGE_SIZE;
                let full_page_end = full_page_start + PAGE_SIZE;
                if offset <= full_page_start && full_page_end <= punch_end {
                    inode.add_punched_hole_page(page_id);
                }
            }
            touch_modified_inode(inode.clone());
            let path = file.get_dentry().path();
            inotify_notify_path(&path, IN_MODIFY);
            fanotify_notify_path(&path, FAN_MODIFY);
        }
        return Ok(0);
    }

    let current_size = inode.get_size();
    if mode == 0 && end > current_size {
        file.truncate(end as u64)
    } else {
        Ok(0)
    }
}

fn touch_modified_inode(inode: Arc<dyn Inode>) {
    let now_us = current_time().as_micros() as i64;
    let now_sec = now_us / 1_000_000;
    let now_nsec = (now_us % 1_000_000) * 1000;
    inode.set_mtime(now_sec, now_nsec);
    inode.set_ctime(now_sec, now_nsec);
}

fn zero_file_range(file: Arc<dyn File>, offset: usize, len: usize) -> SysResult<()> {
    if len == 0 {
        return Ok(());
    }

    let inode = file.get_inode().ok_or(SysError::ENODEV)?;
    let cache_inode_id = inode.cache_inode_id().unwrap_or_else(|| inode.get_ino());
    let end = offset.checked_add(len).ok_or(SysError::EFBIG)?;
    let start_page = offset / PAGE_SIZE;
    let end_page = (end + PAGE_SIZE - 1) / PAGE_SIZE;

    for page_id in start_page..end_page {
        let page_start = page_id * PAGE_SIZE;
        let page_end = page_start + PAGE_SIZE;
        let data_start = offset.saturating_sub(page_start);
        let data_end = end.min(page_end) - page_start;
        if data_start >= data_end {
            continue;
        }
        inode.clear_punched_hole_page(page_id);
        if let Some(page) = crate::fs::page::pagecache::PAGE_CACHE
            .lock()
            .get_page(cache_inode_id, page_id)
        {
            let mut page_writer = page.write();
            page_writer.frame.ppn.get_bytes_array()[data_start..data_end].fill(0);
            page_writer.dirty = true;
        }
    }

    Ok(())
}

fn read_file_range(file: Arc<dyn File>, offset: usize, buf: &mut [u8]) -> SysResult<usize> {
    let old_offset = file.get_offset();
    let static_buf: &'static mut [u8] =
        unsafe { core::slice::from_raw_parts_mut(buf.as_mut_ptr(), buf.len()) };
    file.set_offset(offset);
    let ret = file.read(UserBuffer::new(vec![static_buf]));
    file.set_offset(old_offset);
    ret
}

fn write_file_range(file: Arc<dyn File>, offset: usize, buf: &[u8]) -> SysResult<usize> {
    let old_offset = file.get_offset();
    let mut data = Vec::from(buf);
    let static_buf: &'static mut [u8] =
        unsafe { core::slice::from_raw_parts_mut(data.as_mut_ptr(), data.len()) };
    file.set_offset(offset);
    let ret = file.write(UserBuffer::new(vec![static_buf]));
    file.set_offset(old_offset);
    ret
}

fn shift_file_range(
    file: Arc<dyn File>,
    src_offset: usize,
    dst_offset: usize,
    len: usize,
) -> SysResult<()> {
    let mut copied = 0usize;
    let mut buf = [0u8; PAGE_SIZE];
    while copied < len {
        let chunk = (len - copied).min(PAGE_SIZE);
        let read_len = read_file_range(file.clone(), src_offset + copied, &mut buf[..chunk])?;
        if read_len == 0 {
            break;
        }
        write_file_range(file.clone(), dst_offset + copied, &buf[..read_len])?;
        copied += read_len;
    }
    Ok(())
}

fn shift_file_range_reverse(
    file: Arc<dyn File>,
    src_offset: usize,
    dst_offset: usize,
    len: usize,
) -> SysResult<()> {
    let mut remaining = len;
    let mut buf = [0u8; PAGE_SIZE];
    while remaining > 0 {
        let chunk = remaining.min(PAGE_SIZE);
        remaining -= chunk;
        let read_len = read_file_range(file.clone(), src_offset + remaining, &mut buf[..chunk])?;
        if read_len == 0 {
            zero_file_range(file.clone(), dst_offset + remaining, chunk)?;
        } else {
            write_file_range(file.clone(), dst_offset + remaining, &buf[..read_len])?;
        }
    }
    Ok(())
}

///
pub fn sys_sync() -> SyscallResult {
    crate::fs::writeback::drain_all();
    let mut files = Vec::new();
    let pid_map = crate::task::manager::PID2PCB.lock();
    for (_, process) in pid_map.iter() {
        if let Some(inner) = process.inner_try_access() {
            for fd in 0..inner.fd_table.len() {
                if let Some(file) = inner.fd_table[fd].as_ref() {
                    files.push(file.clone());
                }
            }
        }
    }
    drop(pid_map);
    for file in files {
        file.flush();
    }
    if let Some(mut cache) = crate::fs::page::pagecache::PAGE_CACHE.try_lock() {
        cache.trim_clean_to_limit();
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
const F_GET_SEALS: usize = 1034;
const F_SET_SEALS: usize = 1035;
const F_ADD_SEALS: usize = 1033;

pub fn sys_fcntl(fd: usize, cmd: usize, arg: usize) -> SyscallResult {
    let process = crate::task::current_process();
    let mut inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
        return Err(SysError::EBADF);
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
                inner.fd_flags[new_fd] = FD_CLOEXEC_FLAG;
            } else {
                inner.fd_flags[new_fd] = 0;
            }
            Ok(new_fd)
        }
        F_GETFD => {
            // 获取 fd 标志。通常只看有没有 FD_CLOEXEC (值为 1)
            let pid = process.getpid();
            if let Some(sock) = SOCKET_MANAGER.lock().get_socket(fd, pid) {
                Ok((sock.flags & FD_CLOEXEC_FLAG) as usize)
            } else if fd < inner.fd_flags.len() {
                Ok((inner.fd_flags[fd] & FD_CLOEXEC_FLAG) as usize)
            } else {
                Ok(0)
            }
        }
        F_SETFD => {
            // 设置 fd 标志 (比如设置 FD_CLOEXEC)
            if fd < inner.fd_flags.len() {
                inner.fd_flags[fd] =
                    (inner.fd_flags[fd] & !FD_CLOEXEC_FLAG) | (arg as u32 & FD_CLOEXEC_FLAG);
            }
            // 保持 socket 层同步（部分旧代码通过 socket.flags 判断）
            let pid = process.getpid();
            if let Some(sock) = SOCKET_MANAGER.lock().get_socket_mut(fd, pid) {
                if (arg & FD_CLOEXEC_FLAG as usize) != 0 {
                    sock.flags |= FD_CLOEXEC_FLAG;
                } else {
                    sock.flags &= !FD_CLOEXEC_FLAG;
                }
            }
            if fd < inner.fd_flags.len() {
                if (arg & FD_CLOEXEC_FLAG as usize) != 0 {
                    inner.fd_flags[fd] |= FD_CLOEXEC_FLAG;
                } else {
                    inner.fd_flags[fd] &= !FD_CLOEXEC_FLAG;
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
                let file = inner.fd_table[fd].as_ref().unwrap().clone();
                Ok(file.status_flags() as usize)
            }
        }
        F_SETFL => {
            // 设置文件状态标志 (通常是用来设置 O_NONBLOCK 非阻塞模式)
            let file = inner.fd_table[fd].as_ref().unwrap().clone();
            let pid = process.getpid();
            if let Some(sock) = SOCKET_MANAGER.lock().get_socket_mut(fd, pid) {
                // 只允许修改 O_APPEND, O_NONBLOCK, O_ASYNC, O_DIRECT, O_NOATIME, O_DSYNC, O_SYNC
                let settable =
                    0o4000 | 0o2000 | 0o10000 | 0o40000 | 0o100000 | 0o1000000 | 0o4000000;
                sock.flags = (sock.flags & 1) | ((arg as u32) & settable);
            } else {
                file.set_status_flags(arg as u32);
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
        F_ADD_SEALS => {
            // 添加 memfd seal flags (只能添加，不能移除)
            let file = inner.fd_table[fd].as_ref().unwrap();
            if let Some(inode) = file.get_inode() {
                inode.set_seals(arg as u64)?;
                Ok(0)
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

        F_GET_SEALS => {
            let file = inner.fd_table[fd].as_ref().unwrap();
            if let Some(inode) = file.get_inode() {
                Ok(inode.get_seals() as usize)
            } else {
                Ok(0)
            }
        }
        F_SET_SEALS => {
            let file = inner.fd_table[fd].as_ref().unwrap();
            if let Some(inode) = file.get_inode() {
                inode.set_seals(arg as u64)?;
                Ok(0)
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
#[derive(Clone, Copy)]
pub struct IoVec {
    pub base: usize,
    pub len: usize,
}

const IOV_MAX: usize = 1024;

fn read_iovec(token: usize, iov_ptr: usize, iovcnt: usize) -> SysResult<Vec<IoVec>> {
    if iovcnt > IOV_MAX {
        return Err(SysError::EINVAL);
    }
    if iovcnt == 0 {
        return Ok(Vec::new());
    }
    if iov_ptr == 0 {
        return Err(SysError::EFAULT);
    }

    let iov_size = core::mem::size_of::<IoVec>();
    let bytes_len = iovcnt.checked_mul(iov_size).ok_or(SysError::EINVAL)?;
    let raw = read_user_bytes(token, iov_ptr as *const u8, bytes_len)?;
    let mut iovs = Vec::with_capacity(iovcnt);
    for chunk in raw.chunks_exact(iov_size) {
        let base = usize::from_ne_bytes(chunk[0..8].try_into().map_err(|_| SysError::EFAULT)?);
        let len = usize::from_ne_bytes(chunk[8..16].try_into().map_err(|_| SysError::EFAULT)?);
        iovs.push(IoVec { base, len });
    }
    Ok(iovs)
}

fn total_iov_len(iovs: &[IoVec]) -> SysResult<usize> {
    let mut total = 0usize;
    for iov in iovs {
        total = total.checked_add(iov.len).ok_or(SysError::EINVAL)?;
    }
    Ok(total)
}

//一次性将多个不连续的内存缓冲区写入同一个文件。
pub fn sys_writev(fd: usize, iov_ptr: usize, iovcnt: usize) -> SyscallResult {
    let process = crate::task::current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
        return Err(SysError::EINVAL);
    }
    let file = inner.fd_table[fd].as_ref().unwrap().clone();
    let notify_target = file.get_inode().map(|_| file.get_dentry());
    drop(inner);

    let token = crate::task::current_user_token();
    let mut total_written = 0;
    let iovs = read_iovec(token, iov_ptr, iovcnt)?;
    check_write_size_limit(file.get_offset(), total_iov_len(&iovs)?)?;

    for iov in iovs {
        if iov.len == 0 {
            continue;
        }
        let buffers = translated_byte_buffer(token, iov.base as *const u8, iov.len)?;
        let user_buffer = UserBuffer::new(buffers);
        let written = file.write(user_buffer)?;
        total_written += written;
    }
    if total_written > 0 {
        if let Some(target) = notify_target {
            let path = target.path();
            inotify_notify_path(&path, IN_MODIFY);
            fanotify_notify_dentry(target, FAN_MODIFY);
        }
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
    let notify_target = file.get_inode().map(|_| file.get_dentry());
    drop(inner);
    if let Some(target) = notify_target.as_ref() {
        fanotify_check_permission_dentry(target.clone(), FAN_ACCESS_PERM)?;
    }

    let token = crate::task::current_user_token();
    let mut total_read = 0;
    let iovs = read_iovec(token, iov_ptr, iovcnt)?;

    for iov in iovs {
        if iov.len == 0 {
            continue;
        }
        let buffers = translated_byte_buffer(token, iov.base as *mut u8, iov.len)?;
        let user_buffer = UserBuffer::new(buffers);
        let read = file.read(user_buffer)?;
        total_read += read;
    }
    Ok(total_read)
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PollFd {
    pub fd: i32,
    pub events: i16,
    pub revents: i16,
}

const POLL_MAXFDS: usize = 1024;

fn read_pollfds(token: usize, ufds: usize, nfds: usize) -> SysResult<Vec<PollFd>> {
    if nfds > POLL_MAXFDS {
        return Err(SysError::EINVAL);
    }
    if nfds == 0 {
        return Ok(Vec::new());
    }
    if ufds == 0 {
        return Err(SysError::EFAULT);
    }

    let pollfd_size = core::mem::size_of::<PollFd>();
    let bytes_len = nfds.checked_mul(pollfd_size).ok_or(SysError::EINVAL)?;
    let raw = read_user_bytes(token, ufds as *const u8, bytes_len)?;
    let mut fds = Vec::with_capacity(nfds);
    for chunk in raw.chunks_exact(pollfd_size) {
        let fd = i32::from_ne_bytes(chunk[0..4].try_into().map_err(|_| SysError::EFAULT)?);
        let events = i16::from_ne_bytes(chunk[4..6].try_into().map_err(|_| SysError::EFAULT)?);
        let revents = i16::from_ne_bytes(chunk[6..8].try_into().map_err(|_| SysError::EFAULT)?);
        fds.push(PollFd {
            fd,
            events,
            revents,
        });
    }
    Ok(fds)
}

fn write_pollfds(token: usize, ufds: usize, fds: &[PollFd]) -> SysResult<()> {
    if fds.is_empty() {
        return Ok(());
    }
    let mut raw = Vec::with_capacity(fds.len() * core::mem::size_of::<PollFd>());
    for pollfd in fds {
        raw.extend_from_slice(&pollfd.fd.to_ne_bytes());
        raw.extend_from_slice(&pollfd.events.to_ne_bytes());
        raw.extend_from_slice(&pollfd.revents.to_ne_bytes());
    }
    write_user_bytes(token, ufds as *mut u8, &raw)
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
    const POLLERR: i16 = 0x008;
    const POLLHUP: i16 = 0x010;

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
    let mut pollfds = read_pollfds(token, ufds, nfds)?;

    loop {
        ready_count = 0;
        for pollfd in pollfds.iter_mut() {
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
            let inner = process.inner_exclusive_access();
            let file = if fd < inner.fd_table.len() {
                inner.fd_table[fd].clone()
            } else {
                None
            };
            drop(inner);
            if let Some(file) = file {
                if file.is_pipe() {
                    if file.readable() && file.pipe_all_write_ends_closed() {
                        revents |= POLLHUP;
                    }
                    if file.writable() && file.pipe_all_read_ends_closed() {
                        revents |= POLLERR;
                    }
                }
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
        for pollfd in pollfds.iter() {
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
        for pollfd in pollfds.iter() {
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

    write_pollfds(token, ufds, &pollfds)?;
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
fn copy_fd_set_from_user(
    token: usize,
    fds_ptr: *mut u64,
    words: usize,
    buf: &mut [u64],
) -> SysResult<()> {
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
fn copy_fd_set_to_user(
    token: usize,
    fds_ptr: *mut u64,
    words: usize,
    buf: &[u64],
) -> SysResult<()> {
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
        if let Some(is_read_ready) = file.read_ready() {
            readable = file.readable() && is_read_ready;
            writable = file
                .write_ready()
                .map(|is_write_ready| file.writable() && is_write_ready)
                .unwrap_or_else(|| file.writable());
        } else if file.is_socket() {
            // Socket check
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
                        writable =
                            !matches!(tcp_guard.state, crate::socket::tcp::TcpSocketState::Closed);
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
                    crate::socket::SocketInner::Unix(_) => {
                        readable = false;
                        writable = true;
                    }
                }
            }
        } else if file.is_pipe() {
            if file.readable() {
                readable = file.pipe_has_data() || file.pipe_all_write_ends_closed();
            }
            if file.writable() {
                writable = file.pipe_has_space() && !file.pipe_all_read_ends_closed();
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
    if let Some(inode) = file.get_inode() {
        let mode = inode.get_mode().get_type();
        if (mode == InodeMode::CHAR || mode == InodeMode::BLOCK)
            && !matches!(request, FIOCLEX | FIONCLEX | FIONBIO | FIOASYNC)
        {
            let target = file.get_dentry();
            landlock_check_dentry(&target, LANDLOCK_ACCESS_FS_IOCTL_DEV)?;
        }
    }
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

pub fn sys_splice(
    fd_in: usize,
    off_in: usize,
    fd_out: usize,
    off_out: usize,
    len: usize,
    flags: u32,
) -> SyscallResult {
    const SPLICE_F_MOVE: u32 = 0x01;
    const SPLICE_F_NONBLOCK: u32 = 0x02;
    const SPLICE_F_MORE: u32 = 0x04;
    const SPLICE_F_GIFT: u32 = 0x08;
    const VALID_SPLICE_FLAGS: u32 =
        SPLICE_F_MOVE | SPLICE_F_NONBLOCK | SPLICE_F_MORE | SPLICE_F_GIFT;

    if flags & !VALID_SPLICE_FLAGS != 0 {
        return Err(SysError::EINVAL);
    }
    if len == 0 {
        return Ok(0);
    }

    let token = current_user_token();
    let process = current_process();
    let inner = process.inner_exclusive_access();
    let (in_file, out_file) = match (inner.fd_table.get(fd_in), inner.fd_table.get(fd_out)) {
        (Some(Some(in_f)), Some(Some(out_f))) => (in_f.clone(), out_f.clone()),
        _ => return Err(SysError::EBADF),
    };
    drop(inner);

    if !in_file.readable() || !out_file.writable() {
        return Err(SysError::EBADF);
    }
    if !in_file.is_pipe() && !out_file.is_pipe() {
        return Err(SysError::EINVAL);
    }
    if out_file.is_pipe()
        && !in_file.is_pipe()
        && (in_file.get_inode().is_none() || !in_file.writable())
    {
        return Err(SysError::EINVAL);
    }
    if in_file.is_pipe() && off_in != 0 {
        return Err(SysError::ESPIPE);
    }
    if out_file.is_pipe() && off_out != 0 {
        return Err(SysError::ESPIPE);
    }
    if out_file.is_append() {
        return Err(SysError::EINVAL);
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

    if current_in_off.checked_add(len).is_none() || current_out_off.checked_add(len).is_none() {
        return Err(SysError::EOVERFLOW);
    }
    if !out_file.is_pipe() {
        check_write_size_limit(current_out_off, len)?;
    }
    if in_file.is_pipe() && !out_file.is_pipe() && in_file.pipe_read_len() == Some(0) {
        if out_file.is_socket() || out_file.get_inode().is_none() {
            return Err(SysError::EINVAL);
        }
        return Err(SysError::EBADF);
    }

    let mut total_spliced = 0usize;
    const BUF_SIZE: usize = 4096;
    let mut buffer = [0u8; BUF_SIZE];

    while total_spliced < len {
        let chunk = (len - total_spliced).min(BUF_SIZE);
        if off_in != 0 {
            in_file.set_offset(current_in_off + total_spliced);
        }

        let read_buf: &'static mut [u8] =
            unsafe { core::slice::from_raw_parts_mut(buffer.as_mut_ptr(), chunk) };
        let read_bytes = match in_file.read(UserBuffer::new(vec![read_buf])) {
            Ok(n) => n,
            Err(e) => {
                if total_spliced > 0 {
                    break;
                }
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

        if off_out != 0 {
            out_file.set_offset(current_out_off + total_spliced);
        }
        let write_buf: &'static mut [u8] =
            unsafe { core::slice::from_raw_parts_mut(buffer.as_mut_ptr(), read_bytes) };
        let written = match out_file.write(UserBuffer::new(vec![write_buf])) {
            Ok(n) => n,
            Err(e) => {
                if total_spliced > 0 {
                    break;
                }
                if off_in != 0 {
                    in_file.set_offset(saved_in_offset);
                }
                if off_out != 0 {
                    out_file.set_offset(saved_out_offset);
                }
                return Err(e);
            }
        };
        total_spliced += written;
        if written == 0 || written < read_bytes {
            break;
        }
    }

    if off_in != 0 {
        *translated_refmut(token, off_in as *mut i64)? = (current_in_off + total_spliced) as i64;
        in_file.set_offset(saved_in_offset);
    }
    if off_out != 0 {
        *translated_refmut(token, off_out as *mut i64)? = (current_out_off + total_spliced) as i64;
        out_file.set_offset(saved_out_offset);
    }

    if total_spliced > 0 {
        if let Some(path) = in_file.get_inode().map(|_| in_file.get_dentry().path()) {
            inotify_notify_path(&path, IN_ACCESS);
            fanotify_notify_path(&path, FAN_ACCESS);
        }
        if let Some(path) = out_file.get_inode().map(|_| out_file.get_dentry().path()) {
            inotify_notify_path(&path, IN_MODIFY);
            fanotify_notify_path(&path, FAN_MODIFY);
        }
    }

    Ok(total_spliced)
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
    if current_in_off.checked_add(len).is_none() || current_out_off.checked_add(len).is_none() {
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

    if total_copied > 0 {
        out_file.flush();

        let now_us = current_time().as_micros() as i64;
        let now_sec = now_us / 1_000_000;
        let now_nsec = (now_us % 1_000_000) * 1000;
        if let Some(in_inode) = in_file.get_inode() {
            in_inode.set_atime(now_sec, now_nsec);
        }
        if let Some(out_inode) = out_file.get_inode() {
            out_inode.set_mtime(now_sec, now_nsec);
            out_inode.set_ctime(now_sec, now_nsec);
        }
        if let Some(path) = in_file.get_inode().map(|_| in_file.get_dentry().path()) {
            inotify_notify_path(&path, IN_ACCESS);
            fanotify_notify_path(&path, FAN_ACCESS);
        }
        if let Some(path) = out_file.get_inode().map(|_| out_file.get_dentry().path()) {
            inotify_notify_path(&path, IN_MODIFY);
            fanotify_notify_path(&path, FAN_MODIFY);
        }
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
    let stat = statfs_for_path(&abs_path).ok_or(SysError::ENOENT)?;
    copy_statfs_to_user(token, buf, &stat)?;
    Ok(0)
}

fn statfs_for_path(path: &str) -> Option<Statfs> {
    let sb = find_superblock_by_path(path)?;
    let mut stat = sb.statfs();
    stat.f_flags |= statfs_flags_from_mount_flags(sb.inner().flags());
    stat.f_flags |= crate::syscall::misc::mount_attr_flags_for_path(path) as i64;
    Some(stat)
}

fn pipe_statfs() -> Statfs {
    const PIPEFS_MAGIC: i64 = 0x5049_5045;
    let mut stat = Statfs::new();
    stat.f_type = PIPEFS_MAGIC;
    stat.f_bsize = 4096;
    stat.f_frsize = 4096;
    stat.f_flags = ST_VALID;
    stat
}

fn copy_statfs_to_user(token: usize, buf: *mut u8, stat: &Statfs) -> SyscallResult {
    let stat_bytes = unsafe {
        core::slice::from_raw_parts(
            stat as *const _ as *const u8,
            core::mem::size_of::<Statfs>(),
        )
    };
    copy_to_user(token, buf, stat_bytes)?;
    Ok(0)
}

pub fn sys_fstatfs(fd: usize, buf: *mut u8) -> SyscallResult {
    if buf.is_null() {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let process = current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return Err(SysError::EBADF);
    }
    let file = inner.fd_table[fd].as_ref().ok_or(SysError::EBADF)?.clone();
    drop(inner);

    let stat = if file.is_pipe() {
        pipe_statfs()
    } else if file.get_inode().is_none() {
        return Err(SysError::EINVAL);
    } else {
        let path = file.get_dentry().path();
        statfs_for_path(&path).ok_or(SysError::ENOENT)?
    };
    copy_statfs_to_user(token, buf, &stat)?;
    Ok(0)
}

pub fn encode_file_handle(ino: u64) -> [u8; FILE_HANDLE_BYTES as usize] {
    ino.to_ne_bytes()
}

pub fn sys_name_to_handle_at(
    dirfd: isize,
    pathname: *const u8,
    handle: *mut FileHandleHeader,
    mount_id: *mut i32,
    flags: u32,
) -> SyscallResult {
    const AT_EMPTY_PATH: u32 = 0x1000;
    const AT_SYMLINK_FOLLOW: u32 = 0x400;
    const AT_HANDLE_FID: u32 = 0x200;
    let allowed = AT_EMPTY_PATH | AT_SYMLINK_FOLLOW | AT_HANDLE_FID;
    if flags & !allowed != 0 {
        return Err(SysError::EINVAL);
    }
    if pathname.is_null() || handle.is_null() || mount_id.is_null() {
        return Err(SysError::EFAULT);
    }

    let token = current_user_token();
    let raw_path = translated_str(token, pathname)?;
    if raw_path.is_empty() && flags & AT_EMPTY_PATH == 0 {
        return Err(SysError::ENOENT);
    }

    let dentry = if raw_path.is_empty() {
        let process = current_process();
        let inner = process.inner_exclusive_access();
        if dirfd < 0 {
            return Err(SysError::EBADF);
        }
        let fd = dirfd as usize;
        let Some(Some(file)) = inner.fd_table.get(fd) else {
            return Err(SysError::EBADF);
        };
        file.get_dentry()
    } else {
        let start = get_start_dentry(dirfd, &raw_path)?;
        if flags & AT_SYMLINK_FOLLOW != 0 {
            resolve_path(start, &raw_path)?
        } else {
            resolve_path_nofollow_last(start, &raw_path)?
        }
    };
    let inode = dentry.get_inode().ok_or(SysError::ENOENT)?;
    let handle_bytes = unsafe { (*handle).handle_bytes };
    if handle_bytes < FILE_HANDLE_BYTES {
        unsafe {
            (*handle).handle_bytes = FILE_HANDLE_BYTES;
        }
        return Err(SysError::EOVERFLOW);
    }

    unsafe {
        (*handle).handle_bytes = FILE_HANDLE_BYTES;
        (*handle).handle_type = FILE_HANDLE_TYPE_INO;
    }
    let encoded = encode_file_handle(inode.get_ino() as u64);
    copy_to_user(
        token,
        unsafe { (handle as *mut u8).add(core::mem::size_of::<FileHandleHeader>()) },
        &encoded,
    )?;
    *translated_refmut(token, mount_id)? = 1;
    Ok(0)
}

pub fn sys_open_by_handle_at(
    _mount_fd: isize,
    _handle: *const FileHandleHeader,
    _flags: u32,
) -> SyscallResult {
    Err(SysError::EOPNOTSUPP)
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

const XATTR_NAME_MAX: usize = 255;
const XATTR_SIZE_MAX: usize = 65536;
const XATTR_LIST_MAX: usize = 65536;

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

/// Helper: get file from fd, returning EBADF if invalid.
fn fd_to_file(fd: usize) -> SysResult<Arc<dyn File + Send + Sync>> {
    let process = current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
        return Err(SysError::EBADF);
    }
    Ok(inner.fd_table[fd].as_ref().unwrap().clone())
}

/// Helper: get inode from fd, returning EBADF if invalid.
fn fd_to_inode(fd: usize) -> SysResult<Arc<dyn Inode>> {
    let file = fd_to_file(fd)?;
    file.get_inode().ok_or(SysError::EBADF)
}

/// Helper: resolve path to dentry.
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

/// syscall: fsetxattr
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
    let inode = fd_to_inode(fd)?;
    inode.setxattr(&name_str, &value_buf, flags)
}

/// syscall: fgetxattr
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

/// syscall: flistxattr
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

/// syscall: fremovexattr
pub fn sys_fremovexattr(fd: usize, name: *const u8) -> SyscallResult {
    let token = current_user_token();
    let name_str = read_xattr_name(token, name)?;
    let inode = fd_to_inode(fd)?;
    inode.removexattr(&name_str)
}

/// syscall: setxattr
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
    inode.setxattr(&name_str, &value_buf, flags)
}

/// syscall: lsetxattr (does not follow symlink on last component)
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
    inode.setxattr(&name_str, &value_buf, flags)
}

/// syscall: getxattr
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

/// syscall: lgetxattr (does not follow symlink on last component)
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

/// syscall: listxattr
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

/// syscall: llistxattr (does not follow symlink on last component)
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

/// syscall: removexattr
pub fn sys_removexattr(path: *const u8, name: *const u8) -> SyscallResult {
    let token = current_user_token();
    let name_str = read_xattr_name(token, name)?;
    let dentry = path_to_dentry(path, true)?;
    let inode = dentry.get_inode().ok_or(SysError::ENOENT)?;
    inode.removexattr(&name_str)
}

/// syscall: lremovexattr (does not follow symlink on last component)
pub fn sys_lremovexattr(path: *const u8, name: *const u8) -> SyscallResult {
    let token = current_user_token();
    let name_str = read_xattr_name(token, name)?;
    let dentry = path_to_dentry(path, false)?;
    let inode = dentry.get_inode().ok_or(SysError::ENOENT)?;
    inode.removexattr(&name_str)
}
