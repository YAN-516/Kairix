#![allow(missing_docs)]

use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::vfs::file::find_dentry;
use crate::fs::vfs::inode::InodeMode;
use crate::fs::vfs::path::{get_start_dentry, resolve_path, resolve_path_nofollow_last};
use crate::fs::vfs::{Dentry, DentryInner, File, FileInner, OpenFlags};
use crate::fs::{FS_MANAGER, find_superblock_by_path};
use crate::mm::{UserBuffer, translated_str};
use crate::syscall::fs::{
    FD_CLOEXEC_FLAG, FD_FANOTIFY_EVENT, FILE_HANDLE_BYTES, FILE_HANDLE_TYPE_INO, encode_file_handle,
};
use crate::task::{
    TaskControlBlock, block_current_and_run_next, current_process, current_task,
    current_user_token, wakeup_task,
};
use alloc::collections::{BTreeMap, VecDeque};
use alloc::format;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use spin::{Mutex, MutexGuard};

const O_NONBLOCK: u32 = 0o0004000;
const O_CLOEXEC: u32 = 0o2000000;

pub const FAN_ACCESS: u64 = 0x0000_0001;
pub const FAN_MODIFY: u64 = 0x0000_0002;
pub const FAN_ATTRIB: u64 = 0x0000_0004;
pub const FAN_CLOSE_WRITE: u64 = 0x0000_0008;
pub const FAN_CLOSE_NOWRITE: u64 = 0x0000_0010;
pub const FAN_OPEN: u64 = 0x0000_0020;
pub const FAN_MOVED_FROM: u64 = 0x0000_0040;
pub const FAN_MOVED_TO: u64 = 0x0000_0080;
pub const FAN_CREATE: u64 = 0x0000_0100;
pub const FAN_DELETE: u64 = 0x0000_0200;
pub const FAN_DELETE_SELF: u64 = 0x0000_0400;
pub const FAN_MOVE_SELF: u64 = 0x0000_0800;
pub const FAN_OPEN_EXEC: u64 = 0x0000_1000;
pub const FAN_Q_OVERFLOW: u64 = 0x0000_4000;
pub const FAN_OPEN_PERM: u64 = 0x0001_0000;
pub const FAN_ACCESS_PERM: u64 = 0x0002_0000;
pub const FAN_OPEN_EXEC_PERM: u64 = 0x0004_0000;
pub const FAN_EVENT_ON_CHILD: u64 = 0x0800_0000;
pub const FAN_RENAME: u64 = 0x1000_0000;
pub const FAN_ONDIR: u64 = 0x4000_0000;

const FAN_CLOSE: u64 = FAN_CLOSE_WRITE | FAN_CLOSE_NOWRITE;
const FAN_MOVE: u64 = FAN_MOVED_FROM | FAN_MOVED_TO;
const FAN_ALL_EVENT_BITS: u64 = FAN_ACCESS
    | FAN_MODIFY
    | FAN_ATTRIB
    | FAN_CLOSE
    | FAN_OPEN
    | FAN_MOVE
    | FAN_CREATE
    | FAN_DELETE
    | FAN_DELETE_SELF
    | FAN_MOVE_SELF
    | FAN_OPEN_EXEC
    | FAN_OPEN_PERM
    | FAN_ACCESS_PERM
    | FAN_OPEN_EXEC_PERM
    | FAN_RENAME;

const FAN_CLOEXEC: u32 = 0x0000_0001;
const FAN_NONBLOCK: u32 = 0x0000_0002;
const FAN_CLASS_CONTENT: u32 = 0x0000_0004;
const FAN_CLASS_PRE_CONTENT: u32 = 0x0000_0008;
const FAN_UNLIMITED_QUEUE: u32 = 0x0000_0010;
const FAN_UNLIMITED_MARKS: u32 = 0x0000_0020;
const FAN_ENABLE_AUDIT: u32 = 0x0000_0040;
const FAN_REPORT_PIDFD: u32 = 0x0000_0080;
const FAN_REPORT_TID: u32 = 0x0000_0100;
const FAN_REPORT_FID: u32 = 0x0000_0200;
const FAN_REPORT_DIR_FID: u32 = 0x0000_0400;
const FAN_REPORT_NAME: u32 = 0x0000_0800;
const FAN_REPORT_TARGET_FID: u32 = 0x0000_1000;
const FAN_REPORT_FD_ERROR: u32 = 0x0000_2000;
const FAN_REPORT_FLAGS: u32 = FAN_REPORT_PIDFD
    | FAN_REPORT_TID
    | FAN_REPORT_FID
    | FAN_REPORT_DIR_FID
    | FAN_REPORT_NAME
    | FAN_REPORT_TARGET_FID
    | FAN_REPORT_FD_ERROR;

const FAN_MARK_ADD: u32 = 0x0000_0001;
const FAN_MARK_REMOVE: u32 = 0x0000_0002;
const FAN_MARK_DONT_FOLLOW: u32 = 0x0000_0004;
const FAN_MARK_ONLYDIR: u32 = 0x0000_0008;
const FAN_MARK_MOUNT: u32 = 0x0000_0010;
const FAN_MARK_IGNORED_MASK: u32 = 0x0000_0020;
const FAN_MARK_IGNORED_SURV_MODIFY: u32 = 0x0000_0040;
const FAN_MARK_FLUSH: u32 = 0x0000_0080;
const FAN_MARK_FILESYSTEM: u32 = 0x0000_0100;
const FAN_MARK_EVICTABLE: u32 = 0x0000_0200;
const FAN_MARK_IGNORE: u32 = 0x0000_0400;
const FAN_MARK_ALLOWED: u32 = FAN_MARK_ADD
    | FAN_MARK_REMOVE
    | FAN_MARK_DONT_FOLLOW
    | FAN_MARK_ONLYDIR
    | FAN_MARK_MOUNT
    | FAN_MARK_IGNORED_MASK
    | FAN_MARK_IGNORED_SURV_MODIFY
    | FAN_MARK_FLUSH
    | FAN_MARK_FILESYSTEM
    | FAN_MARK_EVICTABLE
    | FAN_MARK_IGNORE;

const FANOTIFY_METADATA_VERSION: u8 = 3;
const FAN_EVENT_METADATA_LEN: usize = 24;
const FAN_EVENT_INFO_TYPE_FID: u8 = 1;
const FAN_EVENT_INFO_TYPE_DFID_NAME: u8 = 2;
const FAN_EVENT_INFO_TYPE_DFID: u8 = 3;
const FAN_EVENT_INFO_TYPE_PIDFD: u8 = 4;
const FAN_EVENT_INFO_TYPE_OLD_DFID_NAME: u8 = 10;
const FAN_EVENT_INFO_TYPE_NEW_DFID_NAME: u8 = 12;
const FAN_ALLOW: u32 = 0x01;
const FAN_DENY: u32 = 0x02;
const FAN_NOFD: i32 = -1;
const FANOTIFY_DEFAULT_MAX_USER_GROUPS: usize = 129;
const FANOTIFY_DEFAULT_MAX_USER_MARKS: usize = 8192;
const FANOTIFY_DEFAULT_MAX_QUEUED_EVENTS: usize = 16_384;
const FANOTIFY_DIRENT_EVENTS: u64 = FAN_MOVE | FAN_CREATE | FAN_DELETE | FAN_RENAME;
const FANOTIFY_FID_ONLY_EVENTS: u64 =
    FAN_ATTRIB | FANOTIFY_DIRENT_EVENTS | FAN_DELETE_SELF | FAN_MOVE_SELF;
const FANOTIFY_DIRONLY_EVENT_BITS: u64 = FANOTIFY_DIRENT_EVENTS | FAN_EVENT_ON_CHILD | FAN_ONDIR;
const FANOTIFY_REQUIRED_USER_INIT_FLAGS: u32 = FAN_REPORT_FID;
const FANOTIFY_DISALLOWED_USER_INIT_FLAGS: u32 = FAN_UNLIMITED_QUEUE
    | FAN_UNLIMITED_MARKS
    | FAN_CLASS_CONTENT
    | FAN_CLASS_PRE_CONTENT
    | FAN_REPORT_TID;
const FANOTIFY_DISALLOWED_USER_MARK_FLAGS: u32 = FAN_MARK_MOUNT | FAN_MARK_FILESYSTEM;
const FANOTIFY_PERM_EVENTS: u64 = FAN_OPEN_PERM | FAN_ACCESS_PERM | FAN_OPEN_EXEC_PERM;
const FANOTIFY_MERGEABLE_FILE_EVENTS: u64 =
    FAN_ACCESS | FAN_MODIFY | FAN_OPEN | FAN_OPEN_EXEC | FAN_CLOSE;

static FANOTIFY_MAX_USER_GROUPS: AtomicUsize = AtomicUsize::new(FANOTIFY_DEFAULT_MAX_USER_GROUPS);
static FANOTIFY_MAX_USER_MARKS: AtomicUsize = AtomicUsize::new(FANOTIFY_DEFAULT_MAX_USER_MARKS);
static FANOTIFY_MAX_QUEUED_EVENTS: AtomicUsize =
    AtomicUsize::new(FANOTIFY_DEFAULT_MAX_QUEUED_EVENTS);

