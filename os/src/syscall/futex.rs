//! futex(2) 最小实现
//!
//! 目前支持：
//! - `FUTEX_WAIT` / `FUTEX_WAIT_PRIVATE`
//! - `FUTEX_WAKE` / `FUTEX_WAKE_PRIVATE`
//! - `FUTEX_REQUEUE` / `FUTEX_REQUEUE_PRIVATE`
//! - `FUTEX_WAIT_BITSET` / `FUTEX_WAKE_BITSET`
//!
//! 超时：支持基于 timer 中断轮询的粗粒度超时唤醒。
//!
//! 注意：本实现使用 `(pid, uaddr)` 作为 futex key，适用于同一进程内线程同步
//! （musl pthread 默认带 `FUTEX_PRIVATE_FLAG`）。跨进程共享内存 futex 尚未支持。

use crate::error::{SysError, SyscallResult};
use crate::mm::{PageTable, VirtAddr};
use crate::mm::{translated_byte_buffer, translated_byte_buffer_no_fault, translated_ref};
use crate::sync::SpinNoIrqLock;
use crate::syscall::time::TimeSpec;
use crate::task::current_user_token;
use crate::task::{
    block_current_and_run_next, current_process, current_task, tid2task, wakeup_task,
};
use alloc::collections::{BTreeMap, VecDeque};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use lazy_static::lazy_static;
use log::warn;
use log::{error, info};
use polyhal::timer::current_time;

const FUTEX_OWNER_DIED: u32 = 0x40000000;
const FUTEX_WAITERS: u32 = 0x80000000;
const FUTEX_TID_MASK: u32 = 0x3fffffff;
const ROBUST_LIST_LIMIT: usize = 2048;

// Linux futex 操作码
const FUTEX_WAIT: i32 = 0;
const FUTEX_WAKE: i32 = 1;
const FUTEX_REQUEUE: i32 = 3;
const FUTEX_CMP_REQUEUE: i32 = 4;
const FUTEX_WAKE_OP: i32 = 5;
const FUTEX_LOCK_PI: i32 = 6;
const FUTEX_UNLOCK_PI: i32 = 7;
const FUTEX_TRYLOCK_PI: i32 = 8;
const FUTEX_WAIT_BITSET: i32 = 9;
const FUTEX_WAKE_BITSET: i32 = 10;
const FUTEX_WAIT_REQUEUE_PI: i32 = 11;
const FUTEX_CMP_REQUEUE_PI: i32 = 12;
const FUTEX_LOCK_PI2: i32 = 13;
const FUTEX_PRIVATE_FLAG: i32 = 128;
const FUTEX_CLOCK_REALTIME: i32 = 256;

const FUTEX_BITSET_MATCH_ANY: u32 = 0xffffffff;
const FUTEX_32: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Default)]
#[allow(missing_docs)]
pub struct FutexWaitv {
    val: u64,
    uaddr: u64,
    flags: u32,
    __reserved: u32,
}

/// futex 等待队列中的一个条目
pub struct FutexWaiter {
    task: Arc<crate::task::TaskControlBlock>,
    waiter_tid: usize,
    bitset: u32,
    /// Monotonic absolute timeout in nanoseconds, or `None` for no timeout.
    deadline_ns: Option<u64>,
    wake_index: usize,
    /// This waiter is queued on a PI mutex and therefore contributes to the
    /// current owner's inherited scheduling priority.
    pi_waiter: bool,
    /// Expected PI mutex for FUTEX_WAIT_REQUEUE_PI. Ordinary waiters carry
    /// `None` and must never be consumed by FUTEX_CMP_REQUEUE_PI.
    requeue_pi_target: Option<FutexKey>,
}

const NO_FUTEX_DEADLINE: u64 = u64::MAX;

/// Earliest monotonic futex deadline. The scheduler uses this as an
/// allocation-free fast path before inspecting the futex table while idle.
static NEXT_FUTEX_DEADLINE_NS: AtomicU64 = AtomicU64::new(NO_FUTEX_DEADLINE);

#[derive(Clone, Copy)]
enum FutexTimeoutMode {
    Relative,
    AbsoluteMonotonic,
    AbsoluteRealtime,
}

/// futex key：区分进程私有与跨进程共享
/// - Private: 同一进程内线程同步，使用 (pid, uaddr)
/// - Shared:  跨进程共享内存同步，使用物理地址
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum FutexKey {
    /// 进程私有 futex，使用 (pid, uaddr) 作为 key
    Private {
        /// 进程 ID
        pid: usize,
        /// 用户态虚拟地址
        uaddr: usize,
    },
    /// 跨进程共享 futex，使用物理地址作为 key
    Shared {
        /// 物理地址
        paddr: usize,
    },
}

lazy_static! {
    /// Global futex table
    pub static ref FUTEX_TABLE: SpinNoIrqLock<BTreeMap<FutexKey, VecDeque<FutexWaiter>>> =
        SpinNoIrqLock::new(BTreeMap::new());
    static ref PI_STATES: SpinNoIrqLock<BTreeMap<FutexKey, PiState>> =
        SpinNoIrqLock::new(BTreeMap::new());
}

#[derive(Clone, Copy)]
struct PiState {
    owner_tid: usize,
    max_waiter_priority: i32,
}

/// Once one member of a futex_waitv set is selected, all sibling queue entries
/// must disappear in the same FUTEX_TABLE critical section. Otherwise a wake
/// on one address and a timeout on another can both claim completion and make
/// the userspace return value depend on later task-lock timing.
fn remove_waitv_siblings_locked(
    table: &mut BTreeMap<FutexKey, VecDeque<FutexWaiter>>,
    task: &Arc<crate::task::TaskControlBlock>,
) {
    table.retain(|_, queue| {
        queue.retain(|waiter| !Arc::ptr_eq(&waiter.task, task));
        !queue.is_empty()
    });
}

fn commit_waitv_wake_selections_locked(
    table: &mut BTreeMap<FutexKey, VecDeque<FutexWaiter>>,
    selected: &mut Vec<FutexWaiter>,
) {
    let mut index = 0usize;
    while index < selected.len() {
        if selected[index].wake_index == usize::MAX {
            index += 1;
            continue;
        }
        let task = Arc::clone(&selected[index].task);
        let mut duplicate = index + 1;
        while duplicate < selected.len() {
            if Arc::ptr_eq(&selected[duplicate].task, &task) {
                selected.remove(duplicate);
            } else {
                duplicate += 1;
            }
        }
        remove_waitv_siblings_locked(table, &task);
        index += 1;
    }
}

#[allow(missing_docs)]
pub struct FutexStats {
    pub queues: usize,
    pub waiters: usize,
    pub lock_busy: bool,
}

#[allow(missing_docs)]
pub fn stats() -> FutexStats {
    let Some(table) = FUTEX_TABLE.try_lock() else {
        return FutexStats {
            queues: 0,
            waiters: 0,
            lock_busy: true,
        };
    };
    FutexStats {
        queues: table.len(),
        waiters: table.values().map(VecDeque::len).sum(),
        lock_busy: false,
    }
}

/// 从用户地址安全读取一个 u32（使用指定的页表 token，不依赖 current_task）。
fn read_user_u32_with_token(token: usize, uaddr: *const u32) -> Result<u32, SysError> {
    let buffers =
        translated_byte_buffer_no_fault(token, uaddr as *const u8, core::mem::size_of::<u32>())?;
    let mut bytes = [0u8; core::mem::size_of::<u32>()];
    let mut copied = 0usize;
    for buffer in buffers {
        let len = (bytes.len() - copied).min(buffer.len());
        bytes[copied..copied + len].copy_from_slice(&buffer[..len]);
        copied += len;
        if copied == bytes.len() {
            break;
        }
    }
    if copied != bytes.len() {
        return Err(SysError::EFAULT);
    }
    Ok(u32::from_ne_bytes(bytes))
}

