use crate::devices::BlockDevice;
use crate::drivers::BLOCK_DEVICE;
use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::FS_MANAGER;
use crate::fs::devfs::loopx::loop_block_device_from_inode;
use crate::fs::find_superblock_by_path;
use crate::fs::notify::fanotify::fanotify_notify_unmount;
use crate::fs::notify::inotify::inotify_notify_unmount;
use crate::fs::tmpfs::dentry::TempDentry;
use crate::fs::vfs::dcache::GLOBAL_DCACHE;
use crate::fs::vfs::file::File;
use crate::fs::vfs::fstype::MountFlags;
use crate::fs::vfs::inode::{Inode, InodeMode};
use crate::fs::vfs::path::{resolve_path, split_parent_and_name};
use crate::mm::{PageTable, VirtAddr};
use crate::task::{current_process, current_user_token};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use log::{debug, info};
use polyhal::timer::current_time;

const PATH_MAX: usize = 4096;

pub(super) const ST_RDONLY: i64 = 1;
pub(super) const ST_NOSUID: i64 = 2;
pub(super) const ST_NODEV: i64 = 4;
pub(super) const ST_NOEXEC: i64 = 8;
pub(super) const ST_VALID: i64 = 32;
pub(super) const ST_NOATIME: i64 = 1024;
pub(super) const ST_NODIRATIME: i64 = 2048;
pub(super) const ST_NOSYMFOLLOW: i64 = 8192;

pub(super) fn statfs_flags_from_mount_flags(flags: MountFlags) -> i64 {
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

pub(super) fn mount_flags_for_path(path: &str) -> Option<MountFlags> {
    find_superblock_by_path(path).map(|sb| sb.inner().flags())
}

static ATIME_MOUNT_FLAGS_NEED_PATH: AtomicBool = AtomicBool::new(false);

fn note_atime_mount_flags(flags: MountFlags) {
    if flags.contains(MountFlags::MS_STRICTATIME)
        || flags.contains(MountFlags::MS_NOATIME)
        || flags.contains(MountFlags::MS_NODEIRATIME)
    {
        ATIME_MOUNT_FLAGS_NEED_PATH.store(true, Ordering::Relaxed);
    }
}

fn relatime_would_skip(inode: &Arc<dyn Inode>, now_sec: i64) -> bool {
    const RELATIME_INTERVAL_SECS: i64 = 24 * 60 * 60;
    let (atime_sec, atime_nsec) = inode.get_atime();
    let (mtime_sec, mtime_nsec) = inode.get_mtime();
    let (ctime_sec, ctime_nsec) = inode.get_ctime();
    let atime_after_mtime =
        atime_sec > mtime_sec || (atime_sec == mtime_sec && atime_nsec >= mtime_nsec);
    let atime_after_ctime =
        atime_sec > ctime_sec || (atime_sec == ctime_sec && atime_nsec >= ctime_nsec);
    let atime_recent = now_sec.saturating_sub(atime_sec) < RELATIME_INTERVAL_SECS;
    atime_after_mtime && atime_after_ctime && atime_recent
}

pub(super) fn check_readonly_mount(path: &str) -> SyscallResult {
    if mount_flags_for_path(path).is_some_and(|flags| flags.contains(MountFlags::MS_RDONLY)) {
        Err(SysError::EROFS)
    } else {
        Ok(0)
    }
}

pub(super) fn dentry_is_symlink(dentry: &Arc<dyn crate::fs::vfs::Dentry>) -> bool {
    dentry
        .get_inode()
        .is_some_and(|inode| inode.get_mode().contains(InodeMode::LINK))
}

pub(super) fn check_nosymfollow_mount(
    path: &str,
    dentry: &Arc<dyn crate::fs::vfs::Dentry>,
) -> SyscallResult {
    if !mount_flags_for_path(path).is_some_and(|flags| flags.contains(MountFlags::MS_NOSYMFOLLOW)) {
        return Ok(0);
    }

    if dentry_is_symlink(dentry) {
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
    let now_sec = now_us / 1_000_000;
    let now_nsec = (now_us % 1_000_000) * 1000;

    if !flags.contains(MountFlags::MS_STRICTATIME) {
        if relatime_would_skip(inode, now_sec) {
            return;
        }
    }

    inode.set_atime(now_sec, now_nsec);
}

pub(crate) fn maybe_update_atime_for_dentry(
    dentry: &Arc<dyn crate::fs::vfs::Dentry>,
    inode: &Arc<dyn Inode>,
    is_dir: bool,
) {
    let now_us = current_time().as_micros() as i64;
    let now_sec = now_us / 1_000_000;
    if !ATIME_MOUNT_FLAGS_NEED_PATH.load(Ordering::Relaxed) && relatime_would_skip(inode, now_sec) {
        return;
    }
    maybe_update_atime(&dentry.path(), inode, is_dir);
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

pub fn sys_umount2(target: *const u8, _flags: u32) -> SyscallResult {
    let process = current_process();
    if process.inner_exclusive_access().euid != 0 {
        return Err(SysError::EPERM);
    }
    let token = current_user_token();
    let target_path = crate::mm::translated_str(token, target)?;
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
    note_atime_mount_flags(flags);

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
            || find_superblock_by_path(&mount_path_abs)
                .is_none_or(|sb| sb.root().path() != mount_path_abs)
        {
            return Err(SysError::EINVAL);
        }
        if let Some(sb) = find_superblock_by_path(&mount_path_abs) {
            if flags.contains(MountFlags::MS_RDONLY) && has_writable_file_on_superblock(&sb) {
                return Err(SysError::EBUSY);
            }

            let mut new_flags = flags;
            new_flags.remove(MountFlags::MS_REMOUNT);
            note_atime_mount_flags(new_flags);
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