pub fn fanotify_max_user_groups() -> usize {
    FANOTIFY_MAX_USER_GROUPS.load(Ordering::Relaxed)
}

pub fn fanotify_set_max_user_groups(value: usize) {
    FANOTIFY_MAX_USER_GROUPS.store(value, Ordering::Relaxed);
}

pub fn fanotify_max_user_marks() -> usize {
    FANOTIFY_MAX_USER_MARKS.load(Ordering::Relaxed)
}

pub fn fanotify_set_max_user_marks(value: usize) {
    FANOTIFY_MAX_USER_MARKS.store(value, Ordering::Relaxed);
}

pub fn fanotify_max_queued_events() -> usize {
    FANOTIFY_MAX_QUEUED_EVENTS.load(Ordering::Relaxed)
}

pub fn fanotify_set_max_queued_events(value: usize) {
    FANOTIFY_MAX_QUEUED_EVENTS.store(value, Ordering::Relaxed);
}

pub struct FanotifyFile {
    inner: Mutex<FileInner>,
    status_flags: Mutex<u32>,
    init_flags: u32,
    event_f_flags: u32,
    unprivileged: bool,
    owner_pid: usize,
    state: Arc<Mutex<FanotifyState>>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum MarkKind {
    Inode,
    Mount,
    Filesystem,
}

struct FanotifyMark {
    path: String,
    ino: usize,
    kind: MarkKind,
    mask: u64,
    ignored_mask: u64,
    ignored_survives_modify: bool,
    ignore_survives_modify: bool,
    evictable: bool,
}

#[derive(Clone)]
struct FanotifyEvent {
    id: u32,
    mask: u64,
    path: String,
    target: Option<Arc<dyn Dentry>>,
    name: String,
    ino: u64,
    parent_ino: u64,
    pid: i32,
    is_dir: bool,
    permission: Option<Arc<Mutex<PermissionWait>>>,
    fid_kind: FidKind,
    rename_has_old: bool,
    rename_new: Option<RenameInfo>,
    child_ino: Option<u64>,
}

#[derive(Clone)]
struct RenameInfo {
    parent_ino: u64,
    name: String,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum FidKind {
    Normal,
    MovedFrom,
    MovedTo,
}

struct PermissionWait {
    response: Option<bool>,
    waiters: VecDeque<Arc<TaskControlBlock>>,
}

struct FanotifyState {
    marks: Vec<FanotifyMark>,
    events: VecDeque<FanotifyEvent>,
    read_waiters: VecDeque<Arc<TaskControlBlock>>,
    poll_waiters: VecDeque<Arc<TaskControlBlock>>,
    fd_to_event: BTreeMap<i32, u32>,
    pending_permissions: BTreeMap<u32, Arc<Mutex<PermissionWait>>>,
    overflowed: bool,
}

impl FanotifyFile {
    fn new(
        dentry: Arc<dyn Dentry>,
        init_flags: u32,
        event_f_flags: u32,
        status_flags: u32,
        unprivileged: bool,
        owner_pid: usize,
    ) -> Self {
        Self {
            inner: Mutex::new(FileInner {
                offset: 0,
                dentry,
                flags: OpenFlags::empty(),
            }),
            status_flags: Mutex::new(status_flags),
            init_flags,
            event_f_flags,
            unprivileged,
            owner_pid,
            state: Arc::new(Mutex::new(FanotifyState {
                marks: Vec::new(),
                events: VecDeque::new(),
                read_waiters: VecDeque::new(),
                poll_waiters: VecDeque::new(),
                fd_to_event: BTreeMap::new(),
                pending_permissions: BTreeMap::new(),
                overflowed: false,
            })),
        }
    }

    fn is_unprivileged(&self) -> bool {
        self.unprivileged
    }

    fn add_mark(&self, path: String, ino: usize, kind: MarkKind, mask: u64, flags: u32) -> SysResult<()> {
        let mut state = self.state.lock();
        let is_ignore = flags & (FAN_MARK_IGNORED_MASK | FAN_MARK_IGNORE) != 0;
        if let Some(mark) = state
            .marks
            .iter_mut()
            .find(|mark| mark.path == path && mark.kind == kind)
        {
            if flags & FAN_MARK_EVICTABLE != 0 && !mark.evictable {
                return Err(SysError::EEXIST);
            }
            if flags & FAN_MARK_EVICTABLE == 0 && !is_ignore {
                mark.evictable = false;
            }
            if is_ignore {
                mark.ignored_mask |= mask & (FAN_ALL_EVENT_BITS | FAN_EVENT_ON_CHILD | FAN_ONDIR);
                mark.ignored_survives_modify = flags & FAN_MARK_IGNORED_SURV_MODIFY != 0;
                mark.ignore_survives_modify = flags & FAN_MARK_IGNORED_SURV_MODIFY != 0;
            } else {
                mark.mask |= mask & (FAN_ALL_EVENT_BITS | FAN_EVENT_ON_CHILD | FAN_ONDIR);
            }
            return Ok(());
        }
        state.marks.push(FanotifyMark {
            path,
            ino,
            kind,
            mask: if is_ignore {
                0
            } else {
                mask & (FAN_ALL_EVENT_BITS | FAN_EVENT_ON_CHILD | FAN_ONDIR)
            },
            ignored_mask: if is_ignore {
                mask & (FAN_ALL_EVENT_BITS | FAN_EVENT_ON_CHILD | FAN_ONDIR)
            } else {
                0
            },
            ignored_survives_modify: flags & FAN_MARK_IGNORED_SURV_MODIFY != 0,
            ignore_survives_modify: flags & FAN_MARK_IGNORED_SURV_MODIFY != 0,
            evictable: flags & FAN_MARK_EVICTABLE != 0,
        });
        Ok(())
    }

    fn has_mark(&self, path: &str, kind: MarkKind) -> bool {
        self.state
            .lock()
            .marks
            .iter()
            .any(|mark| mark.path == path && mark.kind == kind)
    }

    fn mark_count(&self) -> usize {
        self.state.lock().marks.len()
    }

    fn remove_mark(&self, path: String, kind: MarkKind, mask: u64, flags: u32) -> SysResult<()> {
        let mut state = self.state.lock();
        let is_ignore = flags & (FAN_MARK_IGNORED_MASK | FAN_MARK_IGNORE) != 0;
        let mut found = false;
        for mark in state
            .marks
            .iter_mut()
            .filter(|mark| mark.path == path && mark.kind == kind)
        {
            found = true;
            if is_ignore {
                mark.ignored_mask &=
                    !(mask & (FAN_ALL_EVENT_BITS | FAN_EVENT_ON_CHILD | FAN_ONDIR));
            } else {
                mark.mask &= !(mask & (FAN_ALL_EVENT_BITS | FAN_EVENT_ON_CHILD | FAN_ONDIR));
            }
        }
        state
            .marks
            .retain(|mark| mark.mask != 0 || mark.ignored_mask != 0);
        if found {
            Ok(())
        } else {
            Err(SysError::ENOENT)
        }
    }

    fn flush_marks(&self, kind: Option<MarkKind>) {
        let mut state = self.state.lock();
        match kind {
            Some(kind) => state.marks.retain(|mark| mark.kind != kind),
            None => state.marks.clear(),
        }
    }

    fn queue_event(&self, event: FanotifyEvent) -> bool {
        let mut state = self.state.lock();
        if state.overflowed {
            return false;
        }
        if state.events.len() >= fanotify_max_queued_events() {
            let overflow = FanotifyEvent {
                id: next_event_id(),
                mask: FAN_Q_OVERFLOW,
                path: String::new(),
                target: None,
                name: String::new(),
                ino: 0,
                parent_ino: 0,
                pid: self.report_pid(),
                is_dir: false,
                permission: None,
                fid_kind: FidKind::Normal,
                rename_has_old: false,
                rename_new: None,
                child_ino: None,
            };
            state.events.push_back(overflow);
            state.overflowed = true;
        } else {
            if event.permission.is_none() {
                for idx in 0..state.events.len() {
                    let same_event = {
                        let queued = &state.events[idx];
                        queued.permission.is_none()
                            && queued.pid == event.pid
                            && same_merge_key(self.init_flags, queued, &event)
                    };
                    if !same_event {
                        continue;
                    }
                    if state.events[idx].mask == event.mask {
                        return false;
                    }
                    if can_merge_events(self.init_flags, &state.events[idx], &event) {
                        state.events[idx].mask |= event.mask;
                        coalesce_merged_event(self.init_flags, &mut state.events, idx);
                        wake_waiters(&mut state);
                        return true;
                    }
                }
            }
            if let Some(wait) = &event.permission {
                state.pending_permissions.insert(event.id, wait.clone());
            }
            if should_insert_self_before_close(self.init_flags, &event) {
                if let Some(pos) = state
                    .events
                    .iter()
                    .position(|queued| is_close_for_self_target(queued, &event))
                {
                    state.events.insert(pos, event);
                    wake_waiters(&mut state);
                    return true;
                }
            }
            state.events.push_back(event);
        }
        wake_waiters(&mut state);
        true
    }

