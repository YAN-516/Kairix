use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::Ext4File;
use crate::fs::File;
use crate::fs::vfs::OpenFlags;
use alloc::collections::BTreeMap;
use alloc::ffi::CString;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use log::*;
use spin::Mutex;

use crate::fs::vfs::{Dentry, DentryInner, dcache::GLOBAL_DCACHE, inode::InodeMode, kstat::Kstat};

use crate::fs::lwext4::ext4::{dir::ExtDir, file::ExtFS};
use crate::fs::lwext4::{
    Lwext4MountGate, Lwext4Op, lwext4_err_to_sys, with_lwext4_mount_lock_op,
    with_lwext4_path_lock_op,
};
use lwext4_rust::bindings::ext4_inode_stat;

use crate::fs::lwext4::inode::fill_ext4_kstat;
use crate::fs::vfs::inode::Inode;
use crate::fs::{Ext4Inode, InodeTypes};

///remove the dentry with the name, if the flag has AT_REMOVEDIR, then remove the directory, otherwise remove the file
pub const AT_REMOVEDIR: u32 = 0x200;
///
pub const DT_UNKNOWN: u8 = 0;
///
pub const DT_DIR: u8 = 4;
///
pub const DT_REG: u8 = 8;
///
pub const DT_LNK: u8 = 10;

static EXT4_RENAME_BACKUP_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

///
pub struct Ext4Dentry {
    inner: DentryInner,
    path: String,
    /// The self_weak field is designed to allow a Dentry to correctly set the parent reference
    /// when creating child Dentry instances
    self_weak: Weak<Ext4Dentry>,
    mount_id: usize,
    mount_gate: Arc<Lwext4MountGate>,
    negative_children: Mutex<BTreeMap<String, usize>>,
    stat_cache: Mutex<Option<(usize, ext4_inode_stat)>>,
}

impl Ext4Dentry {
    const NEGATIVE_CACHE_LIMIT: usize = 256;