fn validate_futex_addr(uaddr: *const u32) -> Result<usize, SysError> {
    let addr = uaddr as usize;
    if addr & (core::mem::align_of::<u32>() - 1) != 0 {
        return Err(SysError::EINVAL);
    }
    Ok(addr)
}

/// Read a futex word without allocating or faulting.
///
/// Callers use this while holding `FUTEX_TABLE`, so it must stay short and
/// must not invoke lazy page-fault handling.
fn read_user_u32_mapped(token: usize, uaddr: usize) -> Result<u32, SysError> {
    let page_table = PageTable::from_token(token);
    let va = VirtAddr::from(uaddr);
    let Some(pte) = page_table.translate(va.floor()) else {
        return Err(SysError::EFAULT);
    };
    if !pte.readable() {
        return Err(SysError::EFAULT);
    }
    let Some(pa) = page_table.translate_va(va) else {
        return Err(SysError::EFAULT);
    };
    Ok(unsafe { (&*pa.get_mut_ptr::<AtomicU32>()).load(Ordering::Acquire) })
}

fn user_atomic_u32(token: usize, uaddr: usize, write: bool) -> Result<&'static AtomicU32, SysError> {
    validate_futex_addr(uaddr as *const u32)?;
    let page_table = PageTable::from_token(token);
    let va = VirtAddr::from(uaddr);
    let Some(pte) = page_table.translate(va.floor()) else {
        return Err(SysError::EFAULT);
    };
    if !pte.readable() || write && !pte.writable() {
        return Err(SysError::EFAULT);
    }
    let pa = page_table.translate_va(va).ok_or(SysError::EFAULT)?;
    Ok(unsafe { &*pa.get_mut_ptr::<AtomicU32>() })
}

fn read_user_usize_with_token(token: usize, uaddr: usize) -> Result<usize, SysError> {
    let buffers = translated_byte_buffer_no_fault(token, uaddr as *const u8, size_of::<usize>())?;
    let mut bytes = [0u8; size_of::<usize>()];
    let mut copied = 0usize;
    for buffer in buffers {
        let len = (bytes.len() - copied).min(buffer.len());
        bytes[copied..copied + len].copy_from_slice(&buffer[..len]);
        copied += len;
        if copied == bytes.len() {
            break;
        }
    }
    if copied != bytes.len() {
        return Err(SysError::EFAULT);
    }
    Ok(usize::from_ne_bytes(bytes))
}

/// 构造 futex key
/// is_private 为 true 时使用 (pid, uaddr)；否则使用物理地址
fn make_key(uaddr: usize, is_private: bool) -> Result<FutexKey, SysError> {
    if is_private {
        let pid = current_process().getpid();
        Ok(FutexKey::Private { pid, uaddr })
    } else {
        let token = current_user_token();
        let page_table = PageTable::from_token(token);
        let va = VirtAddr::from(uaddr);
        match page_table.translate_va(va) {
            Some(pa) => Ok(FutexKey::Shared { paddr: pa.0 }),
            None => {
                error!(
                    "futex: shared futex addr {:p} not mapped",
                    uaddr as *const u8
                );
                Err(SysError::EFAULT)
            }
        }
    }
}

/// 系统调用入口
pub fn sys_futex(
    uaddr: *mut u32,
    futex_op: i32,
    val: u32,
    timeout: *const TimeSpec,
    uaddr2: *mut u32,
    val3: u32,
) -> SyscallResult {
    let op = futex_op & !(FUTEX_PRIVATE_FLAG | FUTEX_CLOCK_REALTIME);

    let is_private = (futex_op & FUTEX_PRIVATE_FLAG) != 0;
    let clock_realtime = (futex_op & FUTEX_CLOCK_REALTIME) != 0;
    if clock_realtime
        && !matches!(
            op,
            FUTEX_WAIT | FUTEX_WAIT_BITSET | FUTEX_LOCK_PI | FUTEX_LOCK_PI2 | FUTEX_WAIT_REQUEUE_PI
        )
    {
        return Err(SysError::EINVAL);
    }

    match op {
        FUTEX_WAIT => futex_wait(
            uaddr,
            val,
            timeout,
            FUTEX_BITSET_MATCH_ANY,
            is_private,
            FutexTimeoutMode::Relative,
            None,
        ),
        FUTEX_WAIT_BITSET => futex_wait(
            uaddr,
            val,
            timeout,
            val3,
            is_private,
            if clock_realtime {
                FutexTimeoutMode::AbsoluteRealtime
            } else {
                FutexTimeoutMode::AbsoluteMonotonic
            },
            None,
        ),
        FUTEX_WAKE => futex_wake(uaddr, val as usize, FUTEX_BITSET_MATCH_ANY, is_private),
        FUTEX_WAKE_BITSET => futex_wake(uaddr, val as usize, val3, is_private),
        FUTEX_REQUEUE => futex_requeue(uaddr, val as usize, timeout as usize, uaddr2, is_private),
        FUTEX_CMP_REQUEUE => futex_cmp_requeue(
            uaddr,
            val as usize,
            timeout as usize,
            uaddr2,
            val3,
            is_private,
        ),
        FUTEX_WAKE_OP => futex_wake_op(
            uaddr,
            val as usize,
            timeout as usize,
            uaddr2,
            val3,
            is_private,
        ),
        FUTEX_LOCK_PI | FUTEX_LOCK_PI2 => {
            futex_lock_pi(uaddr, timeout, is_private, clock_realtime, false)
        }
        FUTEX_TRYLOCK_PI => futex_lock_pi(uaddr, core::ptr::null(), is_private, false, true),
        FUTEX_UNLOCK_PI => futex_unlock_pi(uaddr, is_private),
        FUTEX_WAIT_REQUEUE_PI => futex_wait_requeue_pi(
            uaddr,
            val,
            timeout,
            uaddr2,
            is_private,
            clock_realtime,
        ),
        FUTEX_CMP_REQUEUE_PI => futex_cmp_requeue_pi(
            uaddr,
            val as usize,
            timeout as usize,
            uaddr2,
            val3,
            is_private,
        ),
        _ => {
            error!("Unsupported futex op: {}", op);
            Err(SysError::ENOSYS)
        }
    }
}

fn sign_extend_12(value: u32) -> i32 {
    ((value << 20) as i32) >> 20
}