    fn notify_path_with_target(
        &self,
        path: &str,
        target: Option<Arc<dyn Dentry>>,
        mask: u64,
        is_dir: bool,
        fid_kind: FidKind,
    ) {
        let event_mask = mask | if is_dir { FAN_ONDIR } else { 0 };
        self.notify_event(path, target, event_mask, None, fid_kind);
    }

    fn notify_path(&self, path: &str, mask: u64, is_dir: bool, fid_kind: FidKind) {
        let target = find_dentry(path).ok();
        self.notify_path_with_target(path, target, mask, is_dir, fid_kind);
    }

    fn notify_rename(
        &self,
        old_path: &str,
        new_path: &str,
        old_target: Option<Arc<dyn Dentry>>,
        new_target: Option<Arc<dyn Dentry>>,
        is_dir: bool,
    ) {
        let event_mask = FAN_RENAME | if is_dir { FAN_ONDIR } else { 0 };
        let (old_parent, old_name) = parent_and_name(old_path);
        let (new_parent, new_name) = parent_and_name(new_path);
        let old_parent_ino = path_ino(&old_parent);
        let new_parent_ino = path_ino(&new_parent);
        let child_ino = {
            let old_ino = old_target
                .as_ref()
                .and_then(|dentry| dentry.get_inode())
                .map_or_else(|| path_ino(old_path), |inode| inode.get_ino() as u64);
            if old_ino != 0 {
                old_ino
            } else {
                new_target
                    .as_ref()
                    .and_then(|dentry| dentry.get_inode())
                    .map_or_else(|| path_ino(new_path), |inode| inode.get_ino() as u64)
            }
        };
        if self.path_has_ignored_dirent_interest(old_path, old_target.clone(), FAN_RENAME)
            || self.path_has_ignored_dirent_interest(new_path, new_target.clone(), FAN_RENAME)
        {
            return;
        }
        let old_matches =
            self.path_has_dirent_interest(old_path, old_target.clone(), is_dir, FAN_RENAME);
        let new_matches =
            self.path_has_dirent_interest(new_path, new_target.clone(), is_dir, FAN_RENAME);
        if !old_matches && !new_matches {
            return;
        }
        let rename_new = if new_matches {
            Some(RenameInfo {
                parent_ino: new_parent_ino,
                name: new_name.clone(),
            })
        } else {
            None
        };
        let mut event = {
            let mut state = self.state.lock();
            let Some(event) = build_matching_event(
                &mut state,
                if old_matches { old_path } else { new_path },
                if old_matches {
                    old_target.clone()
                } else {
                    new_target.clone()
                },
                event_mask,
                None,
                FidKind::MovedFrom,
                self.report_pid(),
                Some(if old_matches { &old_name } else { &new_name }),
                Some(if old_matches {
                    old_parent_ino
                } else {
                    new_parent_ino
                }),
                rename_new,
            ) else {
                return;
            };
            event
        };
        event.rename_has_old = old_matches;
        if child_ino != 0 {
            event.child_ino = Some(child_ino);
            event.ino = child_ino;
        }
        self.adjust_reported_mask(&mut event);
        if event.mask == 0 {
            return;
        }
        self.queue_event(event);
    }

    fn path_has_dirent_interest(
        &self,
        path: &str,
        target: Option<Arc<dyn Dentry>>,
        is_dir: bool,
        interest: u64,
    ) -> bool {
        let mut state = self.state.lock();
        build_matching_event(
            &mut state,
            path,
            target.or_else(|| find_dentry(path).ok()),
            interest | if is_dir { FAN_ONDIR } else { 0 },
            None,
            FidKind::Normal,
            self.report_pid(),
            None,
            None,
            None,
        )
        .is_some()
    }

    fn path_has_ignored_dirent_interest(
        &self,
        path: &str,
        target: Option<Arc<dyn Dentry>>,
        interest: u64,
    ) -> bool {
        let state = self.state.lock();
        let (parent_path, _) = parent_and_name(path);
        let target = target.or_else(|| find_dentry(path).ok());
        let target_ino = target
            .as_ref()
            .and_then(|dentry| dentry.get_inode())
            .map(|inode| inode.get_ino());
        let target_parent_ino = target
            .as_ref()
            .and_then(|dentry| dentry.parent())
            .and_then(|parent| parent.get_inode())
            .map(|inode| inode.get_ino());
        for mark in &state.marks {
            let (matches, _) = mark_matches(
                mark,
                path,
                &parent_path,
                target_ino,
                target_parent_ino,
                false,
                true,
            );
            if !matches {
                continue;
            }
            if mark.ignored_mask & interest != 0 {
                return true;
            }
        }
        false
    }

    fn notify_event(
        &self,
        path: &str,
        target: Option<Arc<dyn Dentry>>,
        event_mask: u64,
        permission: Option<Arc<Mutex<PermissionWait>>>,
        fid_kind: FidKind,
    ) {
        let mut event = {
            let mut state = self.state.lock();
            let Some(event) = build_matching_event(
                &mut state,
                path,
                target,
                event_mask,
                permission,
                fid_kind,
                self.report_pid(),
                None,
                None,
                None,
            ) else {
                return;
            };
            event
        };
        self.adjust_reported_mask(&mut event);
        if event.mask == 0 {
            return;
        }
        self.queue_event(event);
    }

    fn check_permission_with_target(
        &self,
        path: &str,
        target: Option<Arc<dyn Dentry>>,
        mask: u64,
    ) -> SysResult<bool> {
        let wait = Arc::new(Mutex::new(PermissionWait {
            response: None,
            waiters: VecDeque::new(),
        }));
        let mut event = {
            let mut state = self.state.lock();
            let Some(event) = build_matching_event(
                &mut state,
                path,
                target,
                mask,
                Some(wait.clone()),
                FidKind::Normal,
                self.report_pid(),
                None,
                None,
                None,
            ) else {
                return Ok(false);
            };
            event
        };
        self.adjust_reported_mask(&mut event);
        if event.mask == 0 {
            return Ok(false);
        }
        self.queue_event(event);
        loop {
            let mut wait_inner = wait.lock();
            if let Some(allow) = wait_inner.response {
                return if allow {
                    Ok(true)
                } else {
                    Err(SysError::EACCES)
                };
            }
            let task = current_task().unwrap();
            register_waiter(&mut wait_inner.waiters, task);
            drop(wait_inner);
            block_current_and_run_next();
            if current_process().inner_exclusive_access().is_zombie
                || crate::syscall::signal::should_interrupt_syscall()
            {
                return Err(SysError::EINTR);
            }
        }
    }

    fn has_events(&self) -> bool {
        !self.state.lock().events.is_empty()
    }

    fn fdinfo(&self) -> String {
        let state = self.state.lock();
        let mut info = String::new();
        for mark in &state.marks {
            let mflags = match mark.kind {
                MarkKind::Inode => 0,
                MarkKind::Mount => FAN_MARK_MOUNT,
                MarkKind::Filesystem => FAN_MARK_FILESYSTEM,
            };
            info.push_str(&format!(
                "fanotify ino:{} sdev:0 mflags:{:x} mask:{:x} ignored_mask:{:x} path:{}\n",
                mark.ino, mflags, mark.mask, mark.ignored_mask, mark.path
            ));
        }
        info
    }

    fn drop_evictable_marks(&self) {
        let mut state = self.state.lock();
        state.marks.retain(|mark| !mark.evictable);
    }

    fn adjust_reported_mask(&self, event: &mut FanotifyEvent) {
        let fid_mode =
            self.init_flags & (FAN_REPORT_FID | FAN_REPORT_DIR_FID | FAN_REPORT_NAME) != 0;
        if !fid_mode {
            event.mask &= !FAN_ONDIR;
            return;
        }

        if is_self_event(event.mask) && !event.is_dir && self.init_flags & FAN_REPORT_FID == 0 {
            event.mask = 0;
            return;
        }

        if event.is_dir && is_dot_dir_event(event.mask) {
            event.parent_ino = event.ino;
            event.name = String::from(".");
            event.child_ino = None;
        }

        if is_self_event(event.mask) {
            event.child_ino = None;
        }
    }

