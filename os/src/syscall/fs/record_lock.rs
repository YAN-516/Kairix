use super::read_user_bytes;
use crate::error::{SysError, SyscallResult};
use crate::fs::File;
use crate::mm::copy_to_user;
use crate::sync::SpinNoIrqLock;
use crate::task::{TaskControlBlock, current_process, current_task};
use alloc::collections::BTreeMap;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use lazy_static::lazy_static;

pub(super) const F_GETLK: usize = 5;
pub(super) const F_SETLK: usize = 6;
pub(super) const F_SETLKW: usize = 7;

const F_RDLCK: i16 = 0;
const F_WRLCK: i16 = 1;
const F_UNLCK: i16 = 2;
const SEEK_SET: i16 = 0;
const SEEK_CUR: i16 = 1;
const SEEK_END: i16 = 2;
const FLOCK64_SIZE: usize = 32;
const OFFSET_MAX: u64 = i64::MAX as u64;

#[derive(Clone, Copy, Debug)]
struct UserFlock {
    lock_type: i16,
    whence: i16,
    start: i64,
    len: i64,
    pid: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ByteRange {
    start: u64,
    end: u64,
}

#[derive(Clone, Copy, Debug)]
struct PosixLock {
    owner_pid: usize,
    lock_type: i16,
    range: ByteRange,
}

#[derive(Clone, Copy, Debug)]
struct FlockLock {
    owner_file: usize,
    exclusive: bool,
}

#[derive(Default)]
struct InodeLocks {
    locks: Vec<PosixLock>,
    waiters: Vec<Weak<TaskControlBlock>>,
    flock_locks: Vec<FlockLock>,
    flock_waiters: Vec<Weak<TaskControlBlock>>,
}

#[derive(Default)]
struct RecordLockTable {
    inodes: BTreeMap<usize, InodeLocks>,
}

lazy_static! {
    static ref RECORD_LOCKS: SpinNoIrqLock<RecordLockTable> =
        SpinNoIrqLock::new(RecordLockTable::default());
}

fn read_flock(arg: usize) -> Result<UserFlock, SysError> {
    if arg == 0 {
        return Err(SysError::EFAULT);
    }
    let bytes = read_user_bytes(
        crate::task::current_user_token(),
        arg as *const u8,
        FLOCK64_SIZE,
    )?;
    Ok(UserFlock {
        lock_type: i16::from_ne_bytes(bytes[0..2].try_into().map_err(|_| SysError::EFAULT)?),
        whence: i16::from_ne_bytes(bytes[2..4].try_into().map_err(|_| SysError::EFAULT)?),
        start: i64::from_ne_bytes(bytes[8..16].try_into().map_err(|_| SysError::EFAULT)?),
        len: i64::from_ne_bytes(bytes[16..24].try_into().map_err(|_| SysError::EFAULT)?),
        pid: i32::from_ne_bytes(bytes[24..28].try_into().map_err(|_| SysError::EFAULT)?),
    })
}

fn write_flock(arg: usize, flock: UserFlock) -> SyscallResult {
    let mut bytes = [0u8; FLOCK64_SIZE];
    bytes[0..2].copy_from_slice(&flock.lock_type.to_ne_bytes());
    bytes[2..4].copy_from_slice(&flock.whence.to_ne_bytes());
    bytes[8..16].copy_from_slice(&flock.start.to_ne_bytes());
    bytes[16..24].copy_from_slice(&flock.len.to_ne_bytes());
    bytes[24..28].copy_from_slice(&flock.pid.to_ne_bytes());
    copy_to_user(crate::task::current_user_token(), arg as *mut u8, &bytes)?;
    Ok(0)
}

fn inode_key(file: &Arc<dyn File + Send + Sync>) -> Result<usize, SysError> {
    file.cache_inode_id().ok_or(SysError::EBADF)
}

fn open_file_description_key(file: &Arc<dyn File + Send + Sync>) -> usize {
    Arc::as_ptr(file) as *const () as usize
}

fn inode_locks_empty(inode: &InodeLocks) -> bool {
    inode.locks.is_empty()
        && inode.waiters.is_empty()
        && inode.flock_locks.is_empty()
        && inode.flock_waiters.is_empty()
}

fn normalize_range(
    file: &Arc<dyn File + Send + Sync>,
    flock: UserFlock,
) -> Result<ByteRange, SysError> {
    let base = match flock.whence {
        SEEK_SET => 0i128,
        SEEK_CUR => file.get_offset() as i128,
        SEEK_END => file.get_inode().ok_or(SysError::EBADF)?.get_size() as i128,
        _ => return Err(SysError::EINVAL),
    };
    let anchor = base
        .checked_add(flock.start as i128)
        .ok_or(SysError::EOVERFLOW)?;
    if !(0..=i64::MAX as i128).contains(&anchor) {
        return Err(SysError::EINVAL);
    }

    if flock.len == 0 {
        return Ok(ByteRange {
            start: anchor as u64,
            end: OFFSET_MAX,
        });
    }
    if flock.len > 0 {
        let end = anchor
            .checked_add(flock.len as i128 - 1)
            .ok_or(SysError::EOVERFLOW)?;
        if end > i64::MAX as i128 {
            return Err(SysError::EOVERFLOW);
        }
        return Ok(ByteRange {
            start: anchor as u64,
            end: end as u64,
        });
    }

    let start = anchor
        .checked_add(flock.len as i128)
        .ok_or(SysError::EOVERFLOW)?;
    let end = anchor - 1;
    if start < 0 || end < start {
        return Err(SysError::EINVAL);
    }
    Ok(ByteRange {
        start: start as u64,
        end: end as u64,
    })
}

fn overlaps(left: ByteRange, right: ByteRange) -> bool {
    left.start <= right.end && right.start <= left.end
}

fn conflicts(existing: PosixLock, owner_pid: usize, lock_type: i16, range: ByteRange) -> bool {
    existing.owner_pid != owner_pid
        && overlaps(existing.range, range)
        && (existing.lock_type == F_WRLCK || lock_type == F_WRLCK)
}

fn first_conflict(
    locks: &[PosixLock],
    owner_pid: usize,
    lock_type: i16,
    range: ByteRange,
) -> Option<PosixLock> {
    locks
        .iter()
        .copied()
        .filter(|lock| conflicts(*lock, owner_pid, lock_type, range))
        .min_by_key(|lock| lock.range.start)
}

fn replace_owner_range(
    locks: &mut Vec<PosixLock>,
    owner_pid: usize,
    lock_type: i16,
    range: ByteRange,
) {
    let mut updated = Vec::with_capacity(locks.len() + 2);
    for lock in locks.drain(..) {
        if lock.owner_pid != owner_pid || !overlaps(lock.range, range) {
            updated.push(lock);
            continue;
        }
        if lock.range.start < range.start {
            updated.push(PosixLock {
                range: ByteRange {
                    start: lock.range.start,
                    end: range.start - 1,
                },
                ..lock
            });
        }
        if lock.range.end > range.end {
            updated.push(PosixLock {
                range: ByteRange {
                    start: range.end + 1,
                    end: lock.range.end,
                },
                ..lock
            });
        }
    }
    if lock_type != F_UNLCK {
        updated.push(PosixLock {
            owner_pid,
            lock_type,
            range,
        });
    }
    updated.sort_by_key(|lock| (lock.owner_pid, lock.lock_type, lock.range.start));

    let mut merged: Vec<PosixLock> = Vec::with_capacity(updated.len());
    for lock in updated {
        if let Some(last) = merged.last_mut() {
            let adjacent = last.range.end == OFFSET_MAX || last.range.end + 1 >= lock.range.start;
            if last.owner_pid == lock.owner_pid && last.lock_type == lock.lock_type && adjacent {
                last.range.end = last.range.end.max(lock.range.end);
                continue;
            }
        }
        merged.push(lock);
    }
    *locks = merged;
}

fn register_waiter(inode: &mut InodeLocks, task: &Arc<TaskControlBlock>) {
    inode.waiters.retain(|waiter| waiter.strong_count() != 0);
    if !inode
        .waiters
        .iter()
        .filter_map(Weak::upgrade)
        .any(|waiter| Arc::ptr_eq(&waiter, task))
    {
        inode.waiters.push(Arc::downgrade(task));
    }
}

fn remove_waiter(inode_key: usize, task: &Arc<TaskControlBlock>) {
    let mut table = RECORD_LOCKS.lock();
    if let Some(inode) = table.inodes.get_mut(&inode_key) {
        inode.waiters.retain(|waiter| {
            waiter
                .upgrade()
                .is_some_and(|waiter| !Arc::ptr_eq(&waiter, task))
        });
        if inode_locks_empty(inode) {
            table.inodes.remove(&inode_key);
        }
    }
}

fn take_waiters(inode: &mut InodeLocks) -> Vec<Arc<TaskControlBlock>> {
    inode
        .waiters
        .drain(..)
        .filter_map(|waiter| waiter.upgrade())
        .collect()
}

fn wake_waiters(waiters: Vec<Arc<TaskControlBlock>>) {
    for waiter in waiters {
        crate::task::wakeup_task(waiter);
    }
}

fn register_flock_waiter(inode: &mut InodeLocks, task: &Arc<TaskControlBlock>) {
    inode
        .flock_waiters
        .retain(|waiter| waiter.strong_count() != 0);
    if !inode
        .flock_waiters
        .iter()
        .filter_map(Weak::upgrade)
        .any(|waiter| Arc::ptr_eq(&waiter, task))
    {
        inode.flock_waiters.push(Arc::downgrade(task));
    }
}

fn remove_flock_waiter(inode_key: usize, task: &Arc<TaskControlBlock>) {
    let mut table = RECORD_LOCKS.lock();
    if let Some(inode) = table.inodes.get_mut(&inode_key) {
        inode.flock_waiters.retain(|waiter| {
            waiter
                .upgrade()
                .is_some_and(|waiter| !Arc::ptr_eq(&waiter, task))
        });
        if inode_locks_empty(inode) {
            table.inodes.remove(&inode_key);
        }
    }
}

fn take_flock_waiters(inode: &mut InodeLocks) -> Vec<Arc<TaskControlBlock>> {
    inode
        .flock_waiters
        .drain(..)
        .filter_map(|waiter| waiter.upgrade())
        .collect()
}

fn flock_conflicts(locks: &[FlockLock], owner_file: usize, requested_exclusive: bool) -> bool {
    locks
        .iter()
        .any(|lock| lock.owner_file != owner_file && (lock.exclusive || requested_exclusive))
}

/// Implement Linux BSD-style whole-file locks. Unlike POSIX record locks,
/// these locks are owned by the open file description and therefore survive
/// `dup()` and `fork()` until the last inherited descriptor is closed.
pub(crate) fn sys_flock(fd: usize, operation: usize) -> SyscallResult {
    const LOCK_SH: usize = 1;
    const LOCK_EX: usize = 2;
    const LOCK_NB: usize = 4;
    const LOCK_UN: usize = 8;

    if operation & !(LOCK_SH | LOCK_EX | LOCK_NB | LOCK_UN) != 0 {
        return Err(SysError::EINVAL);
    }
    let command = operation & !LOCK_NB;
    if !matches!(command, LOCK_SH | LOCK_EX | LOCK_UN) {
        return Err(SysError::EINVAL);
    }

    let file = {
        let process = current_process();
        let inner = process.inner_exclusive_access();
        inner
            .fd_table
            .get(fd)
            .and_then(|file| file.as_ref())
            .cloned()
            .ok_or(SysError::EBADF)?
    };
    let key = inode_key(&file)?;
    let owner_file = open_file_description_key(&file);

    if command == LOCK_UN {
        let waiters = {
            let mut table = RECORD_LOCKS.lock();
            let Some(inode) = table.inodes.get_mut(&key) else {
                return Ok(0);
            };
            let old_len = inode.flock_locks.len();
            inode
                .flock_locks
                .retain(|lock| lock.owner_file != owner_file);
            let waiters = if inode.flock_locks.len() != old_len {
                take_flock_waiters(inode)
            } else {
                Vec::new()
            };
            if inode_locks_empty(inode) {
                table.inodes.remove(&key);
            }
            waiters
        };
        wake_waiters(waiters);
        return Ok(0);
    }

    let requested_exclusive = command == LOCK_EX;
    let nonblocking = operation & LOCK_NB != 0;
    let task = current_task().ok_or(SysError::ESRCH)?;

    // Linux flock conversions are deliberately non-atomic: an existing lock
    // is removed before the new mode is attempted. Keeping a shared lock while
    // waiting for an exclusive conversion would deadlock two simultaneous
    // converters forever.
    let conversion_waiters = {
        let mut table = RECORD_LOCKS.lock();
        if let Some(inode) = table.inodes.get_mut(&key) {
            let converting = inode
                .flock_locks
                .iter()
                .any(|lock| lock.owner_file == owner_file && lock.exclusive != requested_exclusive);
            if converting {
                inode
                    .flock_locks
                    .retain(|lock| lock.owner_file != owner_file);
                take_flock_waiters(inode)
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        }
    };
    wake_waiters(conversion_waiters);

    loop {
        let mut table = RECORD_LOCKS.lock();
        let inode = table.inodes.entry(key).or_default();
        if !flock_conflicts(&inode.flock_locks, owner_file, requested_exclusive) {
            inode
                .flock_locks
                .retain(|lock| lock.owner_file != owner_file);
            inode.flock_locks.push(FlockLock {
                owner_file,
                exclusive: requested_exclusive,
            });
            let waiters = take_flock_waiters(inode);
            drop(table);
            wake_waiters(waiters);
            // Close can race an in-flight flock syscall in another thread. If
            // the descriptor disappeared while this syscall held its Arc,
            // release the newly installed orphan lock before returning.
            release_file_description_flock_if_unreferenced(&file);
            return Ok(0);
        }
        if nonblocking {
            return Err(SysError::EAGAIN);
        }
        register_flock_waiter(inode, &task);
        drop(table);

        crate::task::block_current_and_run_next();
        let interrupted = {
            let mut task_inner = task.inner_exclusive_access();
            let interrupted = task_inner.interrupted_by_signal;
            if interrupted {
                task_inner.interrupted_by_signal = false;
            }
            interrupted
        };
        if interrupted {
            remove_flock_waiter(key, &task);
            return Err(SysError::EINTR);
        }
        if task
            .process
            .upgrade()
            .is_none_or(|process| process.inner_exclusive_access().is_zombie)
        {
            remove_flock_waiter(key, &task);
            return Err(SysError::EINTR);
        }
    }
}

fn conflict_to_user(lock: PosixLock) -> UserFlock {
    let len = if lock.range.end == OFFSET_MAX {
        0
    } else {
        i64::try_from(lock.range.end - lock.range.start + 1).unwrap_or(0)
    };
    UserFlock {
        lock_type: lock.lock_type,
        whence: SEEK_SET,
        start: lock.range.start as i64,
        len,
        pid: lock.owner_pid as i32,
    }
}

pub(super) fn fcntl_record_lock(
    file: Arc<dyn File + Send + Sync>,
    cmd: usize,
    arg: usize,
) -> SyscallResult {
    let requested = read_flock(arg)?;
    if !matches!(requested.lock_type, F_RDLCK | F_WRLCK | F_UNLCK) {
        return Err(SysError::EINVAL);
    }
    if cmd == F_GETLK && requested.lock_type == F_UNLCK {
        return Err(SysError::EINVAL);
    }
    if cmd != F_GETLK {
        if requested.lock_type == F_RDLCK && !file.readable() {
            return Err(SysError::EBADF);
        }
        if requested.lock_type == F_WRLCK && !file.writable() {
            return Err(SysError::EBADF);
        }
    }

    let key = inode_key(&file)?;
    let range = normalize_range(&file, requested)?;
    let owner_pid = current_process().getpid();

    if cmd == F_GETLK {
        let conflict =
            RECORD_LOCKS.lock().inodes.get(&key).and_then(|inode| {
                first_conflict(&inode.locks, owner_pid, requested.lock_type, range)
            });
        let result = conflict.map_or(
            UserFlock {
                lock_type: F_UNLCK,
                ..requested
            },
            conflict_to_user,
        );
        return write_flock(arg, result);
    }

    let blocking = cmd == F_SETLKW;
    let task = current_task().ok_or(SysError::ESRCH)?;
    loop {
        let mut table = RECORD_LOCKS.lock();
        let inode = table.inodes.entry(key).or_default();
        if requested.lock_type == F_UNLCK
            || first_conflict(&inode.locks, owner_pid, requested.lock_type, range).is_none()
        {
            replace_owner_range(&mut inode.locks, owner_pid, requested.lock_type, range);
            let waiters = take_waiters(inode);
            if inode_locks_empty(inode) {
                table.inodes.remove(&key);
            }
            drop(table);
            wake_waiters(waiters);
            return Ok(0);
        }
        if !blocking {
            return Err(SysError::EAGAIN);
        }
        register_waiter(inode, &task);
        drop(table);

        crate::task::block_current_and_run_next();
        let interrupted = {
            let mut task_inner = task.inner_exclusive_access();
            let interrupted = task_inner.interrupted_by_signal;
            if interrupted {
                task_inner.interrupted_by_signal = false;
            }
            interrupted
        };
        if interrupted {
            remove_waiter(key, &task);
            return Err(SysError::EINTR);
        }
        if task
            .process
            .upgrade()
            .is_none_or(|process| process.inner_exclusive_access().is_zombie)
        {
            remove_waiter(key, &task);
            return Err(SysError::EINTR);
        }
    }
}

pub(crate) fn release_process_file_locks(owner_pid: usize, file: &Arc<dyn File + Send + Sync>) {
    let Some(key) = file.cache_inode_id() else {
        return;
    };
    let waiters = {
        let mut table = RECORD_LOCKS.lock();
        let mut waiters = Vec::new();
        if let Some(inode) = table.inodes.get_mut(&key) {
            let old_len = inode.locks.len();
            inode.locks.retain(|lock| lock.owner_pid != owner_pid);
            if inode.locks.len() != old_len {
                waiters = take_waiters(inode);
            }
            if inode_locks_empty(inode) {
                table.inodes.remove(&key);
            }
        }
        waiters
    };
    wake_waiters(waiters);
    release_file_description_flock_if_unreferenced(file);
}

/// Release a BSD flock once no process has a descriptor referring to its open
/// file description. This deliberately scans descriptor tables rather than
/// Arc strong counts, because VM mappings and deferred writeback may retain a
/// file object without retaining an open descriptor.
pub(crate) fn release_file_description_flock_if_unreferenced(file: &Arc<dyn File + Send + Sync>) {
    let owner_file = open_file_description_key(file);
    let has_lock = {
        let table = RECORD_LOCKS.lock();
        table.inodes.values().any(|inode| {
            inode
                .flock_locks
                .iter()
                .any(|lock| lock.owner_file == owner_file)
        })
    };
    if !has_lock {
        return;
    }

    let still_referenced = crate::task::manager::all_processes()
        .into_iter()
        .any(|process| {
            process
                .inner_exclusive_access()
                .fd_table
                .iter()
                .flatten()
                .any(|open_file| Arc::ptr_eq(open_file, file))
        });
    if still_referenced {
        return;
    }

    let waiters = {
        let mut table = RECORD_LOCKS.lock();
        let mut waiters = Vec::new();
        let keys: Vec<usize> = table.inodes.keys().copied().collect();
        for key in keys {
            if let Some(inode) = table.inodes.get_mut(&key) {
                let old_len = inode.flock_locks.len();
                inode
                    .flock_locks
                    .retain(|lock| lock.owner_file != owner_file);
                if inode.flock_locks.len() != old_len {
                    waiters.extend(take_flock_waiters(inode));
                }
                if inode_locks_empty(inode) {
                    table.inodes.remove(&key);
                }
            }
        }
        waiters
    };
    wake_waiters(waiters);
}

pub(crate) fn release_process_record_locks(owner_pid: usize) {
    let waiters = {
        let mut table = RECORD_LOCKS.lock();
        let mut waiters = Vec::new();
        let keys: Vec<usize> = table.inodes.keys().copied().collect();
        for key in keys {
            if let Some(inode) = table.inodes.get_mut(&key) {
                let old_len = inode.locks.len();
                inode.locks.retain(|lock| lock.owner_pid != owner_pid);
                if inode.locks.len() != old_len {
                    waiters.extend(take_waiters(inode));
                }
                if inode_locks_empty(inode) {
                    table.inodes.remove(&key);
                }
            }
        }
        waiters
    };
    wake_waiters(waiters);
}