fn futex_wake_op(
    uaddr: *mut u32,
    nr_wake: usize,
    nr_wake2: usize,
    uaddr2: *mut u32,
    encoded_op: u32,
    is_private: bool,
) -> SyscallResult {
    let uaddr1 = validate_futex_addr(uaddr)?;
    let uaddr2 = validate_futex_addr(uaddr2)?;
    let mut op = (encoded_op >> 28) & 0xf;
    let cmp = (encoded_op >> 24) & 0xf;
    let mut op_arg = sign_extend_12((encoded_op >> 12) & 0xfff);
    let cmp_arg = sign_extend_12(encoded_op & 0xfff);
    if op & 8 != 0 {
        op &= 7;
        if !(0..32).contains(&op_arg) {
            return Err(SysError::EINVAL);
        }
        op_arg = 1i32.wrapping_shl(op_arg as u32);
    }
    if op > 4 || cmp > 5 {
        return Err(SysError::ENOSYS);
    }

    let key1 = make_key(uaddr1, is_private)?;
    let key2 = make_key(uaddr2, is_private)?;
    let word = user_atomic_u32(current_user_token(), uaddr2, true)?;
    let mut to_wake = Vec::new();
    {
        // FUTEX_WAKE_OP's read-modify-write and both wake selections are one
        // operation with respect to futex queues. Holding FUTEX_TABLE also
        // prevents a PI lock from appearing after validation but before the
        // user word is modified.
        let mut table = FUTEX_TABLE.lock();
        let contains_pi = {
            let states = PI_STATES.lock();
            states.contains_key(&key1) || states.contains_key(&key2)
        };
        let special_waiter = [key1, key2].iter().any(|key| {
            table.get(key).is_some_and(|queue| {
                queue
                    .iter()
                    .any(|waiter| waiter.pi_waiter || waiter.requeue_pi_target.is_some())
            })
        });
        if contains_pi || special_waiter {
            return Err(SysError::EINVAL);
        }

        let mut old = word.load(Ordering::Acquire);
        loop {
            let new = match op {
                0 => op_arg as u32,
                1 => old.wrapping_add(op_arg as u32),
                2 => old | op_arg as u32,
                3 => old & !(op_arg as u32),
                4 => old ^ op_arg as u32,
                _ => unreachable!(),
            };
            match word.compare_exchange_weak(old, new, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => break,
                Err(observed) => old = observed,
            }
        }
        let old_signed = old as i32;
        let comparison = match cmp {
            0 => old_signed == cmp_arg,
            1 => old_signed != cmp_arg,
            2 => old_signed < cmp_arg,
            3 => old_signed <= cmp_arg,
            4 => old_signed > cmp_arg,
            5 => old_signed >= cmp_arg,
            _ => unreachable!(),
        };

        let mut collect = |key: FutexKey, limit: usize| {
            let mut empty = false;
            if let Some(queue) = table.get_mut(&key) {
                for _ in 0..limit {
                    let Some(waiter) = queue.pop_front() else {
                        break;
                    };
                    to_wake.push(waiter);
                }
                empty = queue.is_empty();
            }
            if empty {
                table.remove(&key);
            }
        };
        collect(key1, nr_wake);
        if comparison {
            collect(key2, nr_wake2);
        }
        commit_waitv_wake_selections_locked(&mut table, &mut to_wake);
    }
    let woken = to_wake.len();
    for waiter in to_wake {
        wake_futex_waiter(waiter);
    }
    Ok(woken)
}

fn current_global_tid() -> Result<usize, SysError> {
    current_task()
        .map(|task| task.inner_exclusive_access().global_tid)
        .ok_or(SysError::ESRCH)
}

fn apply_pi_boost(tid: usize) {
    // Priority inheritance is transitive: if T1 owns A while waiting for B,
    // boosting T1 must also update B's owner. Recompute each edge from the
    // actual queues instead of incrementally adding boosts, which also makes
    // timeout/signal/exit removal reliably lower inherited priority again.
    let mut pending = vec![tid];
    let mut visited = Vec::new();
    while let Some(tid) = pending.pop() {
        if tid == 0 || visited.contains(&tid) {
            continue;
        }
        visited.push(tid);

        let priority = {
            let states = PI_STATES.lock();
            states
                .values()
                .filter(|state| state.owner_tid == tid)
                .map(|state| state.max_waiter_priority)
                .max()
                .unwrap_or(0)
        };
        let Some(task) = tid2task(tid) else {
            continue;
        };
        let queued = task.is_ready_queued();
        if queued {
            crate::task::manager::remove_task(Arc::clone(&task));
        }
        task.set_pi_boost_priority(priority);
        if queued {
            crate::task::manager::add_task(Arc::clone(&task));
        } else if let Some(cpu) = task.on_cpu_index() {
            let _ = polyhal::multicore::send_reschedule_ipi(cpu);
        }

        // A task can block on at most one PI mutex at a time. Keep the scan
        // defensive and propagate every matching edge if corrupted userspace
        // or a concurrent cancellation temporarily leaves more than one.
        let upstream_owners = {
            let table = FUTEX_TABLE.lock();
            let waiting_keys: Vec<FutexKey> = table
                .iter()
                .filter_map(|(key, queue)| {
                    queue
                        .iter()
                        .any(|waiter| waiter.pi_waiter && waiter.waiter_tid == tid)
                        .then_some(*key)
                })
                .collect();
            let mut states = PI_STATES.lock();
            let mut owners = Vec::new();
            for key in waiting_keys {
                let max_waiter_priority = table
                    .get(&key)
                    .into_iter()
                    .flat_map(|queue| queue.iter())
                    .filter(|waiter| waiter.pi_waiter)
                    .map(|waiter| waiter.task.effective_sched_priority())
                    .max()
                    .unwrap_or(0);
                if let Some(state) = states.get_mut(&key) {
                    state.max_waiter_priority = max_waiter_priority;
                    if state.owner_tid != tid && !owners.contains(&state.owner_tid) {
                        owners.push(state.owner_tid);
                    }
                }
            }
            owners
        };
        pending.extend(upstream_owners);
    }
}

fn futex_lock_pi(
    uaddr: *mut u32,
    timeout: *const TimeSpec,
    is_private: bool,
    clock_realtime: bool,
    try_only: bool,
) -> SyscallResult {
    let uaddr = validate_futex_addr(uaddr)?;
    let token = current_user_token();
    let key = make_key(uaddr, is_private)?;
    let tid = current_global_tid()?;
    if tid > FUTEX_TID_MASK as usize {
        return Err(SysError::EOVERFLOW);
    }
    let word = user_atomic_u32(token, uaddr, true)?;
    let deadline = parse_futex_deadline(
        token,
        timeout,
        if clock_realtime {
            FutexTimeoutMode::AbsoluteRealtime
        } else {
            FutexTimeoutMode::AbsoluteMonotonic
        },
    )?;
    let task = current_task().ok_or(SysError::ESRCH)?;

    loop {
        let mut observed = word.load(Ordering::Acquire);
        if observed & FUTEX_TID_MASK == tid as u32 {
            return Err(SysError::EDEADLK);
        }
        if observed & FUTEX_TID_MASK == 0 {
            // Serialize acquisition and PI-state publication with ordinary
            // WAIT/WAKE queue operations. Without this table lock an ordinary
            // waiter can slip between the owner-word CAS and PI_STATES insert,
            // leaving FUTEX_UNLOCK_PI to transfer ownership to a non-PI waiter.
            let table = FUTEX_TABLE.lock();
            observed = word.load(Ordering::Acquire);
            if observed & FUTEX_TID_MASK != 0 {
                drop(table);
                continue;
            }
            if table
                .get(&key)
                .is_some_and(|queue| queue.iter().any(|waiter| !waiter.pi_waiter))
            {
                return Err(SysError::EINVAL);
            }
            let replacement = (observed & (FUTEX_OWNER_DIED | FUTEX_WAITERS)) | tid as u32;
            if word
                .compare_exchange(observed, replacement, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                drop(table);
                continue;
            }
            let max_waiter_priority = table
                .get(&key)
                .into_iter()
                .flat_map(|queue| queue.iter())
                .filter(|waiter| waiter.pi_waiter)
                .map(|waiter| waiter.task.effective_sched_priority())
                .max()
                .unwrap_or(0);
            let previous_owner = PI_STATES
                .lock()
                .insert(
                    key,
                    PiState {
                        owner_tid: tid,
                        max_waiter_priority,
                    },
                )
                .map(|state| state.owner_tid);
            drop(table);
            if let Some(previous_owner) = previous_owner.filter(|owner| *owner != tid) {
                apply_pi_boost(previous_owner);
            }
            apply_pi_boost(tid);
            return Ok(0);
        }
        if try_only {
            return Err(SysError::EAGAIN);
        }
        if deadline.is_some_and(|deadline| monotonic_now_ns() >= deadline) {
            return Err(SysError::ETIMEDOUT);
        }
        if observed & FUTEX_WAITERS == 0 {
            match word.compare_exchange_weak(
                observed,
                observed | FUTEX_WAITERS,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => observed |= FUTEX_WAITERS,
                Err(_) => continue,
            }
        }
        let owner_tid = (observed & FUTEX_TID_MASK) as usize;
        {
            let mut inner = task.inner_exclusive_access();
            inner.futex_woken = false;
            inner.futex_timed_out = false;
        }
        {
            let mut table = FUTEX_TABLE.lock();
            let current = word.load(Ordering::Acquire);
            if current & FUTEX_TID_MASK != owner_tid as u32 || current & FUTEX_TID_MASK == 0 {
                continue;
            }
            if table
                .get(&key)
                .is_some_and(|queue| queue.iter().any(|waiter| !waiter.pi_waiter))
            {
                return Err(SysError::EINVAL);
            }
            table.entry(key).or_insert_with(VecDeque::new).push_back(FutexWaiter {
                task: Arc::clone(&task),
                waiter_tid: tid,
                bitset: FUTEX_BITSET_MATCH_ANY,
                deadline_ns: deadline,
                wake_index: usize::MAX,
                pi_waiter: true,
                requeue_pi_target: None,
            });
            let waiter_priority = task.effective_sched_priority();
            let mut states = PI_STATES.lock();
            let state = states.entry(key).or_insert(PiState {
                owner_tid,
                max_waiter_priority: 0,
            });
            state.owner_tid = owner_tid;
            state.max_waiter_priority = state.max_waiter_priority.max(waiter_priority);
        }
        apply_pi_boost(owner_tid);
        if let Some(deadline) = deadline {
            publish_futex_deadline(deadline);
        }

        loop {
            {
                let mut inner = task.inner_exclusive_access();
                if inner.futex_woken {
                    inner.futex_woken = false;
                    drop(inner);
                    // Normal PI unlock performs an atomic owner transfer
                    // before waking us. Robust-owner death, however, wakes a
                    // waiter after publishing OWNER_DIED with owner 0. In that
                    // case retry acquisition instead of falsely returning as
                    // owner of an unlocked word.
                    if word.load(Ordering::Acquire) & FUTEX_TID_MASK == tid as u32 {
                        return Ok(0);
                    }
                    break;
                }
                if inner.futex_timed_out || inner.interrupted_by_signal {
                    let timed_out = inner.futex_timed_out;
                    inner.futex_timed_out = false;
                    inner.interrupted_by_signal = false;
                    drop(inner);
                    if let Some(owner_tid) = remove_task_from_futex_queue(&key, &task) {
                        apply_pi_boost(owner_tid);
                    }
                    return Err(if timed_out {
                        SysError::ETIMEDOUT
                    } else {
                        SysError::EINTR
                    });
                }
            }
            block_current_and_run_next();
        }
    }
}

