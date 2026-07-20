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

#[derive(Default)]
struct InodeLocks {
    locks: Vec<PosixLock>,
    waiters: Vec<Weak<TaskControlBlock>>,
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
        if inode.locks.is_empty() && inode.waiters.is_empty() {
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
            if inode.locks.is_empty() && inode.waiters.is_empty() {
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
            if inode.locks.is_empty() && inode.waiters.is_empty() {
                table.inodes.remove(&key);
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
                if inode.locks.is_empty() && inode.waiters.is_empty() {
                    table.inodes.remove(&key);
                }
            }
        }
        waiters
    };
    wake_waiters(waiters);
}