    fn report_pid(&self) -> i32 {
        if self.unprivileged && current_process().getpid() != self.owner_pid {
            return 0;
        }
        if self.init_flags & FAN_REPORT_TID != 0 {
            current_task()
                .and_then(|task| {
                    task.inner_exclusive_access()
                        .res
                        .as_ref()
                        .map(|res| res.tid as i32)
                })
                .unwrap_or(0)
        } else {
            current_process().getpid() as i32
        }
    }
}

impl File for FanotifyFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        self.inner.lock()
    }

    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        true
    }

    fn read(&self, mut buf: UserBuffer) -> SysResult<usize> {
        let buf_len = buf.len();
        if buf_len == 0 {
            return Ok(0);
        }

        let events = loop {
            let mut state = self.state.lock();
            let Some(front) = state.events.front() else {
                if *self.status_flags.lock() & O_NONBLOCK != 0 {
                    return Err(SysError::EAGAIN);
                }
                let task = current_task().unwrap();
                register_waiter(&mut state.read_waiters, task);
                drop(state);
                block_current_and_run_next();
                if current_process().inner_exclusive_access().is_zombie
                    || crate::syscall::signal::should_interrupt_syscall()
                {
                    return Err(SysError::EINTR);
                }
                continue;
            };
            let first_len = serialized_event_len(self.init_flags, front);
            if buf_len < first_len {
                return Err(SysError::EINVAL);
            }
            let mut used = 0;
            let mut events = Vec::new();
            while let Some(event) = state.events.front() {
                let event_len = serialized_event_len(self.init_flags, event);
                if used + event_len > buf_len {
                    break;
                }
                used += event_len;
                let event = state.events.pop_front().unwrap();
                if event.mask == FAN_Q_OVERFLOW {
                    state.overflowed = false;
                }
                events.push(event);
            }
            break events;
        };

        let mut out = Vec::new();
        for event in events {
            let (bytes, event_fd) = serialize_event(self.init_flags, self.event_f_flags, &event);
            if let (Some(wait), Some(fd)) = (&event.permission, event_fd) {
                let mut state = self.state.lock();
                state.fd_to_event.insert(fd, event.id);
                state.pending_permissions.insert(event.id, wait.clone());
            }
            out.extend_from_slice(&bytes);
        }

        let mut written = 0;
        for slice in buf.buffers.iter_mut() {
            if written >= out.len() {
                break;
            }
            let copy_len = slice.len().min(out.len() - written);
            slice[..copy_len].copy_from_slice(&out[written..written + copy_len]);
            written += copy_len;
        }
        Ok(written)
    }

    fn write(&self, buf: UserBuffer) -> SysResult<usize> {
        if buf.len() < 8 {
            return Err(SysError::EINVAL);
        }
        let mut raw = [0u8; 8];
        copy_from_user_buffer(buf, &mut raw);
        let fd = i32::from_ne_bytes([raw[0], raw[1], raw[2], raw[3]]);
        let response = u32::from_ne_bytes([raw[4], raw[5], raw[6], raw[7]]);
        if response & !(FAN_ALLOW | FAN_DENY) != 0 {
            return Err(SysError::EINVAL);
        }
        let allow = response & FAN_ALLOW != 0 && response & FAN_DENY == 0;
        let wait = {
            let mut state = self.state.lock();
            let event_id = match state.fd_to_event.remove(&fd) {
                Some(id) => id,
                None => return Err(SysError::EINVAL),
            };
            state.pending_permissions.remove(&event_id)
        };
        let Some(wait) = wait else {
            return Err(SysError::EINVAL);
        };
        let mut wait_inner = wait.lock();
        wait_inner.response = Some(allow);
        wake_waiter_queue(&mut wait_inner.waiters);
        Ok(8)
    }

    fn get_inode(&self) -> Option<Arc<dyn crate::fs::vfs::Inode>> {
        None
    }

    fn status_flags(&self) -> u32 {
        *self.status_flags.lock()
    }

    fn set_status_flags(&self, flags: u32) {
        let mut status_flags = self.status_flags.lock();
        *status_flags = (*status_flags & !O_NONBLOCK) | (flags & O_NONBLOCK);
    }

    fn read_ready(&self) -> Option<bool> {
        Some(self.has_events())
    }

    fn register_poll_waker(&self, task: Arc<TaskControlBlock>) {
        let mut state = self.state.lock();
        register_waiter(&mut state.poll_waiters, task);
    }

    fn clear_poll_waker(&self, task: &Arc<TaskControlBlock>) {
        let mut state = self.state.lock();
        clear_waiter(&mut state.poll_waiters, task);
    }

    fn wake_poll_waiters(&self) {
        let mut state = self.state.lock();
        wake_waiter_queue(&mut state.poll_waiters);
    }
}

pub struct FanotifyDentry {
    inner: DentryInner,
}

impl FanotifyDentry {
    fn new(name: &str) -> Self {
        Self {
            inner: DentryInner::new(name, None),
        }
    }
}

impl Dentry for FanotifyDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }

    fn open(self: Arc<Self>, flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        let unprivileged = current_process().inner_exclusive_access().euid != 0;
        Ok(Arc::new(FanotifyFile::new(
            self,
            0,
            0,
            flags.bits() & O_NONBLOCK,
            unprivileged,
            current_process().getpid(),
        )))
    }
}

static FANOTIFY_INSTANCES: Mutex<Vec<Weak<FanotifyFile>>> = Mutex::new(Vec::new());
static RENAMED_PATHS: Mutex<BTreeMap<usize, String>> = Mutex::new(BTreeMap::new());
static NEXT_EVENT_ID: AtomicU32 = AtomicU32::new(1);

pub fn sys_fanotify_init(flags: u32, event_f_flags: u32) -> SyscallResult {
    let allowed = FAN_CLOEXEC
        | FAN_NONBLOCK
        | FAN_CLASS_CONTENT
        | FAN_CLASS_PRE_CONTENT
        | FAN_UNLIMITED_QUEUE
        | FAN_UNLIMITED_MARKS
        | FAN_ENABLE_AUDIT
        | FAN_REPORT_FLAGS;
    if flags & !allowed != 0 {
        return Err(SysError::EINVAL);
    }
    let unprivileged = current_process().inner_exclusive_access().euid != 0;
    if unprivileged
        && (flags & FANOTIFY_DISALLOWED_USER_INIT_FLAGS != 0
            || flags & FANOTIFY_REQUIRED_USER_INIT_FLAGS != FANOTIFY_REQUIRED_USER_INIT_FLAGS)
    {
        return Err(SysError::EPERM);
    }
    if flags & FAN_CLASS_CONTENT != 0 && flags & FAN_CLASS_PRE_CONTENT != 0 {
        return Err(SysError::EINVAL);
    }
    if flags & FAN_REPORT_PIDFD != 0 && flags & FAN_REPORT_TID != 0 {
        return Err(SysError::EINVAL);
    }
    if flags & (FAN_CLASS_CONTENT | FAN_CLASS_PRE_CONTENT) != 0 && flags & FAN_REPORT_FID != 0 {
        return Err(SysError::EINVAL);
    }
    if flags & FAN_REPORT_NAME != 0 && flags & FAN_REPORT_DIR_FID == 0 {
        return Err(SysError::EINVAL);
    }
    if flags & FAN_REPORT_TARGET_FID != 0
        && flags & (FAN_REPORT_FID | FAN_REPORT_DIR_FID | FAN_REPORT_NAME)
            != (FAN_REPORT_FID | FAN_REPORT_DIR_FID | FAN_REPORT_NAME)
    {
        return Err(SysError::EINVAL);
    }

    let status_flags = if flags & FAN_NONBLOCK != 0 || event_f_flags & O_NONBLOCK != 0 {
        O_NONBLOCK
    } else {
        0
    };
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    let fd = inner.alloc_fd()?;
    let dentry = Arc::new(FanotifyDentry::new("fanotify"));
    let file = Arc::new(FanotifyFile::new(
        dentry,
        flags,
        event_f_flags,
        status_flags,
        unprivileged,
        process.getpid(),
    ));
    let mut instances = FANOTIFY_INSTANCES.lock();
    instances.retain(|weak| weak.strong_count() > 0);
    if instances.len() >= fanotify_max_user_groups() {
        return Err(SysError::EMFILE);
    }
    inner.fd_table[fd] = Some(file.clone());
    if flags & FAN_CLOEXEC != 0 && fd < inner.fd_flags.len() {
        inner.fd_flags[fd] |= FD_CLOEXEC_FLAG;
    }
    instances.push(Arc::downgrade(&file));
    Ok(fd)
}

pub fn sys_fanotify_mark(
    fanotify_fd: usize,
    flags: u32,
    mask: u64,
    dirfd: isize,
    pathname: *const u8,
) -> SyscallResult {
    if flags & !FAN_MARK_ALLOWED != 0 {
        return Err(SysError::EINVAL);
    }
    let op_count = ((flags & FAN_MARK_ADD != 0) as u32)
        + ((flags & FAN_MARK_REMOVE != 0) as u32)
        + ((flags & FAN_MARK_FLUSH != 0) as u32);
    if op_count != 1 {
        return Err(SysError::EINVAL);
    }
    let kind = mark_kind_from_flags(flags)?;
    let fanotify_file = get_fanotify_file(fanotify_fd)?;
    if fanotify_file.is_unprivileged()
        && (flags & FANOTIFY_DISALLOWED_USER_MARK_FLAGS != 0 || mask & FANOTIFY_PERM_EVENTS != 0)
    {
        return Err(SysError::EPERM);
    }
    validate_mark_request(&fanotify_file, flags, mask, kind, None)?;

    if flags & FAN_MARK_FLUSH != 0 {
        fanotify_file.flush_marks(Some(kind));
        return Ok(0);
    }
    let token = current_user_token();
    let target = if pathname.is_null() {
        let process = current_process();
        let inner = process.inner_exclusive_access();
        if dirfd < 0 {
            return Err(SysError::EBADF);
        }
        let fd = dirfd as usize;
        let Some(Some(file)) = inner.fd_table.get(fd) else {
            return Err(SysError::EBADF);
        };
        if file.get_inode().is_none() {
            return Err(SysError::EINVAL);
        }
        file.get_dentry()
    } else {
        let path = translated_str(token, pathname)?;
        let start = get_start_dentry(dirfd, &path)?;
        if flags & FAN_MARK_DONT_FOLLOW != 0 {
            resolve_path_nofollow_last(start, &path)?
        } else {
            resolve_path(start, &path)?
        }
    };
    let inode = target.get_inode().ok_or(SysError::ENOENT)?;
    let is_dir = inode.get_mode().contains(InodeMode::DIR);
    if flags & FAN_MARK_ONLYDIR != 0 && !is_dir {
        return Err(SysError::ENOTDIR);
    }
    validate_mark_request(&fanotify_file, flags, mask, kind, Some(is_dir))?;

    let path = target.path();
    let ino = inode.get_ino();
    if flags & FAN_MARK_ADD != 0 {
        if fanotify_file.init_flags & FAN_UNLIMITED_MARKS == 0
            && !fanotify_file.has_mark(&path, kind)
            && live_fanotify_mark_count() >= fanotify_max_user_marks()
        {
            return Err(SysError::ENOSPC);
        }
        let _ = fanotify_file.add_mark(path, ino, kind, mask, flags)?;
    } else {
        let _ = fanotify_file.remove_mark(path, kind, mask, flags)?;
    }
    Ok(0)
}