fn futex_unlock_pi(uaddr: *mut u32, is_private: bool) -> SyscallResult {
    let uaddr = validate_futex_addr(uaddr)?;
    let token = current_user_token();
    let key = make_key(uaddr, is_private)?;
    let tid = current_global_tid()?;
    let word = user_atomic_u32(token, uaddr, true)?;
    let mut next_waiter = None;
    let mut next_owner = 0usize;
    let previous_owner;
    {
        let mut table = FUTEX_TABLE.lock();
        let observed = word.load(Ordering::Acquire);
        if observed & FUTEX_TID_MASK != tid as u32 {
            return Err(SysError::EPERM);
        }
        if let Some(queue) = table.get_mut(&key) {
            if !queue.is_empty() {
                let position = queue
                    .iter()
                    .enumerate()
                    .max_by_key(|(_, waiter)| waiter.task.effective_sched_priority())
                    .map(|(index, _)| index)
                    .unwrap();
                let waiter = queue.remove(position).unwrap();
                next_owner = waiter.waiter_tid;
                let replacement = next_owner as u32
                    | if queue.is_empty() { 0 } else { FUTEX_WAITERS };
                word.compare_exchange(observed, replacement, Ordering::AcqRel, Ordering::Acquire)
                    .map_err(|_| SysError::EAGAIN)?;
                next_waiter = Some(waiter);
            }
            if queue.is_empty() {
                table.remove(&key);
            }
        }
        if next_waiter.is_none() {
            word.compare_exchange(observed, 0, Ordering::AcqRel, Ordering::Acquire)
                .map_err(|_| SysError::EAGAIN)?;
        }
        let max_waiter_priority = table
            .get(&key)
            .into_iter()
            .flat_map(|queue| queue.iter())
            .filter(|waiter| waiter.pi_waiter)
            .map(|waiter| waiter.task.effective_sched_priority())
            .max()
            .unwrap_or(0);
        let mut states = PI_STATES.lock();
        previous_owner = states.get(&key).map(|state| state.owner_tid);
        if next_owner == 0 {
            states.remove(&key);
        } else {
            states.insert(
                key,
                PiState {
                    owner_tid: next_owner,
                    max_waiter_priority,
                },
            );
        }
    }
    apply_pi_boost(tid);
    if let Some(previous_owner) = previous_owner.filter(|owner| *owner != tid) {
        apply_pi_boost(previous_owner);
    }
    if next_owner != 0 {
        apply_pi_boost(next_owner);
    }
    if let Some(waiter) = next_waiter {
        wake_futex_waiter(waiter);
    }
    Ok(0)
}

fn futex_wait_requeue_pi(
    uaddr: *mut u32,
    val: u32,
    timeout: *const TimeSpec,
    uaddr2: *mut u32,
    is_private: bool,
    clock_realtime: bool,
) -> SyscallResult {
    if uaddr == uaddr2 || uaddr2.is_null() {
        return Err(SysError::EINVAL);
    }
    let target_key = make_key(validate_futex_addr(uaddr2)?, is_private)?;
    futex_wait(
        uaddr,
        val,
        timeout,
        FUTEX_BITSET_MATCH_ANY,
        is_private,
        if clock_realtime {
            FutexTimeoutMode::AbsoluteRealtime
        } else {
            FutexTimeoutMode::AbsoluteMonotonic
        },
        Some(target_key),
    )?;
    let tid = current_global_tid()?;
    let word = user_atomic_u32(current_user_token(), uaddr2 as usize, true)?;
    if word.load(Ordering::Acquire) & FUTEX_TID_MASK == tid as u32 {
        return Ok(0);
    }
    futex_lock_pi(uaddr2, core::ptr::null(), is_private, false, false)
}

