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
use crate::task::{block_current_and_run_next, current_process, current_task, wakeup_task};
use alloc::collections::{BTreeMap, VecDeque};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use lazy_static::lazy_static;
use log::warn;
use log::{error, info};
use polyhal::timer::current_time;

const FUTEX_OWNER_DIED: u32 = 0x40000000;
const ROBUST_LIST_LIMIT: usize = 2048;

// Linux futex 操作码
const FUTEX_WAIT: i32 = 0;
const FUTEX_WAKE: i32 = 1;
const FUTEX_REQUEUE: i32 = 3;
const FUTEX_CMP_REQUEUE: i32 = 4;
const FUTEX_WAIT_BITSET: i32 = 9;
const FUTEX_WAKE_BITSET: i32 = 10;
const FUTEX_PRIVATE_FLAG: i32 = 128;
const FUTEX_CLOCK_REALTIME: i32 = 256;

const FUTEX_BITSET_MATCH_ANY: u32 = 0xffffffff;

/// futex 等待队列中的一个条目
pub struct FutexWaiter {
    task: Arc<crate::task::TaskControlBlock>,
    bitset: u32,
    /// Monotonic absolute timeout in nanoseconds, or `None` for no timeout.
    deadline_ns: Option<u64>,
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
    Ok(unsafe { core::ptr::read_volatile(pa.get_mut_ptr::<u32>()) })
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
    if clock_realtime && !matches!(op, FUTEX_WAIT | FUTEX_WAIT_BITSET) {
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
        _ => {
            error!("Unsupported futex op: {}", op);
            Err(SysError::ENOSYS)
        }
    }
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

/// 从 futex 等待队列中移除指定任务
fn remove_task_from_futex_queue(key: &FutexKey, task: &Arc<crate::task::TaskControlBlock>) {
    let mut table = FUTEX_TABLE.lock();
    if let Some(queue) = table.get_mut(key) {
        let mut remaining = VecDeque::new();
        while let Some(waiter) = queue.pop_front() {
            if Arc::ptr_eq(&waiter.task, task) {
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
    for key in keys {
        let should_remove = if let Some(queue) = table.get_mut(&key) {
            queue.retain(|waiter| Arc::as_ptr(&waiter.task) != task_ptr);
            queue.is_empty()
        } else {
            false
        };
        if should_remove {
            table.remove(&key);
        }
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
) -> SyscallResult {
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

    // 1. Resolve the Linux timeout ABI outside the futex-table lock. WAIT uses
    // a relative duration, while WAIT_BITSET uses an absolute clock deadline.
    let deadline_ns = parse_futex_deadline(token, timeout, timeout_mode)?;

    // 2. Linux futex WAIT 的核心语义是“原子比较并阻塞”：
    //    wake/requeue 也持有 FUTEX_TABLE，因此 signal 不能滑过比较和入队之间。
    {
        let mut table = FUTEX_TABLE.lock();
        let current_val = read_user_u32_mapped(token, uaddr_usize)?;
        if current_val != val {
            info!(
                "futex_wait: val mismatch, expected {}, got {}, returning EAGAIN",
                val, current_val
            );
            return Err(SysError::EAGAIN);
        }
        {
            let mut t_inner = task.inner_exclusive_access();
            t_inner.futex_woken = false;
            t_inner.futex_timed_out = false;
        }
        if deadline_ns.is_some_and(|deadline| monotonic_now_ns() >= deadline) {
            return Err(SysError::ETIMEDOUT);
        }
        let queue = table.entry(key).or_insert_with(VecDeque::new);
        queue.push_back(FutexWaiter {
            task: task.clone(),
            bitset,
            deadline_ns,
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
                remove_task_from_futex_queue(&key, &task);
                return Err(SysError::EINTR);
            }

            // 如果进程已被 exit_group 等标记为 zombie，不再阻塞
            if t_inner
                .zombie_flag
                .load(core::sync::atomic::Ordering::SeqCst)
            {
                drop(t_inner);
                remove_task_from_futex_queue(&key, &task);
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
    if bitset == 0 {
        return Err(SysError::EINVAL);
    }

    let key = make_key(validate_futex_addr(uaddr)?, is_private)?;
    info!(
        "futex_wake: addr={:p}, nr_wake={}, key={:?}",
        uaddr, nr_wake, key
    );
    let mut to_wake: Vec<Arc<crate::task::TaskControlBlock>> = Vec::new();

    {
        let mut table = FUTEX_TABLE.lock();
        if let Some(queue) = table.get_mut(&key) {
            let mut remaining = VecDeque::new();
            while let Some(waiter) = queue.pop_front() {
                if to_wake.len() < nr_wake && (waiter.bitset & bitset) != 0 {
                    // 标记为已唤醒，防止丢失唤醒
                    waiter.task.inner_exclusive_access().futex_woken = true;
                    to_wake.push(waiter.task);
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
    }

    let woken = to_wake.len();
    for task in to_wake {
        wakeup_task(task);
    }

    Ok(woken)
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
    let mut to_wake: Vec<Arc<crate::task::TaskControlBlock>> = Vec::new();
    let mut to_move: Vec<FutexWaiter> = Vec::new();

    {
        let mut table = FUTEX_TABLE.lock();
        if let Some(queue) = table.get_mut(&key1) {
            // 先唤醒
            while !queue.is_empty() && to_wake.len() < nr_wake {
                let waiter = queue.pop_front().unwrap();
                waiter.task.inner_exclusive_access().futex_woken = true;
                to_wake.push(waiter.task);
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
    }

    let woken = to_wake.len();
    for task in to_wake {
        wakeup_task(task);
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
    let mut to_wake: Vec<Arc<crate::task::TaskControlBlock>> = Vec::new();
    let mut to_move: Vec<FutexWaiter> = Vec::new();

    {
        let mut table = FUTEX_TABLE.lock();
        let current_val = read_user_u32_mapped(token, uaddr as usize)?;
        if current_val != cmpval {
            return Err(SysError::EAGAIN);
        }

        if let Some(queue) = table.get_mut(&key1) {
            while !queue.is_empty() && to_wake.len() < nr_wake {
                let waiter = queue.pop_front().unwrap();
                waiter.task.inner_exclusive_access().futex_woken = true;
                to_wake.push(waiter.task);
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
    }

    let woken = to_wake.len();
    for task in to_wake {
        wakeup_task(task);
    }

    Ok(woken)
}

/// Check and wake expired futex waiters from either timer interrupt context or
/// the idle scheduler. The common no-deadline path is lock- and allocation-free.
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
        let expired_task = {
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
                let (task, queue_empty) = {
                    let queue = table
                        .get_mut(&key)
                        .expect("expired futex queue disappeared while locked");
                    let waiter = queue
                        .remove(index)
                        .expect("expired futex waiter disappeared while locked");
                    (waiter.task, queue.is_empty())
                };
                if queue_empty {
                    table.remove(&key);
                }
                {
                    let mut inner = task.inner_exclusive_access();
                    inner.futex_woken = false;
                    inner.futex_timed_out = true;
                }
                Some(task)
            } else {
                // Publish while holding FUTEX_TABLE so a concurrent enqueue
                // can only lower this value after the completed scan.
                NEXT_FUTEX_DEADLINE_NS.store(following_deadline, Ordering::Release);
                None
            }
        };

        let Some(task) = expired_task else {
            break;
        };
        wakeup_task(task);
    }
}

/// 用于线程退出时（`clear_child_tid`），唤醒等待在该地址上的 1 个线程。
/// 注意：此函数可能在 `current_task()` 为 None 时被调用（如 `exit_current_and_run_next` 中），
/// 因此需要显式传入 `pid`。
/// `paddr` 为该地址对应的物理地址，用于匹配未带 `FUTEX_PRIVATE_FLAG` 的 futex wait。
#[allow(unused)]
pub fn futex_wake_one(uaddr: usize, pid: usize, paddr: Option<usize>) -> usize {
    let mut to_wake: Vec<Arc<crate::task::TaskControlBlock>> = Vec::new();

    {
        let mut table = FUTEX_TABLE.lock();
        // 先尝试 Private key
        let private_key = FutexKey::Private { pid, uaddr };
        if let Some(queue) = table.get_mut(&private_key) {
            if let Some(waiter) = queue.pop_front() {
                waiter.task.inner_exclusive_access().futex_woken = true;
                to_wake.push(waiter.task);
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
                        waiter.task.inner_exclusive_access().futex_woken = true;
                        to_wake.push(waiter.task);
                    }
                    if queue.is_empty() {
                        table.remove(&shared_key);
                    }
                }
            }
        }
    }

    let woken = to_wake.len();
    for task in to_wake {
        wakeup_task(task);
    }
    woken
}

/// 显式指定 pid 的 futex_wake，用于线程退出时 robust list 清理。
fn futex_wake_with_pid(uaddr: *mut u32, nr_wake: usize, bitset: u32, pid: usize) -> SyscallResult {
    if bitset == 0 {
        return Err(SysError::EINVAL);
    }
    let key = FutexKey::Private {
        pid,
        uaddr: uaddr as usize,
    };
    let mut to_wake: Vec<Arc<crate::task::TaskControlBlock>> = Vec::new();
    {
        let mut table = FUTEX_TABLE.lock();
        if let Some(queue) = table.get_mut(&key) {
            let mut remaining = VecDeque::new();
            while let Some(waiter) = queue.pop_front() {
                if to_wake.len() < nr_wake && (waiter.bitset & bitset) != 0 {
                    waiter.task.inner_exclusive_access().futex_woken = true;
                    to_wake.push(waiter.task);
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
    }
    let woken = to_wake.len();
    for task in to_wake {
        wakeup_task(task);
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

    // robust_list_head 布局：
    //   0..8  : list.next  (struct robust_list *)
    //   8..16 : futex_offset (long)
    //   16..24: list_op_pending (struct robust_list *)
    let head_buf =
        match translated_byte_buffer_no_fault(token, head as *const u8, 3 * size_of::<usize>()) {
            Ok(buf) => buf,
            Err(_) => return,
        };
    if head_buf.is_empty() || head_buf[0].len() < 3 * size_of::<usize>() {
        return;
    }
    let head_slice = &head_buf[0][..3 * size_of::<usize>()];
    let mut next_ptr = usize::from_ne_bytes([
        head_slice[0],
        head_slice[1],
        head_slice[2],
        head_slice[3],
        head_slice[4],
        head_slice[5],
        head_slice[6],
        head_slice[7],
    ]);
    let futex_offset = isize::from_ne_bytes([
        head_slice[8],
        head_slice[9],
        head_slice[10],
        head_slice[11],
        head_slice[12],
        head_slice[13],
        head_slice[14],
        head_slice[15],
    ]);
    let pending_ptr = usize::from_ne_bytes([
        head_slice[16],
        head_slice[17],
        head_slice[18],
        head_slice[19],
        head_slice[20],
        head_slice[21],
        head_slice[22],
        head_slice[23],
    ]);

    let mut visited = 0usize;
    while next_ptr != 0 && next_ptr != head && visited < ROBUST_LIST_LIMIT {
        visited += 1;
        // 读取 robust_list 节点：0..8 = next
        let node_buf =
            match translated_byte_buffer_no_fault(token, next_ptr as *const u8, size_of::<usize>())
            {
                Ok(buf) => buf,
                Err(_) => break,
            };
        if node_buf.is_empty() || node_buf[0].len() < size_of::<usize>() {
            break;
        }
        let node_next = usize::from_ne_bytes([
            node_buf[0][0],
            node_buf[0][1],
            node_buf[0][2],
            node_buf[0][3],
            node_buf[0][4],
            node_buf[0][5],
            node_buf[0][6],
            node_buf[0][7],
        ]);

        let futex_uaddr = (next_ptr as isize + futex_offset) as usize;
        if let Ok(val) = read_user_u32_with_token(token, futex_uaddr as *const u32) {
            if (val & 0x3fffffff) == tid as u32 {
                let new_val = FUTEX_OWNER_DIED;
                // 尝试写入用户内存
                let mut buf =
                    match translated_byte_buffer_no_fault(token, futex_uaddr as *const u8, 4) {
                        Ok(buf) => buf,
                        Err(_) => return,
                    };
                if !buf.is_empty() && buf[0].len() >= 4 {
                    buf[0][..4].copy_from_slice(&new_val.to_ne_bytes());
                    let _ = futex_wake_with_pid(
                        futex_uaddr as *mut u32,
                        1,
                        FUTEX_BITSET_MATCH_ANY,
                        pid,
                    );
                }
            }
        }

        next_ptr = node_next;
    }

    // 处理 list_op_pending
    if pending_ptr != 0 && pending_ptr != head {
        let futex_uaddr = (pending_ptr as isize + futex_offset) as usize;
        if let Ok(val) = read_user_u32_with_token(token, futex_uaddr as *const u32) {
            if (val & 0x3fffffff) == tid as u32 {
                let new_val = FUTEX_OWNER_DIED;
                let mut buf =
                    match translated_byte_buffer_no_fault(token, futex_uaddr as *const u8, 4) {
                        Ok(buf) => buf,
                        Err(_) => return,
                    };
                if !buf.is_empty() && buf[0].len() >= 4 {
                    buf[0][..4].copy_from_slice(&new_val.to_ne_bytes());
                    let _ = futex_wake_with_pid(
                        futex_uaddr as *mut u32,
                        1,
                        FUTEX_BITSET_MATCH_ANY,
                        pid,
                    );
                }
            }
        }
    }
}