pub fn fanotify_fdinfo(file: &Arc<dyn File + Send + Sync>) -> Option<String> {
    find_fanotify_file(file).map(|file| file.fdinfo())
}

pub fn fanotify_notify_path(path: &str, mask: u64) {
    if fanotify_skip_path(path) {
        return;
    }
    let is_dir = path_is_dir(path);
    notify_instances(path, mask, is_dir, FidKind::Normal);
}

pub fn fanotify_notify_dentry(dentry: Arc<dyn Dentry>, mask: u64) {
    let path = fanotify_event_path_for_dentry(&dentry);
    if fanotify_skip_path(&path) {
        return;
    }
    let event_mask = mask | if dentry_is_dir(&dentry) { FAN_ONDIR } else { 0 };
    let instances = live_instances();
    for file in instances {
        file.notify_event(
            &path,
            Some(dentry.clone()),
            event_mask,
            None,
            FidKind::Normal,
        );
    }
}

pub fn fanotify_drop_evictable_marks() {
    let instances = live_instances();
    for file in instances {
        file.drop_evictable_marks();
    }
}

pub fn fanotify_notify_delete_dentry(dentry: Arc<dyn Dentry>) {
    let path = fanotify_event_path_for_dentry(&dentry);
    if fanotify_skip_path(&path) {
        return;
    }
    let event_dir_bit = if dentry_is_dir(&dentry) { FAN_ONDIR } else { 0 };
    let instances = live_instances();
    for file in instances {
        file.notify_event(
            &path,
            Some(dentry.clone()),
            FAN_DELETE | event_dir_bit,
            None,
            FidKind::Normal,
        );
        if !dentry_is_dir(&dentry) {
            file.notify_event(
                &path,
                Some(dentry.clone()),
                FAN_DELETE_SELF | event_dir_bit,
                None,
                FidKind::Normal,
            );
        }
    }
    clear_renamed_path(&path);
}

pub fn fanotify_notify_move(
    old_path: &str,
    new_path: &str,
    old_target: Option<Arc<dyn Dentry>>,
    is_dir: bool,
) {
    if fanotify_skip_path(old_path) && fanotify_skip_path(new_path) {
        return;
    }
    let new_target = find_dentry(new_path).ok();
    let instances = live_instances();
    for file in instances {
        if is_dir {
            file.notify_rename(
                old_path,
                new_path,
                old_target.clone(),
                new_target.clone(),
                is_dir,
            );
        }
        let old_move_ignored =
            file.path_has_ignored_dirent_interest(old_path, old_target.clone(), FAN_MOVED_FROM);
        if !old_move_ignored {
            file.notify_path_with_target(
                old_path,
                old_target.clone(),
                FAN_MOVED_FROM,
                is_dir,
                FidKind::MovedFrom,
            );
        }
        if !is_dir {
            file.notify_rename(
                old_path,
                new_path,
                old_target.clone(),
                new_target.clone(),
                is_dir,
            );
        }
        let moved_to_target = new_target.clone().or_else(|| old_target.clone());
        let new_move_ignored =
            file.path_has_ignored_dirent_interest(new_path, moved_to_target.clone(), FAN_MOVED_TO);
        if !new_move_ignored {
            file.notify_path_with_target(
                new_path,
                moved_to_target,
                FAN_MOVED_TO,
                is_dir,
                FidKind::MovedTo,
            );
        }
        if !is_dir {
            file.notify_path_with_target(
                old_path,
                old_target.clone(),
                FAN_MOVE_SELF,
                is_dir,
                FidKind::Normal,
            );
        }
    }
    remember_renamed_path(old_path, new_path);
}

pub fn fanotify_notify_unmount(mount_path: &str) {
    if fanotify_skip_path(mount_path) {
        return;
    }
    notify_instances(
        mount_path,
        FAN_DELETE_SELF | FAN_MOVE_SELF,
        true,
        FidKind::Normal,
    );
    clear_renamed_path(mount_path);
}

pub fn fanotify_check_permission_dentry(dentry: Arc<dyn Dentry>, mask: u64) -> SyscallResult {
    let path = fanotify_event_path_for_dentry(&dentry);
    if fanotify_skip_path(&path) {
        return Ok(0);
    }
    let instances = live_instances();
    let mut result = Ok(0);
    for file in instances {
        if result.is_ok() {
            result = file
                .check_permission_with_target(&path, Some(dentry.clone()), mask)
                .map(|_| 0);
        }
    }
    result
}

pub fn fanotify_check_exec_permission_dentry(
    dentry: Arc<dyn Dentry>,
    exec_mask: u64,
    fallback_mask: u64,
) -> SyscallResult {
    let path = fanotify_event_path_for_dentry(&dentry);
    if fanotify_skip_path(&path) {
        return Ok(0);
    }
    let instances = live_instances();
    let mut result = Ok(0);
    for file in instances {
        if result.is_err() {
            break;
        }
        match file.check_permission_with_target(&path, Some(dentry.clone()), exec_mask) {
            Ok(true) => {}
            Ok(false) => {
                result = file
                    .check_permission_with_target(&path, Some(dentry.clone()), fallback_mask)
                    .map(|_| 0);
            }
            Err(err) => result = Err(err),
        }
    }
    result
}

fn get_fanotify_file(fd: usize) -> SysResult<Arc<FanotifyFile>> {
    let process = current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return Err(SysError::EBADF);
    }
    let Some(file) = &inner.fd_table[fd] else {
        return Err(SysError::EBADF);
    };
    find_fanotify_file(file).ok_or(SysError::EINVAL)
}

fn find_fanotify_file(file: &Arc<dyn File + Send + Sync>) -> Option<Arc<FanotifyFile>> {
    let target = Arc::as_ptr(file) as *const ();
    let mut instances = FANOTIFY_INSTANCES.lock();
    let mut found = None;
    instances.retain(|weak| {
        if let Some(fanotify_file) = weak.upgrade() {
            if Arc::as_ptr(&fanotify_file) as *const () == target {
                found = Some(fanotify_file);
            }
            true
        } else {
            false
        }
    });
    found
}

fn notify_instances(path: &str, mask: u64, is_dir: bool, fid_kind: FidKind) {
    let instances = live_instances();
    for file in instances {
        file.notify_path(path, mask, is_dir, fid_kind);
    }
}

fn live_instances() -> Vec<Arc<FanotifyFile>> {
    let mut instances = FANOTIFY_INSTANCES.lock();
    let mut files = Vec::new();
    instances.retain(|weak| {
        if let Some(file) = weak.upgrade() {
            files.push(file);
            true
        } else {
            false
        }
    });
    files
}

fn live_fanotify_mark_count() -> usize {
    live_instances().iter().map(|file| file.mark_count()).sum()
}

fn fanotify_event_path_for_dentry(dentry: &Arc<dyn Dentry>) -> String {
    let Some(ino) = dentry.get_inode().map(|inode| inode.get_ino()) else {
        return dentry.path();
    };
    RENAMED_PATHS
        .lock()
        .get(&ino)
        .cloned()
        .unwrap_or_else(|| dentry.path())
}

fn dentry_is_dir(dentry: &Arc<dyn Dentry>) -> bool {
    dentry
        .get_inode()
        .is_some_and(|inode| inode.get_mode().contains(InodeMode::DIR))
}

fn remember_renamed_path(old_path: &str, new_path: &str) {
    let old_ino = path_ino(old_path);
    let ino = if old_ino != 0 {
        old_ino
    } else {
        path_ino(new_path)
    };
    if ino != 0 {
        RENAMED_PATHS
            .lock()
            .insert(ino as usize, String::from(new_path));
    }
}

fn clear_renamed_path(path: &str) {
    let ino = path_ino(path);
    if ino != 0 {
        RENAMED_PATHS.lock().remove(&(ino as usize));
    }
}