fn futex_cmp_requeue_pi(
    uaddr: *mut u32,
    nr_wake: usize,
    nr_requeue: usize,
    uaddr2: *mut u32,
    cmpval: u32,
    is_private: bool,
) -> SyscallResult {
    if nr_wake != 1 || uaddr == uaddr2 || uaddr2.is_null() {
        return Err(SysError::EINVAL);
    }
    let token = current_user_token();
    let key1 = make_key(validate_futex_addr(uaddr)?, is_private)?;
    let key2 = make_key(validate_futex_addr(uaddr2)?, is_private)?;
    let word2 = user_atomic_u32(token, uaddr2 as usize, true)?;
    let mut acquired = None;
    let mut owner_tid: usize;
    let previous_owner;
    let moved_count;
    {
        let mut table = FUTEX_TABLE.lock();
        if read_user_u32_mapped(token, uaddr as usize)? != cmpval {
            return Err(SysError::EAGAIN);
        }
        let mut moved = Vec::new();
        if let Some(queue) = table.get_mut(&key1) {
            let limit = nr_wake.saturating_add(nr_requeue);
            if queue
                .iter()
                .take(limit)
                .any(|waiter| waiter.requeue_pi_target != Some(key2))
            {
                return Err(SysError::EINVAL);
            }
            while moved.len() < limit {
                let Some(waiter) = queue.pop_front() else {
                    break;
                };
                let mut waiter = waiter;
                waiter.pi_waiter = true;
                moved.push(waiter);
            }
            if queue.is_empty() {
                table.remove(&key1);
            }
        }
        moved_count = moved.len();
        if moved_count == 0 {
            return Ok(0);
        }
        let queue2 = table.entry(key2).or_insert_with(VecDeque::new);
        queue2.extend(moved);

        let mut observed = word2.load(Ordering::Acquire);
        if observed & FUTEX_TID_MASK == 0 {
            let position = queue2
                .iter()
                .enumerate()
                .max_by_key(|(_, waiter)| waiter.task.effective_sched_priority())
                .map(|(index, _)| index)
                .unwrap();
            let waiter = queue2.remove(position).unwrap();
            owner_tid = waiter.waiter_tid;
            let replacement = owner_tid as u32
                | if queue2.is_empty() { 0 } else { FUTEX_WAITERS };
            loop {
                match word2.compare_exchange_weak(
                    observed,
                    replacement,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        acquired = Some(waiter);
                        break;
                    }
                    Err(current) if current & FUTEX_TID_MASK == 0 => observed = current,
                    Err(current) => {
                        owner_tid = (current & FUTEX_TID_MASK) as usize;
                        queue2.push_front(waiter);
                        break;
                    }
                }
            }
        } else {
            owner_tid = (observed & FUTEX_TID_MASK) as usize;
        }
        if !queue2.is_empty() {
            word2.fetch_or(FUTEX_WAITERS, Ordering::AcqRel);
        }
        let max_waiter_priority = table
            .get(&key2)
            .into_iter()
            .flat_map(|queue| queue.iter())
            .filter(|waiter| waiter.pi_waiter)
            .map(|waiter| waiter.task.effective_sched_priority())
            .max()
            .unwrap_or(0);
        previous_owner = PI_STATES
            .lock()
            .insert(
                key2,
                PiState {
                    owner_tid,
                    max_waiter_priority,
                },
            )
            .map(|state| state.owner_tid);
    }
    if let Some(previous_owner) = previous_owner.filter(|previous| *previous != owner_tid) {
        apply_pi_boost(previous_owner);
    }
    apply_pi_boost(owner_tid);
    if let Some(waiter) = acquired {
        wake_futex_waiter(waiter);
    }
    Ok(moved_count)
}

fn monotonic_now_ns() -> u64 {
    current_time()
        .as_nanos()
        .min((NO_FUTEX_DEADLINE - 1) as u128) as u64
}

fn publish_futex_deadline(deadline_ns: u64) {
    let mut current = NEXT_FUTEX_DEADLINE_NS.load(Ordering::Acquire);
    while deadline_ns < current {
        match NEXT_FUTEX_DEADLINE_NS.compare_exchange_weak(
            current,
            deadline_ns,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => break,
            Err(observed) => current = observed,
        }
    }
}

fn parse_futex_deadline(
    token: usize,
    timeout: *const TimeSpec,
    mode: FutexTimeoutMode,
) -> Result<Option<u64>, SysError> {
    if timeout.is_null() {
        return Ok(None);
    }
    let ts = *translated_ref(token, timeout)?;
    if ts.tv_sec < 0 || ts.tv_nsec < 0 || ts.tv_nsec >= 1_000_000_000 {
        return Err(SysError::EINVAL);
    }

    let requested_ns = (ts.tv_sec as u128)
        .saturating_mul(1_000_000_000)
        .saturating_add(ts.tv_nsec as u128);
    let monotonic_now = current_time().as_nanos();
    let deadline = match mode {
        FutexTimeoutMode::Relative => monotonic_now.saturating_add(requested_ns),
        FutexTimeoutMode::AbsoluteMonotonic => requested_ns,
        FutexTimeoutMode::AbsoluteRealtime => {
            monotonic_now.saturating_add(requested_ns.saturating_sub(crate::timer::realtime_ns()))
        }
    };
    Ok(Some(deadline.min((NO_FUTEX_DEADLINE - 1) as u128) as u64))
}

#[allow(missing_docs)]
pub fn sys_futex_waitv(
    waiters: *const FutexWaitv,
    nr_futexes: usize,
    flags: u32,
    timeout: *const TimeSpec,
    clockid: i32,
) -> SyscallResult {
    const FUTEX_WAITV_MAX: usize = 128;
    if waiters.is_null() {
        return Err(SysError::EFAULT);
    }
    if nr_futexes == 0 || nr_futexes > FUTEX_WAITV_MAX || flags != 0 {
        return Err(SysError::EINVAL);
    }
    let timeout_mode = match clockid {
        0 => FutexTimeoutMode::AbsoluteRealtime,
        1 => FutexTimeoutMode::AbsoluteMonotonic,
        _ => return Err(SysError::EINVAL),
    };
    let token = current_user_token();
    let total = nr_futexes
        .checked_mul(size_of::<FutexWaitv>())
        .ok_or(SysError::EINVAL)?;
    let buffers = translated_byte_buffer(token, waiters as *const u8, total)?;
    let mut raw = Vec::with_capacity(total);
    for buffer in buffers {
        raw.extend_from_slice(&buffer);
    }
    if raw.len() != total {
        return Err(SysError::EFAULT);
    }
    let mut entries = Vec::with_capacity(nr_futexes);
    for index in 0..nr_futexes {
        let offset = index * size_of::<FutexWaitv>();
        let mut entry = FutexWaitv::default();
        unsafe {
            core::ptr::copy_nonoverlapping(
                raw.as_ptr().add(offset),
                &mut entry as *mut FutexWaitv as *mut u8,
                size_of::<FutexWaitv>(),
            );
        }
        if entry.__reserved != 0
            || entry.val > u32::MAX as u64
            || entry.uaddr > usize::MAX as u64
            || entry.flags & !(FUTEX_32 | FUTEX_PRIVATE_FLAG as u32) != 0
            || entry.flags & FUTEX_32 == 0
        {
            return Err(SysError::EINVAL);
        }
        let uaddr = validate_futex_addr(entry.uaddr as usize as *const u32)?;
        let is_private = entry.flags & FUTEX_PRIVATE_FLAG as u32 != 0;
        let key = make_key(uaddr, is_private)?;
        if read_user_u32_with_token(token, uaddr as *const u32)? != entry.val as u32 {
            return Err(SysError::EAGAIN);
        }
        entries.push((uaddr, key, entry.val as u32));
    }
    let deadline_ns = parse_futex_deadline(token, timeout, timeout_mode)?;
    let task = current_task().ok_or(SysError::ESRCH)?;
    let waiter_tid = task.inner_exclusive_access().global_tid;
    {
        let mut inner = task.inner_exclusive_access();
        inner.futex_woken = false;
        inner.futex_timed_out = false;
        inner.futex_waitv_index = usize::MAX;
    }
    {
        let mut table = FUTEX_TABLE.lock();
        let contains_pi = {
            let states = PI_STATES.lock();
            entries.iter().any(|(_, key, _)| states.contains_key(key))
        };
        if contains_pi {
            return Err(SysError::EINVAL);
        }
        for (uaddr, _, expected) in entries.iter() {
            if read_user_u32_mapped(token, *uaddr)? != *expected {
                return Err(SysError::EAGAIN);
            }
        }
        if deadline_ns.is_some_and(|deadline| monotonic_now_ns() >= deadline) {
            return Err(SysError::ETIMEDOUT);
        }
        for (index, (_, key, _)) in entries.iter().enumerate() {
            table.entry(*key).or_insert_with(VecDeque::new).push_back(FutexWaiter {
                task: Arc::clone(&task),
                waiter_tid,
                bitset: FUTEX_BITSET_MATCH_ANY,
                deadline_ns,
                wake_index: index,
                pi_waiter: false,
                requeue_pi_target: None,
            });
        }
    }
    if let Some(deadline) = deadline_ns {
        publish_futex_deadline(deadline);
    }

    loop {
        {
            let mut inner = task.inner_exclusive_access();
            if inner.futex_timed_out {
                inner.futex_timed_out = false;
                drop(inner);
                remove_task_from_futex_table(&task);
                return Err(SysError::ETIMEDOUT);
            }
            if inner.futex_woken {
                inner.futex_woken = false;
                let index = inner.futex_waitv_index;
                inner.futex_waitv_index = usize::MAX;
                drop(inner);
                remove_task_from_futex_table(&task);
                return (index < nr_futexes).then_some(index).ok_or(SysError::EINTR);
            }
            if inner.interrupted_by_signal
                || inner.zombie_flag.load(core::sync::atomic::Ordering::Acquire)
            {
                inner.interrupted_by_signal = false;
                drop(inner);
                remove_task_from_futex_table(&task);
                return Err(SysError::EINTR);
            }
        }
        block_current_and_run_next();
    }
}