    ///
    pub fn new(
        name: &str,
        parent: Option<Arc<dyn Dentry>>,
        mount_gate: Arc<Lwext4MountGate>,
    ) -> Arc<dyn Dentry> {
        let path = if let Some(parent) = parent.as_ref() {
            let parent_path = parent.path();
            if parent_path == "/" {
                format!("/{}", name)
            } else {
                format!("{}/{}", parent_path, name)
            }
        } else {
            "/".to_string()
        };
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|me: &Weak<Ext4Dentry>| Self {
            inner: DentryInner::new(name, parent_weak.clone()),
            path,
            self_weak: me.clone(),
            mount_id: mount_gate.mount_id(),
            mount_gate,
            negative_children: Mutex::new(BTreeMap::new()),
            stat_cache: Mutex::new(None),
        })
    }

    fn namespace_key(&self) -> usize {
        if let Some(ino) = self
            .inner
            .inode
            .lock()
            .as_ref()
            .map(|inode| inode.get_ino())
            .filter(|ino| *ino != 0)
        {
            return ino;
        }
        self.path
            .as_bytes()
            .iter()
            .fold(0xcbf2_9ce4_8422_2325usize, |hash, byte| {
                (hash ^ (*byte as usize)).wrapping_mul(0x100_0000_01b3)
            })
    }

    fn negative_cache_hit(&self, name: &str, namespace_key: usize, generation: usize) -> bool {
        let hit = self.negative_children.lock().get(name).copied() == Some(generation);
        hit && self.mount_gate.namespace_generation(namespace_key) == generation
    }

    fn remember_negative(&self, name: &str, namespace_key: usize, generation: usize) {
        if self.mount_gate.namespace_generation(namespace_key) != generation {
            return;
        }
        let mut negative = self.negative_children.lock();
        if self.mount_gate.namespace_generation(namespace_key) != generation {
            return;
        }
        if negative.len() >= Self::NEGATIVE_CACHE_LIMIT && !negative.contains_key(name) {
            negative.clear();
        }
        negative.insert(name.to_string(), generation);
    }

    fn invalidate_negative_cache(&self) {
        self.mount_gate.note_namespace_change(self.namespace_key());
        self.negative_children.lock().clear();
    }

    fn discard_replaced_file_cache(target: &Arc<dyn Dentry>) {
        let Some(inode) = target.get_inode() else {
            return;
        };
        let Some(cache_inode_id) = inode.cache_inode_id() else {
            return;
        };
        let (discarded, kept_queued) = crate::fs::writeback::discard_closed_inode(cache_inode_id);
        // Namespace and dcache references have already been removed at this
        // point. A remaining dentry reference therefore represents an open fd
        // or a VM mapping and must retain the old inode's cache identity.
        if kept_queued == 0 && Arc::strong_count(target) == 1 {
            let cached_pages =
                crate::fs::page::pagecache::PAGE_CACHE.inode_pages_count(cache_inode_id);
            crate::fs::page::pagecache::PAGE_CACHE.remove_inode_pages(cache_inode_id);
            inode.clear_punched_holes();
            info!(
                "[EXT4_RENAME_REPLACE] inode={} discarded_writeback={} removed_pages={}",
                cache_inode_id, discarded, cached_pages
            );
        }
    }

    fn is_cargo_registry_cache_path(path: &str) -> bool {
        path.contains("/.cargo/registry/index/") && path.contains("/.cache/")
    }

    fn current_pid_and_syscall() -> (usize, Option<usize>) {
        crate::task::current_task()
            .map(|task| {
                let pid = task
                    .process
                    .upgrade()
                    .map(|process| process.getpid())
                    .unwrap_or(0);
                (pid, task.active_syscall())
            })
            .unwrap_or((0, None))
    }

    fn rename_disk_entry(is_dir: bool, old_path: &CString, new_path: &CString) -> SysResult<()> {
        if is_dir {
            ExtFS::rename(old_path, new_path)
        } else {
            ExtFS::rename_file(old_path, new_path)
        }
    }

    fn remove_disk_entry(is_dir: bool, path: &CString) -> SysResult<()> {
        if is_dir {
            ExtFS::remove_dir(path)
        } else {
            ExtFS::remove_file(path)
        }
    }

    /// Pick an absent name in the destination directory while its mount gate
    /// is held.  A replaced destination is moved here temporarily so a failed
    /// source rename can restore the original name instead of losing it.
    fn rename_backup_path(&self, dst_parent_path: &str) -> SysResult<CString> {
        for _ in 0..64 {
            let sequence = EXT4_RENAME_BACKUP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let backup_path = if dst_parent_path == "/" {
                format!("/.kairix.rename.{:x}.{:x}", self.mount_id, sequence)
            } else {
                format!(
                    "{}/.kairix.rename.{:x}.{:x}",
                    dst_parent_path.trim_end_matches('/'),
                    self.mount_id,
                    sequence
                )
            };
            let c_backup = CString::new(backup_path).map_err(|_| SysError::EINVAL)?;
            match ExtFS::inode_stat(&c_backup) {
                Err(SysError::ENOENT) => return Ok(c_backup),
                Ok(_) => continue,
                Err(err) => return Err(err),
            }
        }
        Err(SysError::EEXIST)
    }
}