fn mark_kind_from_flags(flags: u32) -> SysResult<MarkKind> {
    let has_mount = flags & FAN_MARK_MOUNT != 0;
    let has_fs = flags & FAN_MARK_FILESYSTEM != 0;
    if has_mount && has_fs {
        return Err(SysError::EINVAL);
    }
    if has_mount {
        Ok(MarkKind::Mount)
    } else if has_fs {
        Ok(MarkKind::Filesystem)
    } else {
        Ok(MarkKind::Inode)
    }
}

fn validate_mark_request(
    file: &FanotifyFile,
    flags: u32,
    mask: u64,
    kind: MarkKind,
    target_is_dir: Option<bool>,
) -> SysResult<()> {
    let ignore = flags & (FAN_MARK_IGNORED_MASK | FAN_MARK_IGNORE);
    let is_ignore = ignore != 0;
    let is_new_ignore = flags & FAN_MARK_IGNORE != 0;
    let fid_mode = file.init_flags
        & (FAN_REPORT_FID | FAN_REPORT_DIR_FID | FAN_REPORT_NAME | FAN_REPORT_TARGET_FID)
        != 0;
    let report_name = file.init_flags & FAN_REPORT_NAME != 0;
    let report_target_fid = file.init_flags & FAN_REPORT_TARGET_FID != 0;

    if ignore == (FAN_MARK_IGNORED_MASK | FAN_MARK_IGNORE) {
        return Err(SysError::EINVAL);
    }
    if !is_ignore && !fid_mode && mask & FANOTIFY_FID_ONLY_EVENTS != 0 {
        return Err(SysError::EINVAL);
    }
    if !is_ignore && matches!(kind, MarkKind::Mount) {
        if mask & FANOTIFY_FID_ONLY_EVENTS != 0 {
            return Err(SysError::EINVAL);
        }
    }
    if mask & FAN_RENAME != 0 && !report_name {
        return Err(SysError::EINVAL);
    }
    if is_new_ignore
        && flags & FAN_MARK_IGNORED_SURV_MODIFY == 0
        && matches!(kind, MarkKind::Mount | MarkKind::Filesystem)
    {
        return Err(SysError::EINVAL);
    }
    if let Some(is_dir) = target_is_dir {
        let strict_dir_events = report_target_fid || mask & FAN_RENAME != 0 || is_new_ignore;
        if strict_dir_events
            && matches!(kind, MarkKind::Inode)
            && !is_dir
            && mask & FANOTIFY_DIRONLY_EVENT_BITS != 0
        {
            return Err(SysError::ENOTDIR);
        }
        if is_new_ignore
            && flags & FAN_MARK_IGNORED_SURV_MODIFY == 0
            && matches!(kind, MarkKind::Inode)
            && is_dir
        {
            return Err(SysError::EISDIR);
        }
    }
    Ok(())
}

fn build_matching_event(
    state: &mut FanotifyState,
    path: &str,
    target: Option<Arc<dyn Dentry>>,
    event_mask: u64,
    permission: Option<Arc<Mutex<PermissionWait>>>,
    fid_kind: FidKind,
    pid: i32,
    override_name: Option<&str>,
    override_parent_ino: Option<u64>,
    rename_new: Option<RenameInfo>,
) -> Option<FanotifyEvent> {
    let (parent_path, mut name) = parent_and_name(path);
    if let Some(override_name) = override_name {
        name = String::from(override_name);
    }
    let is_dir = event_mask & FAN_ONDIR != 0;
    let interest = event_mask & FAN_ALL_EVENT_BITS;
    let target_ino = target
        .as_ref()
        .and_then(|dentry| dentry.get_inode())
        .map(|inode| inode.get_ino());
    let target_parent_ino = target
        .as_ref()
        .and_then(|dentry| dentry.parent())
        .and_then(|parent| parent.get_inode())
        .map(|inode| inode.get_ino());
    let mut matched_mask = 0;
    let mut ignored_mask = 0;
    let mut matched_ondir = false;
    let dirent_event = interest & FANOTIFY_DIRENT_EVENTS != 0;
    for mark in &mut state.marks {
        let (matches, child_event) = mark_matches(
            mark,
            path,
            &parent_path,
            target_ino,
            target_parent_ino,
            is_self_event(interest),
            dirent_event,
        );
        if !matches {
            continue;
        }
        if interest & FAN_MODIFY != 0 && !mark.ignored_survives_modify {
            mark.ignored_mask = 0;
        }
        let mark_child_matches =
            !child_event || dirent_event || mark.mask & FAN_EVENT_ON_CHILD != 0;
        let ignore_child_matches =
            !child_event || dirent_event || mark.ignored_mask & FAN_EVENT_ON_CHILD != 0;
        let mark_dir_matches = !is_dir || mark.mask & FAN_ONDIR != 0;
        let ignore_dir_matches = !is_dir || dirent_event || mark.ignored_mask & FAN_ONDIR != 0;
        let interested = if mark_child_matches && mark_dir_matches {
            mark.mask & interest
        } else {
            0
        };
        let ignored = if ignore_child_matches && ignore_dir_matches {
            mark.ignored_mask & interest
        } else {
            0
        };
        if interested == 0 && ignored == 0 {
            continue;
        }
        if ignored != 0 {
            ignored_mask |= ignored;
        }
        matched_mask |= interested;
        if interested != 0 && mark.mask & FAN_ONDIR != 0 {
            matched_ondir = true;
        }
    }
    matched_mask &= !ignored_mask;
    if matched_mask == 0 {
        return None;
    }
    if is_dir && matched_ondir {
        matched_mask |= FAN_ONDIR;
    }
    let ino = target
        .as_ref()
        .and_then(|dentry| dentry.get_inode())
        .map_or_else(|| path_ino(path), |inode| inode.get_ino() as u64);
    let parent_ino = target
        .as_ref()
        .and_then(|dentry| dentry.parent())
        .and_then(|parent| parent.get_inode())
        .map_or_else(|| path_ino(&parent_path), |inode| inode.get_ino() as u64);
    let parent_ino = override_parent_ino.unwrap_or_else(|| {
        if is_dir && !is_dirent_event(matched_mask) && rename_new.is_none() {
            name = String::from(".");
            ino
        } else {
            parent_ino
        }
    });
    Some(FanotifyEvent {
        id: next_event_id(),
        mask: matched_mask,
        path: String::from(path),
        target,
        name,
        ino,
        parent_ino,
        pid,
        is_dir,
        permission,
        fid_kind,
        rename_has_old: false,
        rename_new,
        child_ino: if is_dirent_event(matched_mask)
            || matched_mask & (FAN_OPEN | FAN_OPEN_EXEC | FAN_CLOSE) != 0
        {
            Some(ino)
        } else {
            None
        },
    })
}

fn mark_matches(
    mark: &FanotifyMark,
    path: &str,
    parent_path: &str,
    target_ino: Option<usize>,
    parent_ino: Option<usize>,
    self_event: bool,
    dirent_event: bool,
) -> (bool, bool) {
    match mark.kind {
        MarkKind::Inode => {
            if (self_event || !dirent_event) && (target_ino == Some(mark.ino) || mark.path == path)
            {
                (true, false)
            } else if !self_event && (parent_ino == Some(mark.ino) || mark.path == parent_path) {
                (true, true)
            } else {
                (false, false)
            }
        }
        MarkKind::Mount => (path_is_at_or_below(path, &mark.path), false),
        MarkKind::Filesystem => (same_superblock_path(path, &mark.path), false),
    }
}

fn same_superblock_path(path: &str, mark_path: &str) -> bool {
    let Some(mark_sb) = find_superblock_by_path(mark_path) else {
        return false;
    };
    if find_superblock_by_path(path).is_some_and(|path_sb| Arc::ptr_eq(&path_sb, &mark_sb)) {
        return true;
    }
    let Some(path_ino) = find_dentry(path)
        .ok()
        .and_then(|dentry| dentry.get_inode())
        .map(|inode| inode.get_ino())
    else {
        return false;
    };
    let fs_mgr = FS_MANAGER.lock();
    for fstype in fs_mgr.values() {
        let supers = fstype.inner().supers.lock();
        for sb in supers.values() {
            if Arc::ptr_eq(sb, &mark_sb) && dentry_tree_contains_ino(sb.root(), path_ino) {
                return true;
            }
        }
    }
    false
}

fn dentry_tree_contains_ino(root: Arc<dyn Dentry>, ino: usize) -> bool {
    if root.get_inode().is_some_and(|inode| inode.get_ino() == ino) {
        return true;
    }
    for child in root.children().values() {
        if dentry_tree_contains_ino(child.clone(), ino) {
            return true;
        }
    }
    false
}

fn is_dirent_event(mask: u64) -> bool {
    mask & FANOTIFY_DIRENT_EVENTS != 0
}

fn is_self_event(mask: u64) -> bool {
    mask & (FAN_DELETE_SELF | FAN_MOVE_SELF) != 0
}

fn is_dirent_event_only(mask: u64) -> bool {
    mask & FAN_ALL_EVENT_BITS != 0
        && mask & !(FANOTIFY_DIRENT_EVENTS | FAN_ONDIR) == 0
        && mask & FANOTIFY_DIRENT_EVENTS != 0
}