/// 从 futex 等待队列中移除指定任务
fn remove_task_from_futex_queue(
    key: &FutexKey,
    task: &Arc<crate::task::TaskControlBlock>,
) -> Option<usize> {
    let mut table = FUTEX_TABLE.lock();
    let mut removed_pi = false;
    if let Some(queue) = table.get_mut(key) {
        let mut remaining = VecDeque::new();
        while let Some(waiter) = queue.pop_front() {
            if Arc::ptr_eq(&waiter.task, task) {
                removed_pi |= waiter.pi_waiter;
                continue;
            }
            remaining.push_back(waiter);
        }
        if remaining.is_empty() {
            table.remove(key);
        } else {
            *queue = remaining;
        }
    }
    if !removed_pi {
        return None;
    }
    let max_waiter_priority = table
        .get(key)
        .into_iter()
        .flat_map(|queue| queue.iter())
        .filter(|waiter| waiter.pi_waiter)
        .map(|waiter| waiter.task.effective_sched_priority())
        .max()
        .unwrap_or(0);
    PI_STATES.lock().get_mut(key).map(|state| {
        state.max_waiter_priority = max_waiter_priority;
        state.owner_tid
    })
}

/// Remove every futex waiter owned by `task`.
///
/// This is used by task/process exit cleanup. Normal futex wake paths remove
/// waiters from their exact key, but a task killed while blocked may otherwise
/// leave a strong TCB reference in the global futex table.
pub fn remove_task_from_futex_table(task: &Arc<crate::task::TaskControlBlock>) {
    let mut table = FUTEX_TABLE.lock();
    let task_ptr = Arc::as_ptr(task);
    let keys: Vec<FutexKey> = table.keys().cloned().collect();
    let mut affected_pi_keys = Vec::new();
    for key in keys {
        let should_remove = if let Some(queue) = table.get_mut(&key) {
            let mut removed_pi = false;
            queue.retain(|waiter| {
                let remove = Arc::as_ptr(&waiter.task) == task_ptr;
                removed_pi |= remove && waiter.pi_waiter;
                !remove
            });
            if removed_pi {
                affected_pi_keys.push(key);
            }
            queue.is_empty()
        } else {
            false
        };
        if should_remove {
            table.remove(&key);
        }
    }
    let mut owners = Vec::new();
    {
        let mut states = PI_STATES.lock();
        for key in affected_pi_keys {
            let max_waiter_priority = table
                .get(&key)
                .into_iter()
                .flat_map(|queue| queue.iter())
                .filter(|waiter| waiter.pi_waiter)
                .map(|waiter| waiter.task.effective_sched_priority())
                .max()
                .unwrap_or(0);
            if let Some(state) = states.get_mut(&key) {
                state.max_waiter_priority = max_waiter_priority;
                if !owners.contains(&state.owner_tid) {
                    owners.push(state.owner_tid);
                }
            }
        }
    }
    drop(table);
    for owner in owners {
        apply_pi_boost(owner);
    }
}

/// FUTEX_WAIT / FUTEX_WAIT_BITSET
fn futex_wait(
    uaddr: *mut u32,
    val: u32,
    timeout: *const TimeSpec,
    bitset: u32,
    is_private: bool,
    timeout_mode: FutexTimeoutMode,
    requeue_pi_target: Option<FutexKey>,
) -> SyscallResult {
    let _perf_timer =
        crate::task::perf_stats::scope_timer(crate::task::perf_stats::PerfTimerKind::FutexWait);
    if bitset == 0 {
        return Err(SysError::EINVAL);
    }
    let uaddr_usize = validate_futex_addr(uaddr)?;
    let token = current_user_token();

    let current_val = read_user_u32_with_token(token, uaddr)?;
    if current_val != val {
        info!(
            "futex_wait: val mismatch, expected {}, got {}, returning EAGAIN",
            val, current_val
        );
        return Err(SysError::EAGAIN);
    }
    error!(
        "futex_wait: addr={:p}, val={}, task={:?}",
        uaddr,
        val,
        current_task().map(|t| t
            .inner_exclusive_access()
            .res
            .as_ref()
            .map(|r| r.tid)
            .unwrap_or(999))
    );

    let key = make_key(uaddr_usize, is_private)?;
    let task = current_task().unwrap();
    let waiter_tid = task.inner_exclusive_access().global_tid;

    // 1. Resolve the Linux timeout ABI outside the futex-table lock. WAIT uses
    // a relative duration, while WAIT_BITSET uses an absolute clock deadline.
    let deadline_ns = parse_futex_deadline(token, timeout, timeout_mode)?;

    // 2. Linux futex WAIT 的核心语义是“原子比较并阻塞”：
    //    wake/requeue 也持有 FUTEX_TABLE，因此 signal 不能滑过比较和入队之间。
    // Reset the per-task result before taking FUTEX_TABLE.  No path may wait
    // for a TCB spinlock while holding the global futex-table spinlock: timer
    // timeout processing and task exit can otherwise form an IRQ-off lock
    // chain that prevents both timer and recovery IPI delivery on the waiter.
    {
        let mut t_inner = task.inner_exclusive_access();
        t_inner.futex_woken = false;
        t_inner.futex_timed_out = false;
    }
    {
        let mut table = FUTEX_TABLE.lock();
        if PI_STATES.lock().contains_key(&key)
            || table.get(&key).is_some_and(|queue| {
                queue
                    .iter()
                    .any(|waiter| waiter.pi_waiter || waiter.requeue_pi_target.is_some())
            })
        {
            return Err(SysError::EINVAL);
        }
        let current_val = read_user_u32_mapped(token, uaddr_usize)?;
        if current_val != val {
            info!(
                "futex_wait: val mismatch, expected {}, got {}, returning EAGAIN",
                val, current_val
            );
            return Err(SysError::EAGAIN);
        }
        if deadline_ns.is_some_and(|deadline| monotonic_now_ns() >= deadline) {
            return Err(SysError::ETIMEDOUT);
        }
        let queue = table.entry(key).or_insert_with(VecDeque::new);
        queue.push_back(FutexWaiter {
            task: task.clone(),
            waiter_tid,
            bitset,
            deadline_ns,
            wake_index: usize::MAX,
            pi_waiter: false,
            requeue_pi_target,
        });
    }
    if let Some(deadline) = deadline_ns {
        publish_futex_deadline(deadline);
    }

    // 3. 循环检查：处理 wake 已到达但还没真正切走、信号和超时。
    loop {
        {
            info!("loop");
            let mut t_inner = task.inner_exclusive_access();
            if t_inner.futex_timed_out {
                t_inner.futex_timed_out = false;
                drop(t_inner);
                return Err(SysError::ETIMEDOUT);
            }
            // 如果已经被 futex_wake 唤醒，直接返回成功
            if t_inner.futex_woken {
                t_inner.futex_woken = false;
                drop(t_inner);
                return Ok(0);
            }
            // 如果被信号中断，返回 EINTR
            if t_inner.interrupted_by_signal {
                t_inner.interrupted_by_signal = false;
                drop(t_inner);
                remove_task_from_futex_table(&task);
                return Err(SysError::EINTR);
            }

            // 如果进程已被 exit_group 等标记为 zombie，不再阻塞
            if t_inner
                .zombie_flag
                .load(core::sync::atomic::Ordering::SeqCst)
            {
                drop(t_inner);
                remove_task_from_futex_table(&task);
                return Err(SysError::EINTR);
            }
        }

        // 检查超时
        if deadline_ns.is_some() {
            // Timed waits block as well. The timer interrupt or idle scheduler
            // removes an expired waiter and wakes it with futex_timed_out set.
            error!("suspend");
            crate::task::block_current_and_run_next();
        } else {
            // 无超时：完全阻塞等待唤醒
            error!("block");
            crate::task::block_current_and_run_next();
        }
        // 被唤醒后回到循环开头重新检查条件
    }
}