impl Dentry for Ext4Dentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }
    fn name(&self) -> &str {
        &self.inner.name
    }
    fn parent(&self) -> Option<Arc<dyn Dentry>> {
        self.inner.parent.as_ref().and_then(|p| p.upgrade())
    }

    fn note_namespace_change(&self) {
        self.invalidate_negative_cache();
    }

    fn path(&self) -> String {
        self.path.clone()
    }

    fn get_stat(&self, stat: &mut Kstat) -> SysResult<()> {
        let generation = self.mount_gate.metadata_generation();
        let cached = self
            .stat_cache
            .lock()
            .as_ref()
            .filter(|(cached_generation, _)| *cached_generation == generation)
            .map(|(_, disk)| *disk);
        let disk = if let Some(disk) = cached {
            disk
        } else {
            let path = CString::new(self.path()).map_err(|_| SysError::EINVAL)?;
            let disk = ExtFS::inode_stat(&path)?;
            if self.mount_gate.metadata_generation() == generation {
                *self.stat_cache.lock() = Some((generation, disk));
            }
            disk
        };
        let inode = self.get_inode().ok_or(SysError::ENOENT)?;
        fill_ext4_kstat(inode.as_ref(), &disk, stat);
        Ok(())
    }
    /// Find a child by name using lwext4's path lookup.
    ///
    /// `ext4_inode_stat_get` ultimately uses `ext4_dir_find_entry`, including
    /// the ext4 htree when `DIR_INDEX` is enabled.  Do not enumerate the whole
    /// directory here: failed library probes in large build directories are a
    /// normal workload and a linear scan turns each `ENOENT` into O(entries).
    fn find(&self, name: &str) -> SysResult<Arc<dyn Dentry>> {
        let clean_target = name.trim_matches(|c| c == '\0' || c == ' ');
        if let Some(child) = self.inner.children.lock().get(clean_target).cloned() {
            return Ok(child);
        }
        let namespace_key = self.namespace_key();
        let generation = self.mount_gate.namespace_generation(namespace_key);
        if self.negative_cache_hit(clean_target, namespace_key, generation) {
            return Err(SysError::ENOENT);
        }

        let current_dir_path = self.path();
        trace!(
            "lookup ext4 dir [{}] for [{}]",
            current_dir_path, clean_target
        );
        let file_path = if current_dir_path == "/" {
            format!("/{}", clean_target)
        } else {
            format!(
                "{}/{}",
                current_dir_path.trim_end_matches('/'),
                clean_target
            )
        };
        let c_file_path = CString::new(file_path.as_str()).map_err(|_| SysError::EINVAL)?;
        let disk = match ExtFS::inode_stat(&c_file_path) {
            Ok(stat) => stat,
            Err(SysError::ENOENT) => {
                self.remember_negative(clean_target, namespace_key, generation);
                return Err(SysError::ENOENT);
            }
            Err(err) => return Err(err),
        };
        let file_type = InodeMode::from_bits_truncate(disk.mode).to_inode_type();
        trace!("found {} in lwext4, type: {:?}", name, file_type);
        let child_inode = Arc::new(Ext4Inode::new(
            disk.ino as usize,
            file_type,
            file_path,
            self.mount_id,
        ));
        child_inode.sync_from_disk_stat(&disk);
        let my_arc = self.self_weak.upgrade().ok_or(SysError::ENOENT)?;
        let new_dentry = Ext4Dentry::new(clean_target, Some(my_arc), self.mount_gate.clone());
        new_dentry.set_inode(child_inode);
        self.inner
            .children
            .lock()
            .insert(clean_target.to_string(), new_dentry.clone());
        Ok(new_dentry)
    }

    /// create a new dentry with the name and type, and return it, if the dentry already exists, return Err
    fn create(&self, name: &str, mode: InodeMode) -> SysResult<Arc<dyn Dentry>> {
        info!("create {:?} on Ext4Dentry: {}", mode, name);
        let parent_path = self.path();
        let target_path = format!("{}/{}", parent_path.trim_end_matches('/'), name);
        let cpath = match CString::new(target_path.clone()) {
            Ok(path) => path,
            Err(_) => {
                error!(
                    "failed to create {}: invalid path contains NUL",
                    target_path
                );
                return Err(SysError::EINVAL);
            }
        };
        match mode.get_type() {
            InodeMode::DIR => ExtFS::create(&cpath)?,
            InodeMode::FILE => ExtFS::create_file(&cpath)?,
            InodeMode::LINK => {
                // symlink 内容在创建时由 symlink() 方法处理，create 不单独处理 LINK
                warn!("create called with LINK mode, use symlink() instead");
                return Err(SysError::EINVAL);
            }
            _ => {
                warn!("unsupported inode mode: {:?}", mode);
                return Err(SysError::EINVAL);
            }
        };
        self.invalidate_negative_cache();
        // Apply permission bits (lwext4 create functions don't accept mode)
        let _ = ExtFS::mode_set(&cpath, mode.bits());
        let ino = match ExtFS::raw_inode_ino(&cpath) {
            Ok(ino) => ino,
            Err(_) => {
                let new_dentry = match self.find(name) {
                    Ok(dentry) => dentry,
                    Err(_) => {
                        error!("created {} on disk but failed to find it", target_path);
                        return Err(SysError::EIO);
                    }
                };
                self.inner
                    .children
                    .lock()
                    .insert(name.to_string(), new_dentry.clone());
                GLOBAL_DCACHE.insert(target_path, new_dentry.clone());
                return Ok(new_dentry);
            }
        };
        let my_arc = match self.self_weak.upgrade() {
            Some(arc) => arc,
            None => {
                warn!("dentry dropped while creating child: {}", name);
                return Err(SysError::ENOENT);
            }
        };
        let new_dentry = Ext4Dentry::new(name, Some(my_arc), self.mount_gate.clone());
        let inode = Arc::new(Ext4Inode::new(
            ino,
            mode.to_inode_type(),
            target_path.clone(),
            self.mount_id,
        ));
        let disk = ExtFS::inode_stat(&cpath)?;
        inode.sync_from_disk_stat(&disk);
        inode.set_mode(mode);
        new_dentry.set_inode(inode);
        self.inner
            .children
            .lock()
            .insert(name.to_string(), new_dentry.clone());
        GLOBAL_DCACHE.insert(target_path, new_dentry.clone());
        Ok(new_dentry)
    }

    /// list all the children of the current dentry
    /// return name and ino and type
    fn ls(&self) -> Vec<(String, u64, u8)> {
        info!("call ls on {}", self.path());
        let cpath = CString::new(self.path()).unwrap();
        ExtDir::open(&cpath)
            .map(|mut dir| {
                let mut entries = Vec::new();
                loop {
                    let batch = dir.next_batch(64);
                    if batch.is_empty() {
                        break;
                    }
                    for entry in batch {
                        if let Ok(name) = entry.name() {
                            let ino = entry.ino() as u64;
                            let ext4_type = entry.file_type();
                            let dt_type = match ext4_type as i32 {
                                1 => DT_REG,
                                2 => DT_DIR,
                                7 => DT_LNK,
                                _ => DT_UNKNOWN,
                            };
                            entries.push((name, ino, dt_type));
                        }
                    }
                }
                entries
            })
            .unwrap_or_default()
    }

    fn unlink(&self, name: &str, flags: u32) -> SyscallResult {
        let is_rmdir = flags & AT_REMOVEDIR != 0;
        let parent_path = self.path();
        let target_path = if parent_path == "/" {
            format!("/{}", name)
        } else {
            format!("{}/{}", parent_path, name)
        };
        let target_dentry = match GLOBAL_DCACHE.get(&target_path) {
            Some(dentry) => dentry,
            None => {
                // rename 后缓存可能失效，cache miss 时回落到底层目录查找。
                match self.find(name) {
                    Ok(dentry) => {
                        GLOBAL_DCACHE.insert(target_path.clone(), dentry.clone());
                        dentry
                    }
                    Err(_) => {
                        warn!("dentry not found for path: {}", target_path);
                        return Err(SysError::ENOENT);
                    }
                }
            }
        };
        let inode = target_dentry.get_inode().unwrap();
        let is_dir = inode.get_types() == InodeTypes::EXT4_DE_DIR;
        if is_rmdir && !is_dir {
            warn!("unlink failed: {} is not a directory", target_path);
            return Err(SysError::ENOTDIR);
        } else if !is_rmdir && is_dir {
            warn!("unlink failed: {} is a directory", target_path);
            return Err(SysError::EISDIR);
        }
        let cpath = CString::new(target_path.clone()).unwrap();
        let trace_registry = Self::is_cargo_registry_cache_path(&target_path);
        if trace_registry {
            let (pid, syscall) = Self::current_pid_and_syscall();
            error!(
                "[EXT4_REGISTRY_UNLINK] enter pid={} syscall={:?} path={} flags={:#x} inode={} nlink={}",
                pid,
                syscall,
                target_path,
                flags,
                inode.get_ino(),
                inode.get_nlink()
            );
        }
        if let Err(err) = Self::remove_disk_entry(is_rmdir, &cpath) {
            if trace_registry {
                error!(
                    "[EXT4_REGISTRY_UNLINK] failed path={} error={:?}",
                    target_path, err
                );
            }
            return Err(err);
        }
        self.invalidate_negative_cache();
        inode.dec_nlink();
        self.inner.children.lock().remove(name);
        GLOBAL_DCACHE.remove_subtree(&target_path);
        if trace_registry {
            error!(
                "[EXT4_REGISTRY_UNLINK] done path={} inode={} nlink={}",
                target_path,
                inode.get_ino(),
                inode.get_nlink()
            );
        }
        Ok(0)
    }

    fn rename(
        &self,
        src_name: &str,
        dst_parent: Arc<dyn Dentry>,
        dst_name: &str,
    ) -> SysResult<usize> {
        if src_name.is_empty()
            || dst_name.is_empty()
            || src_name == "."
            || src_name == ".."
            || dst_name == "."
            || dst_name == ".."
        {
            return Err(SysError::EINVAL);
        }

        // Capture task identity before taking the mount gate.  Looking up the
        // current task while holding a filesystem gate would invert the lock
        // order against scheduler paths that later enter the filesystem.
        let operation_context = Self::current_pid_and_syscall();
        with_lwext4_mount_lock_op(&self.mount_gate, Lwext4Op::Metadata, || {
            let old_dentry = self.find(src_name)?;
            let old_inode = old_dentry.get_inode().ok_or(SysError::ENOENT)?;
            let old_abs = old_dentry.path();
            let dst_parent_abs = dst_parent.path();
            let new_abs = if dst_parent_abs == "/" {
                format!("/{}", dst_name)
            } else {
                format!("{}/{}", dst_parent_abs, dst_name)
            };
            if old_abs == new_abs {
                return Ok(0);
            }

            let dst_parent_inode = dst_parent.get_inode().ok_or(SysError::ENOENT)?;
            if dst_parent_inode.get_mode().get_type() != InodeMode::DIR {
                return Err(SysError::ENOTDIR);
            }
            let old_is_dir = old_inode.get_mode().get_type() == InodeMode::DIR;
            if old_is_dir
                && (dst_parent_abs == old_abs
                    || dst_parent_abs.starts_with(&format!("{}/", old_abs.trim_end_matches('/'))))
            {
                return Err(SysError::EINVAL);
            }

            let existing_dentry = dst_parent.find(dst_name).ok();
            let existing_is_dir = if let Some(existing) = existing_dentry.as_ref() {
                let existing_inode = existing.get_inode().ok_or(SysError::ENOENT)?;
                if existing_inode.get_ino() == old_inode.get_ino() {
                    return Ok(0);
                }
                let existing_is_dir = existing_inode.get_mode().get_type() == InodeMode::DIR;
                if old_is_dir && !existing_is_dir {
                    return Err(SysError::ENOTDIR);
                }
                if !old_is_dir && existing_is_dir {
                    return Err(SysError::EISDIR);
                }
                if existing_is_dir && !existing.children().is_empty() {
                    return Err(SysError::ENOTEMPTY);
                }
                Some(existing_is_dir)
            } else {
                None
            };

            let c_old = CString::new(old_abs.clone()).map_err(|_| SysError::EINVAL)?;
            let c_new = CString::new(new_abs.clone()).map_err(|_| SysError::EINVAL)?;
            let trace_registry = Self::is_cargo_registry_cache_path(&old_abs)
                || Self::is_cargo_registry_cache_path(&new_abs);
            if trace_registry {
                error!(
                    "[EXT4_REGISTRY_RENAME] enter pid={} syscall={:?} old={} new={} replace={}",
                    operation_context.0,
                    operation_context.1,
                    old_abs,
                    new_abs,
                    existing_dentry.is_some()
                );
            }

            let backup = if let Some(existing_is_dir) = existing_is_dir {
                let backup = self.rename_backup_path(&dst_parent_abs)?;
                if let Err(err) = Self::rename_disk_entry(existing_is_dir, &c_new, &backup) {
                    if trace_registry {
                        error!(
                            "[EXT4_REGISTRY_RENAME] backup failed old={} new={} error={:?}",
                            old_abs, new_abs, err
                        );
                    }
                    return Err(err);
                }
                Some((backup, existing_is_dir))
            } else {
                None
            };

            if let Err(move_error) = Self::rename_disk_entry(old_is_dir, &c_old, &c_new) {
                if let Some((backup, backup_is_dir)) = backup.as_ref() {
                    if let Err(restore_error) =
                        Self::rename_disk_entry(*backup_is_dir, backup, &c_new)
                    {
                        error!(
                            "[EXT4_RENAME_ROLLBACK_FAILED] old={} new={} move_error={:?} restore_error={:?}",
                            old_abs, new_abs, move_error, restore_error
                        );
                        return Err(SysError::EIO);
                    }
                }
                if trace_registry {
                    error!(
                        "[EXT4_REGISTRY_RENAME] move failed and destination restored old={} new={} error={:?}",
                        old_abs, new_abs, move_error
                    );
                }
                return Err(move_error);
            }

            if let Some((backup, backup_is_dir)) = backup.as_ref() {
                if let Err(cleanup_error) = Self::remove_disk_entry(*backup_is_dir, backup) {
                    let move_back = Self::rename_disk_entry(old_is_dir, &c_new, &c_old);
                    let restore_destination =
                        Self::rename_disk_entry(*backup_is_dir, backup, &c_new);
                    if move_back.is_err() || restore_destination.is_err() {
                        error!(
                            "[EXT4_RENAME_ROLLBACK_FAILED] old={} new={} cleanup_error={:?} move_back={:?} restore_destination={:?}",
                            old_abs, new_abs, cleanup_error, move_back, restore_destination
                        );
                        return Err(SysError::EIO);
                    }
                    if trace_registry {
                        error!(
                            "[EXT4_REGISTRY_RENAME] cleanup failed and rename rolled back old={} new={} error={:?}",
                            old_abs, new_abs, cleanup_error
                        );
                    }
                    return Err(cleanup_error);
                }
            }

            if let Some(existing) = existing_dentry.as_ref() {
                let existing_inode = existing.get_inode().ok_or(SysError::ENOENT)?;
                existing_inode.dec_nlink();
            }

            // Detach namespace references before deciding whether the replaced
            // inode has any open-file or VM users. Keeping this inside the same
            // mount gate prevents another create from reusing the raw ext4
            // inode number before its old page-cache identity is retired.
            self.invalidate_negative_cache();
            dst_parent.note_namespace_change();
            self.inner.children.lock().remove(src_name);
            dst_parent.remove_child(dst_name);
            GLOBAL_DCACHE.remove_subtree(&old_abs);
            GLOBAL_DCACHE.remove_subtree(&new_abs);
            if let Some(existing) = existing_dentry.as_ref() {
                if existing
                    .get_inode()
                    .is_some_and(|inode| inode.get_mode().get_type() != InodeMode::DIR)
                {
                    Self::discard_replaced_file_cache(existing);
                }
            }
            if trace_registry {
                error!(
                    "[EXT4_REGISTRY_RENAME] done old={} new={} replaced={}",
                    old_abs,
                    new_abs,
                    existing_dentry.is_some()
                );
            }
            Ok(0)
        })
    }

    fn link(&self, new_name: &str, old_dentry: Arc<dyn Dentry>) -> SyscallResult {
        if old_dentry.get_inode().unwrap().get_types() != InodeTypes::EXT4_DE_REG_FILE {
            return Err(SysError::EINVAL);
        }
        let new_path = if self.path() == "/" {
            format!("/{}", new_name)
        } else {
            format!("{}/{}", self.path(), new_name)
        };
        let c_old = CString::new(old_dentry.path()).unwrap();
        let c_new = CString::new(new_path.clone()).unwrap();
        ExtFS::link(&c_old, &c_new)?;
        self.invalidate_negative_cache();
        old_dentry.get_inode().unwrap().inc_nlink();
        let new_dentry = Ext4Dentry::new(
            new_name,
            Some(self.self_weak.upgrade().unwrap()),
            self.mount_gate.clone(),
        );
        if let Some(inode) = old_dentry.get_inode() {
            new_dentry.set_inode(inode);
        }
        self.inner
            .children
            .lock()
            .insert(new_name.to_string(), new_dentry.clone());
        GLOBAL_DCACHE.insert(new_path, new_dentry);
        Ok(0)
    }
    fn symlink(&self, name: &str, target: &str) -> SyscallResult {
        let new_path = if self.path() == "/" {
            format!("/{}", name)
        } else {
            format!("{}/{}", self.path(), name)
        };
        let c_target = CString::new(target).map_err(|_| SysError::EINVAL)?;
        let c_new = CString::new(new_path.clone()).map_err(|_| SysError::EINVAL)?;
        ExtFS::symlink(&c_target, &c_new)?;
        self.invalidate_negative_cache();
        let new_dentry = Ext4Dentry::new(
            name,
            Some(self.self_weak.upgrade().unwrap()),
            self.mount_gate.clone(),
        );
        let disk = ExtFS::inode_stat(&c_new)?;
        let inode = Arc::new(Ext4Inode::new(
            disk.ino as usize,
            InodeTypes::EXT4_DE_SYMLINK,
            new_path.clone(),
            self.mount_id,
        ));
        inode.sync_from_disk_stat(&disk);
        new_dentry.set_inode(inode);
        self.inner
            .children
            .lock()
            .insert(name.to_string(), new_dentry.clone());
        GLOBAL_DCACHE.insert(new_path, new_dentry);
        Ok(0)
    }
    fn mknod(&self, name: &str, mode: InodeMode, dev: u32) -> SyscallResult {
        let parent_path = self.path();
        let target_path = format!("{}/{}", parent_path.trim_end_matches('/'), name);
        let cpath = match CString::new(target_path.clone()) {
            Ok(path) => path,
            Err(_) => {
                error!("failed to mknod {}: invalid path contains NUL", target_path);
                return Err(SysError::EINVAL);
            }
        };

        let filetype = match mode.get_type() {
            InodeMode::CHAR => InodeTypes::EXT4_DE_CHRDEV,
            InodeMode::BLOCK => InodeTypes::EXT4_DE_BLKDEV,
            InodeMode::FIFO => InodeTypes::EXT4_DE_FIFO,
            InodeMode::SOCKET => InodeTypes::EXT4_DE_SOCK,
            _ => {
                warn!("mknod called with unsupported mode: {:?}", mode);
                return Err(SysError::EINVAL);
            }
        };
        let filetype_i32 = filetype.clone() as i32;

        let err = with_lwext4_path_lock_op(&target_path, Lwext4Op::Metadata, || unsafe {
            lwext4_rust::bindings::ext4_mknod(cpath.as_ptr(), filetype_i32, dev)
        })?;
        if err != 0 {
            warn!(
                "ext4_mknod failed: path = {}, filetype = {:?}, dev = {}, error = {}",
                target_path, filetype, dev, err
            );
            return Err(lwext4_err_to_sys(err));
        }
        self.invalidate_negative_cache();

        // Apply permission bits
        let _ = ExtFS::mode_set(&cpath, mode.bits());

        let new_dentry = match self.find(name) {
            Ok(dentry) => dentry,
            Err(_) => {
                error!("mknod {} on disk but failed to find it", target_path);
                return Err(SysError::EIO);
            }
        };
        if let Some(inode) = new_dentry.get_inode() {
            inode.set_rdev(dev as usize);
        }
        self.inner
            .children
            .lock()
            .insert(name.to_string(), new_dentry.clone());
        GLOBAL_DCACHE.insert(target_path, new_dentry.clone());
        Ok(0)
    }
    fn open(self: Arc<Self>, flags: OpenFlags, mode: InodeMode) -> SysResult<Arc<dyn File>> {
        let (readable, writable) = flags.read_write();
        let types = mode.to_inode_type();
        Ok(Arc::new(Ext4File::new(
            readable, writable, self, types, flags,
        )?))
    }
}