fn is_open_close_event_only(mask: u64) -> bool {
    mask & FAN_ALL_EVENT_BITS != 0
        && mask & !(FAN_OPEN | FAN_OPEN_EXEC | FAN_CLOSE | FAN_ONDIR) == 0
        && mask & (FAN_OPEN | FAN_OPEN_EXEC | FAN_CLOSE) != 0
}

fn is_self_event_only(mask: u64) -> bool {
    mask & FAN_ALL_EVENT_BITS != 0
        && mask & !(FAN_DELETE_SELF | FAN_MOVE_SELF | FAN_ONDIR) == 0
        && mask & (FAN_DELETE_SELF | FAN_MOVE_SELF) != 0
}

fn is_dot_dir_event(mask: u64) -> bool {
    mask & FAN_ONDIR != 0 && (is_open_close_event_only(mask) || is_self_event_only(mask))
}

fn should_insert_self_before_close(init_flags: u32, event: &FanotifyEvent) -> bool {
    init_flags & FAN_REPORT_NAME != 0
        && init_flags & FAN_REPORT_TARGET_FID == 0
        && is_self_event_only(event.mask)
        && !event.is_dir
}

fn is_close_for_self_target(queued: &FanotifyEvent, event: &FanotifyEvent) -> bool {
    queued.pid == event.pid
        && queued.ino == event.ino
        && queued.mask & FAN_CLOSE_WRITE != 0
        && queued.mask & FAN_RENAME == 0
        && queued.permission.is_none()
}

fn fid_kind_merge_compatible(left: &FanotifyEvent, right: &FanotifyEvent) -> bool {
    left.fid_kind == right.fid_kind
        || (is_dirent_event_only(left.mask) && is_dirent_event_only(right.mask))
}

fn has_same_fid_records(init_flags: u32, left: &FanotifyEvent, right: &FanotifyEvent) -> bool {
    let left_child = should_report_child_fid(init_flags, left);
    let right_child = should_report_child_fid(init_flags, right);
    if left_child != right_child {
        return false;
    }
    !left_child || left.child_ino == right.child_ino
}

fn should_report_child_fid(init_flags: u32, event: &FanotifyEvent) -> bool {
    if init_flags & FAN_REPORT_FID == 0 || event.child_ino.is_none() || is_self_event(event.mask) {
        return false;
    }
    if event.mask & FAN_RENAME != 0 {
        return init_flags & FAN_REPORT_TARGET_FID != 0;
    }
    if is_dirent_event(event.mask) {
        return init_flags & FAN_REPORT_TARGET_FID != 0;
    }
    init_flags & FAN_REPORT_NAME != 0 || init_flags & FAN_REPORT_DIR_FID != 0
}

fn reported_fid_ino(init_flags: u32, event: &FanotifyEvent) -> u64 {
    if is_self_event(event.mask) && !event.is_dir {
        return event.ino;
    }
    if init_flags & FAN_REPORT_NAME != 0 {
        return event.parent_ino;
    }
    if init_flags & FAN_REPORT_DIR_FID != 0 {
        if is_dirent_event(event.mask) {
            event.parent_ino
        } else if event.is_dir {
            event.ino
        } else {
            event.parent_ino
        }
    } else if init_flags & FAN_REPORT_FID != 0
        && init_flags & FAN_REPORT_TARGET_FID == 0
        && is_dirent_event(event.mask)
    {
        event.parent_ino
    } else {
        event.ino
    }
}

fn same_merge_key(init_flags: u32, left: &FanotifyEvent, right: &FanotifyEvent) -> bool {
    if left.rename_new.is_some() || right.rename_new.is_some() {
        return false;
    }
    if is_self_event(left.mask) != is_self_event(right.mask) {
        return false;
    }
    if is_self_event(left.mask) && !left.is_dir && !right.is_dir {
        return left.ino == right.ino;
    }
    if init_flags & (FAN_REPORT_FID | FAN_REPORT_DIR_FID | FAN_REPORT_NAME) == 0 {
        return left.path == right.path
            && left.ino == right.ino
            && left.parent_ino == right.parent_ino
            && left.fid_kind == right.fid_kind;
    }
    if init_flags & FAN_REPORT_NAME != 0 {
        let same_name = left.parent_ino == right.parent_ino
            && left.name == right.name
            && has_same_fid_records(init_flags, left, right);
        if init_flags & FAN_REPORT_TARGET_FID != 0 {
            return same_name;
        }
        return same_name && fid_kind_merge_compatible(left, right);
    }
    reported_fid_ino(init_flags, left) == reported_fid_ino(init_flags, right)
        && has_same_fid_records(init_flags, left, right)
}

fn coalesce_merged_event(init_flags: u32, events: &mut VecDeque<FanotifyEvent>, mut idx: usize) {
    let mut pos = 0;
    while pos < events.len() {
        if pos == idx {
            pos += 1;
            continue;
        }
        let merge = {
            let base = &events[idx];
            let other = &events[pos];
            base.permission.is_none()
                && other.permission.is_none()
                && base.pid == other.pid
                && same_merge_key(init_flags, base, other)
                && can_merge_events(init_flags, base, other)
        };
        if merge {
            let other = events.remove(pos).unwrap();
            if pos < idx {
                idx -= 1;
            }
            events[idx].mask |= other.mask;
            pos = 0;
        } else {
            pos += 1;
        }
    }
}

fn can_merge_events(init_flags: u32, left: &FanotifyEvent, right: &FanotifyEvent) -> bool {
    let left_mask = left.mask;
    let right_mask = right.mask;
    let left_events = left_mask & FAN_ALL_EVENT_BITS;
    let right_events = right_mask & FAN_ALL_EVENT_BITS;
    if left_events == 0
        || right_events == 0
        || left_mask & FAN_RENAME != 0
        || right_mask & FAN_RENAME != 0
        || left_mask & FAN_ONDIR != right_mask & FAN_ONDIR
    {
        return false;
    }

    if is_self_event(left_mask) || is_self_event(right_mask) {
        return is_self_event_only(left_mask) && is_self_event_only(right_mask);
    }

    if is_dirent_event(left_mask) || is_dirent_event(right_mask) {
        if is_dirent_event_only(left_mask) && is_dirent_event_only(right_mask) {
            return true;
        }
        if init_flags & FAN_REPORT_TARGET_FID == 0 || !has_same_fid_records(init_flags, left, right)
        {
            return false;
        }
        let combined_events = left_events | right_events;
        let combined_open_close = combined_events & (FAN_OPEN | FAN_CLOSE);
        let combined_dirent = combined_events & FANOTIFY_DIRENT_EVENTS;
        return combined_events & !(FANOTIFY_DIRENT_EVENTS | FAN_CLOSE_WRITE) == 0
            && combined_open_close == FAN_CLOSE_WRITE
            && combined_dirent & FAN_MOVED_TO != 0;
    }

    file_events_can_merge(left_mask) && file_events_can_merge(right_mask)
}

fn file_events_can_merge(mask: u64) -> bool {
    mask & FAN_ALL_EVENT_BITS != 0
        && mask & !(FANOTIFY_MERGEABLE_FILE_EVENTS | FAN_ONDIR) == 0
        && mask & FANOTIFY_MERGEABLE_FILE_EVENTS != 0
}

fn serialized_event_len(init_flags: u32, event: &FanotifyEvent) -> usize {
    FAN_EVENT_METADATA_LEN + pidfd_record_len(init_flags) + fid_records_len(init_flags, event)
}

fn pidfd_record_len(init_flags: u32) -> usize {
    if init_flags & FAN_REPORT_PIDFD != 0 {
        8
    } else {
        0
    }
}

fn fid_records_len(init_flags: u32, event: &FanotifyEvent) -> usize {
    if init_flags & (FAN_REPORT_FID | FAN_REPORT_DIR_FID | FAN_REPORT_NAME) == 0 {
        return 0;
    }
    if event.mask & FAN_RENAME != 0 && init_flags & FAN_REPORT_NAME != 0 {
        return (if event.rename_has_old {
            fid_record_len(event.name.len() + 1)
        } else {
            0
        }) + event
            .rename_new
            .as_ref()
            .map_or(0, |new| fid_record_len(new.name.len() + 1))
            + if should_report_child_fid(init_flags, event) {
                fid_record_len(0)
            } else {
                0
            };
    }
    if is_self_event(event.mask) && !event.is_dir {
        return fid_record_len(0);
    }
    if init_flags & FAN_REPORT_NAME != 0 {
        fid_record_len(event.name.len() + 1)
            + if should_report_child_fid(init_flags, event) {
                fid_record_len(0)
            } else {
                0
            }
    } else {
        fid_record_len(0)
            + if should_report_child_fid(init_flags, event) {
                fid_record_len(0)
            } else {
                0
            }
    }
}

fn fid_record_len(name_len: usize) -> usize {
    align_up(4 + 8 + 8 + FILE_HANDLE_BYTES as usize + name_len, 8)
}