/// FUTEX_WAKE / FUTEX_WAKE_BITSET
fn futex_wake(uaddr: *mut u32, nr_wake: usize, bitset: u32, is_private: bool) -> SyscallResult {
    let _perf_timer =
        crate::task::perf_stats::scope_timer(crate::task::perf_stats::PerfTimerKind::FutexWake);
    if bitset == 0 {
        return Err(SysError::EINVAL);
    }

    let key = make_key(validate_futex_addr(uaddr)?, is_private)?;
    info!(
        "futex_wake: addr={:p}, nr_wake={}, key={:?}",
        uaddr, nr_wake, key
    );
    let mut to_wake: Vec<FutexWaiter> = Vec::new();

    {
        let mut table = FUTEX_TABLE.lock();
        if PI_STATES.lock().contains_key(&key) {
            return Err(SysError::EINVAL);
        }
        if let Some(queue) = table.get_mut(&key) {
            let mut remaining = VecDeque::new();
            while let Some(waiter) = queue.pop_front() {
                if to_wake.len() < nr_wake && (waiter.bitset & bitset) != 0 {
                    to_wake.push(waiter);
                } else {
                    remaining.push_back(waiter);
                }
            }
            if remaining.is_empty() {
                table.remove(&key);
            } else {
                *queue = remaining;
            }
        }
        commit_waitv_wake_selections_locked(&mut table, &mut to_wake);
    }

    let woken = to_wake.len();
    crate::task::perf_stats::record_futex_wake_woken(woken);
    for waiter in to_wake {
        wake_futex_waiter(waiter);
    }

    Ok(woken)
}

fn wake_futex_waiter(waiter: FutexWaiter) {
    let task = waiter.task;
    {
        let mut inner = task.inner_exclusive_access();
        inner.futex_waitv_index = waiter.wake_index;
        inner.futex_woken = true;
    }
    wakeup_task(task);
}

/// FUTEX_REQUEUE
///
/// 先唤醒 `nr_wake` 个，然后把最多 `nr_requeue` 个从 `uaddr` 移到 `uaddr2`。
fn futex_requeue(
    uaddr: *mut u32,
    nr_wake: usize,
    nr_requeue: usize,
    uaddr2: *mut u32,
    is_private: bool,
) -> SyscallResult {
    let key1 = make_key(validate_futex_addr(uaddr)?, is_private)?;
    let key2 = make_key(validate_futex_addr(uaddr2)?, is_private)?;
    let mut to_wake: Vec<FutexWaiter> = Vec::new();
    let mut to_move: Vec<FutexWaiter> = Vec::new();

    {
        let mut table = FUTEX_TABLE.lock();
        let contains_pi = {
            let states = PI_STATES.lock();
            states.contains_key(&key1) || states.contains_key(&key2)
        };
        if contains_pi
            || table.get(&key1).is_some_and(|queue| {
                queue
                    .iter()
                    .any(|waiter| waiter.pi_waiter || waiter.requeue_pi_target.is_some())
            })
            || table.get(&key2).is_some_and(|queue| {
                queue
                    .iter()
                    .any(|waiter| waiter.pi_waiter || waiter.requeue_pi_target.is_some())
            })
        {
            return Err(SysError::EINVAL);
        }
        if let Some(queue) = table.get_mut(&key1) {
            // 先唤醒
            while !queue.is_empty() && to_wake.len() < nr_wake {
                let waiter = queue.pop_front().unwrap();
                to_wake.push(waiter);
            }
            // 再移动
            while !queue.is_empty() && to_move.len() < nr_requeue {
                let waiter = queue.pop_front().unwrap();
                to_move.push(waiter);
            }
            if queue.is_empty() {
                table.remove(&key1);
            }
        }

        if !to_move.is_empty() {
            let queue2 = table.entry(key2).or_insert_with(VecDeque::new);
            for waiter in to_move {
                queue2.push_back(waiter);
            }
        }
        commit_waitv_wake_selections_locked(&mut table, &mut to_wake);
    }

    let woken = to_wake.len();
    for waiter in to_wake {
        wake_futex_waiter(waiter);
    }

    Ok(woken)
}

/// FUTEX_CMP_REQUEUE
///
/// 与 REQUEUE 类似，但要求 `*uaddr == cmpval`，否则返回 `EAGAIN`。
fn futex_cmp_requeue(
    uaddr: *mut u32,
    nr_wake: usize,
    nr_requeue: usize,
    uaddr2: *mut u32,
    cmpval: u32,
    is_private: bool,
) -> SyscallResult {
    let token = current_user_token();
    let key1 = make_key(validate_futex_addr(uaddr)?, is_private)?;
    let key2 = make_key(validate_futex_addr(uaddr2)?, is_private)?;
    let mut to_wake: Vec<FutexWaiter> = Vec::new();
    let mut to_move: Vec<FutexWaiter> = Vec::new();

    {
        let mut table = FUTEX_TABLE.lock();
        let contains_pi = {
            let states = PI_STATES.lock();
            states.contains_key(&key1) || states.contains_key(&key2)
        };
        if contains_pi
            || table.get(&key1).is_some_and(|queue| {
                queue
                    .iter()
                    .any(|waiter| waiter.pi_waiter || waiter.requeue_pi_target.is_some())
            })
            || table.get(&key2).is_some_and(|queue| {
                queue
                    .iter()
                    .any(|waiter| waiter.pi_waiter || waiter.requeue_pi_target.is_some())
            })
        {
            return Err(SysError::EINVAL);
        }
        let current_val = read_user_u32_mapped(token, uaddr as usize)?;
        if current_val != cmpval {
            return Err(SysError::EAGAIN);
        }

        if let Some(queue) = table.get_mut(&key1) {
            while !queue.is_empty() && to_wake.len() < nr_wake {
                let waiter = queue.pop_front().unwrap();
                to_wake.push(waiter);
            }
            while !queue.is_empty() && to_move.len() < nr_requeue {
                let waiter = queue.pop_front().unwrap();
                to_move.push(waiter);
            }
            if queue.is_empty() {
                table.remove(&key1);
            }
        }

        if !to_move.is_empty() {
            let queue2 = table.entry(key2).or_insert_with(VecDeque::new);
            for waiter in to_move {
                queue2.push_back(waiter);
            }
        }
        commit_waitv_wake_selections_locked(&mut table, &mut to_wake);
    }

    let woken = to_wake.len();
    for waiter in to_wake {
        wake_futex_waiter(waiter);
    }

    Ok(woken)
}