fn serialize_event(
    init_flags: u32,
    event_f_flags: u32,
    event: &FanotifyEvent,
) -> (Vec<u8>, Option<i32>) {
    let event_len = serialized_event_len(init_flags, event);
    let mut out = Vec::with_capacity(event_len);
    let (fd, event_fd) =
        if init_flags & (FAN_REPORT_FID | FAN_REPORT_DIR_FID | FAN_REPORT_NAME) != 0 {
            (FAN_NOFD, None)
        } else {
            match alloc_event_fd(event, event_f_flags) {
                Ok(fd) => (fd, Some(fd)),
                Err(_) => (FAN_NOFD, None),
            }
        };
    out.extend_from_slice(&(event_len as u32).to_ne_bytes());
    out.push(FANOTIFY_METADATA_VERSION);
    out.push(0);
    out.extend_from_slice(&(FAN_EVENT_METADATA_LEN as u16).to_ne_bytes());
    out.extend_from_slice(&event.mask.to_ne_bytes());
    out.extend_from_slice(&fd.to_ne_bytes());
    out.extend_from_slice(&event.pid.to_ne_bytes());
    append_pidfd_record(init_flags, event, &mut out);
    append_fid_records(init_flags, event, &mut out);
    while out.len() < event_len {
        out.push(0);
    }
    (out, event_fd)
}

fn append_pidfd_record(init_flags: u32, event: &FanotifyEvent, out: &mut Vec<u8>) {
    if init_flags & FAN_REPORT_PIDFD == 0 {
        return;
    }
    out.push(FAN_EVENT_INFO_TYPE_PIDFD);
    out.push(0);
    out.extend_from_slice(&8u16.to_ne_bytes());
    let pidfd = if event.pid > 0 && crate::task::pid2process(event.pid as usize).is_some() {
        alloc_pidfd(event.pid as usize).unwrap_or(FAN_NOFD)
    } else {
        FAN_NOFD
    };
    out.extend_from_slice(&pidfd.to_ne_bytes());
}

fn append_fid_records(init_flags: u32, event: &FanotifyEvent, out: &mut Vec<u8>) {
    if init_flags & (FAN_REPORT_FID | FAN_REPORT_DIR_FID | FAN_REPORT_NAME) == 0 {
        return;
    }
    if event.mask & FAN_RENAME != 0 {
        if init_flags & FAN_REPORT_NAME != 0 {
            if event.rename_has_old {
                append_fid_record(
                    out,
                    FAN_EVENT_INFO_TYPE_OLD_DFID_NAME,
                    event.parent_ino,
                    Some(event.name.as_str()),
                );
            }
            if let Some(new) = &event.rename_new {
                append_fid_record(
                    out,
                    FAN_EVENT_INFO_TYPE_NEW_DFID_NAME,
                    new.parent_ino,
                    Some(new.name.as_str()),
                );
            }
            if should_report_child_fid(init_flags, event) {
                if let Some(child_ino) = event.child_ino {
                    append_fid_record(out, FAN_EVENT_INFO_TYPE_FID, child_ino, None);
                }
            }
        } else if init_flags & FAN_REPORT_DIR_FID != 0 {
            append_fid_record(out, FAN_EVENT_INFO_TYPE_DFID, event.parent_ino, None);
        } else {
            append_fid_record(out, FAN_EVENT_INFO_TYPE_FID, event.ino, None);
        }
        return;
    }
    if is_self_event(event.mask) && !event.is_dir {
        append_fid_record(
            out,
            FAN_EVENT_INFO_TYPE_FID,
            reported_fid_ino(init_flags, event),
            None,
        );
        return;
    }
    let (info_type, ino, name) = if init_flags & FAN_REPORT_NAME != 0 {
        let info_type = match event.fid_kind {
            FidKind::Normal => FAN_EVENT_INFO_TYPE_DFID_NAME,
            FidKind::MovedFrom | FidKind::MovedTo => FAN_EVENT_INFO_TYPE_DFID_NAME,
        };
        (info_type, event.parent_ino, Some(event.name.as_str()))
    } else if init_flags & FAN_REPORT_DIR_FID != 0 {
        (
            FAN_EVENT_INFO_TYPE_DFID,
            reported_fid_ino(init_flags, event),
            None,
        )
    } else {
        (
            FAN_EVENT_INFO_TYPE_FID,
            reported_fid_ino(init_flags, event),
            None,
        )
    };
    append_fid_record(out, info_type, ino, name);
    if should_report_child_fid(init_flags, event) {
        if let Some(child_ino) = event.child_ino {
            append_fid_record(out, FAN_EVENT_INFO_TYPE_FID, child_ino, None);
        }
    }
}

fn append_fid_record(out: &mut Vec<u8>, info_type: u8, ino: u64, name: Option<&str>) {
    let name_len = name.map_or(0, |name| name.len() + 1);
    let len = fid_record_len(name_len);
    let handle = encode_file_handle(ino);
    out.push(info_type);
    out.push(0);
    out.extend_from_slice(&(len as u16).to_ne_bytes());
    out.extend_from_slice(&0u32.to_ne_bytes());
    out.extend_from_slice(&0u32.to_ne_bytes());
    out.extend_from_slice(&FILE_HANDLE_BYTES.to_ne_bytes());
    out.extend_from_slice(&FILE_HANDLE_TYPE_INO.to_ne_bytes());
    out.extend_from_slice(&handle);
    if let Some(name) = name {
        out.extend_from_slice(name.as_bytes());
        out.push(0);
    }
    while out.len() % 8 != 0 {
        out.push(0);
    }
}

fn alloc_pidfd(pid: usize) -> SysResult<i32> {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    let fd = inner.alloc_fd()?;
    inner.fd_table[fd] = Some(Arc::new(crate::fs::pidfd::PidFdFile::new(pid)));
    Ok(fd as i32)
}

fn alloc_event_fd(event: &FanotifyEvent, event_f_flags: u32) -> SysResult<i32> {
    let target = event
        .target
        .clone()
        .or_else(|| find_dentry(&event.path).ok())
        .ok_or(SysError::ENOENT)?;
    let inode = target.get_inode().ok_or(SysError::ENOENT)?;
    let accmode = event_f_flags & 0o3;
    let flags = match accmode {
        1 => OpenFlags::WRONLY,
        2 => OpenFlags::RDWR,
        _ => OpenFlags::RDONLY,
    };
    let file = target.open(flags, inode.get_mode())?;
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    let fd = inner.alloc_fd()?;
    inner.fd_table[fd] = Some(file);
    if fd < inner.fd_flags.len() {
        inner.fd_flags[fd] |= FD_FANOTIFY_EVENT;
        if event_f_flags & O_CLOEXEC != 0 {
            inner.fd_flags[fd] |= FD_CLOEXEC_FLAG;
        }
    }
    Ok(fd as i32)
}

fn parent_and_name(path: &str) -> (String, String) {
    let path = path.trim_end_matches('/');
    if path.is_empty() || path == "/" {
        return (String::from("/"), String::new());
    }
    match path.rfind('/') {
        Some(0) => (String::from("/"), String::from(&path[1..])),
        Some(idx) => (String::from(&path[..idx]), String::from(&path[idx + 1..])),
        None => (String::from("."), String::from(path)),
    }
}

fn path_is_at_or_below(path: &str, root: &str) -> bool {
    let root = root.trim_end_matches('/');
    if root == "/" || root.is_empty() {
        return path.starts_with('/');
    }
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn path_ino(path: &str) -> u64 {
    crate::fs::vfs::file::find_dentry(path)
        .ok()
        .and_then(|dentry| dentry.get_inode())
        .map_or(0, |inode| inode.get_ino() as u64)
}

fn fanotify_skip_path(path: &str) -> bool {
    path == "/proc" || path.starts_with("/proc/")
}

fn path_is_dir(path: &str) -> bool {
    crate::fs::vfs::file::find_dentry(path)
        .ok()
        .and_then(|dentry| dentry.get_inode())
        .is_some_and(|inode| inode.get_mode().contains(InodeMode::DIR))
}

fn next_event_id() -> u32 {
    let id = NEXT_EVENT_ID.fetch_add(1, Ordering::Relaxed);
    if id == 0 { 1 } else { id }
}

fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

fn copy_from_user_buffer(buf: UserBuffer, out: &mut [u8]) -> usize {
    let mut copied = 0;
    for slice in buf.buffers {
        if copied >= out.len() {
            break;
        }
        let copy_len = slice.len().min(out.len() - copied);
        out[copied..copied + copy_len].copy_from_slice(&slice[..copy_len]);
        copied += copy_len;
    }
    copied
}

fn register_waiter(waiters: &mut VecDeque<Arc<TaskControlBlock>>, task: Arc<TaskControlBlock>) {
    let task_ptr = Arc::as_ptr(&task);
    if !waiters.iter().any(|waiter| Arc::as_ptr(waiter) == task_ptr) {
        waiters.push_back(task);
    }
}

fn clear_waiter(waiters: &mut VecDeque<Arc<TaskControlBlock>>, task: &Arc<TaskControlBlock>) {
    let task_ptr = Arc::as_ptr(task);
    waiters.retain(|waiter| Arc::as_ptr(waiter) != task_ptr);
}

fn wake_waiter_queue(waiters: &mut VecDeque<Arc<TaskControlBlock>>) {
    while let Some(task) = waiters.pop_front() {
        wakeup_task(task);
    }
}

fn wake_waiters(state: &mut FanotifyState) {
    wake_waiter_queue(&mut state.read_waiters);
    wake_waiter_queue(&mut state.poll_waiters);
}