/// Check and wake expired futex waiters from a scheduler safe point.
///
/// The common no-deadline path is lock- and allocation-free. This must not run
/// in a hard timer trap because an expiry can acquire task/run-queue locks.
pub fn check_futex_timeouts() {
    let next_deadline = NEXT_FUTEX_DEADLINE_NS.load(Ordering::Acquire);
    if next_deadline == NO_FUTEX_DEADLINE {
        return;
    }
    let now_ns = monotonic_now_ns();
    if now_ns < next_deadline {
        return;
    }

    loop {
        let expired_waiter = {
            let mut table = FUTEX_TABLE.lock();
            let mut expired = None;
            let mut following_deadline = NO_FUTEX_DEADLINE;
            'search: for (key, queue) in table.iter() {
                for (index, waiter) in queue.iter().enumerate() {
                    if let Some(deadline) = waiter.deadline_ns {
                        if deadline <= now_ns {
                            expired = Some((*key, index));
                            break 'search;
                        }
                        following_deadline = following_deadline.min(deadline);
                    }
                }
            }

            if let Some((key, index)) = expired {
                let (waiter, queue_empty) = {
                    let queue = table
                        .get_mut(&key)
                        .expect("expired futex queue disappeared while locked");
                    let waiter = queue
                        .remove(index)
                        .expect("expired futex waiter disappeared while locked");
                    (waiter, queue.is_empty())
                };
                if queue_empty {
                    table.remove(&key);
                }
                if waiter.wake_index != usize::MAX {
                    remove_waitv_siblings_locked(&mut table, &waiter.task);
                }
                let owner = if waiter.pi_waiter {
                    let max_waiter_priority = table
                        .get(&key)
                        .into_iter()
                        .flat_map(|queue| queue.iter())
                        .filter(|queued| queued.pi_waiter)
                        .map(|queued| queued.task.effective_sched_priority())
                        .max()
                        .unwrap_or(0);
                    PI_STATES.lock().get_mut(&key).map(|state| {
                        state.max_waiter_priority = max_waiter_priority;
                        state.owner_tid
                    })
                } else {
                    None
                };
                Some((waiter.task, owner))
            } else {
                // Publish while holding FUTEX_TABLE so a concurrent enqueue
                // can only lower this value after the completed scan.
                NEXT_FUTEX_DEADLINE_NS.store(following_deadline, Ordering::Release);
                None
            }
        };

        let Some((task, owner)) = expired_waiter else {
            break;
        };
        if let Some(owner) = owner {
            apply_pi_boost(owner);
        }
        {
            let mut inner = task.inner_exclusive_access();
            inner.futex_woken = false;
            inner.futex_timed_out = true;
        }
        wakeup_task(task);
    }
}

/// 用于线程退出时（`clear_child_tid`），唤醒等待在该地址上的 1 个线程。
/// 注意：此函数可能在 `current_task()` 为 None 时被调用（如 `exit_current_and_run_next` 中），
/// 因此需要显式传入 `pid`。
/// `paddr` 为该地址对应的物理地址，用于匹配未带 `FUTEX_PRIVATE_FLAG` 的 futex wait。
#[allow(unused)]
pub fn futex_wake_one(uaddr: usize, pid: usize, paddr: Option<usize>) -> usize {
    let _perf_timer =
        crate::task::perf_stats::scope_timer(crate::task::perf_stats::PerfTimerKind::FutexWakeOne);
    let mut to_wake: Vec<FutexWaiter> = Vec::new();

    {
        let mut table = FUTEX_TABLE.lock();
        // 先尝试 Private key
        let private_key = FutexKey::Private { pid, uaddr };
        if let Some(queue) = table.get_mut(&private_key) {
            if let Some(waiter) = queue.pop_front() {
                to_wake.push(waiter);
            }
            if queue.is_empty() {
                table.remove(&private_key);
            }
        }

        // 若 Private 未找到，且提供了物理地址，再尝试 Shared key
        //（futex_wait 不带 FUTEX_PRIVATE_FLAG 时使用 Shared key）
        if to_wake.is_empty() {
            if let Some(pa) = paddr {
                let shared_key = FutexKey::Shared { paddr: pa };
                if let Some(queue) = table.get_mut(&shared_key) {
                    if let Some(waiter) = queue.pop_front() {
                        to_wake.push(waiter);
                    }
                    if queue.is_empty() {
                        table.remove(&shared_key);
                    }
                }
            }
        }
        commit_waitv_wake_selections_locked(&mut table, &mut to_wake);
    }

    let woken = to_wake.len();
    crate::task::perf_stats::record_futex_wake_one_woken(woken);
    for waiter in to_wake {
        wake_futex_waiter(waiter);
    }
    woken
}

/// 显式指定 pid 的 futex_wake，用于线程退出时 robust list 清理。
#[allow(dead_code)]
fn futex_wake_with_pid(uaddr: *mut u32, nr_wake: usize, bitset: u32, pid: usize) -> SyscallResult {
    if bitset == 0 {
        return Err(SysError::EINVAL);
    }
    let key = FutexKey::Private {
        pid,
        uaddr: uaddr as usize,
    };
    let mut to_wake: Vec<FutexWaiter> = Vec::new();
    {
        let mut table = FUTEX_TABLE.lock();
        if let Some(queue) = table.get_mut(&key) {
            let mut remaining = VecDeque::new();
            while let Some(waiter) = queue.pop_front() {
                if to_wake.len() < nr_wake && (waiter.bitset & bitset) != 0 {
                    to_wake.push(waiter);
                } else {
                    remaining.push_back(waiter);
                }
            }
            if remaining.is_empty() {
                table.remove(&key);
            } else {
                *queue = remaining;
            }
        }
        commit_waitv_wake_selections_locked(&mut table, &mut to_wake);
    }
    let woken = to_wake.len();
    for waiter in to_wake {
        wake_futex_waiter(waiter);
    }
    Ok(woken)
}

/// 线程退出时处理 robust list，标记 owner-died 的 robust mutex 并唤醒等待者。
#[allow(unused)]
pub fn handle_robust_list_exit(
    _task: &Arc<crate::task::TaskControlBlock>,
    tid: usize,
    token: usize,
    pid: usize,
    head: usize,
    _len: usize,
) {
    if head == 0 {
        return;
    }
    let Ok(mut next_ptr) = read_user_usize_with_token(token, head) else {
        return;
    };
    let Ok(offset_bits) = read_user_usize_with_token(token, head + size_of::<usize>()) else {
        return;
    };
    let futex_offset = offset_bits as isize;
    let Ok(pending_ptr) = read_user_usize_with_token(token, head + 2 * size_of::<usize>()) else {
        return;
    };

    let mut visited = 0usize;
    while next_ptr & !1 != 0 && next_ptr & !1 != head && visited < ROBUST_LIST_LIMIT {
        visited += 1;
        let node = next_ptr & !1;
        let Ok(node_next) = read_user_usize_with_token(token, node) else {
            break;
        };
        robust_mark_owner_died(token, pid, tid, (node as isize + futex_offset) as usize);
        next_ptr = node_next;
    }

    // 处理 list_op_pending
    let pending = pending_ptr & !1;
    if pending != 0 && pending != head {
        robust_mark_owner_died(token, pid, tid, (pending as isize + futex_offset) as usize);
    }
}

fn robust_mark_owner_died(token: usize, pid: usize, tid: usize, futex_uaddr: usize) {
    let Ok(word) = user_atomic_u32(token, futex_uaddr, true) else {
        return;
    };
    let mut observed = word.load(Ordering::Acquire);
    loop {
        if observed & FUTEX_TID_MASK != tid as u32 {
            return;
        }
        // Preserve FUTEX_WAITERS and replace only the dead owner TID. A plain
        // store can otherwise overwrite a concurrent waiter publication.
        let replacement = (observed & FUTEX_WAITERS) | FUTEX_OWNER_DIED;
        match word.compare_exchange_weak(
            observed,
            replacement,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => break,
            Err(current) => observed = current,
        }
    }
    let paddr = PageTable::from_token(token)
        .translate_va(VirtAddr::from(futex_uaddr))
        .map(|pa| pa.0);
    let _ = futex_wake_one(futex_uaddr, pid, paddr);
}
