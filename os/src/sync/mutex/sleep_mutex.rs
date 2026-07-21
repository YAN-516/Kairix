use super::{MutexSupport, SpinNoIrq};
use crate::sync::mutex::spin_mutex::SpinMutex;
use crate::task::{
    TaskControlBlock, block_current_and_run_next, current_task, manager::wakeup_task_front,
};
use alloc::collections::VecDeque;
use alloc::sync::{Arc, Weak};
use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::panic::Location;
use core::sync::atomic::{AtomicUsize, Ordering};

/// A mutex that blocks the current task instead of spinning.
///
/// When contention occurs, the current task is moved to a wait queue and
/// another task is scheduled. When the lock is released, the lock becomes
/// available and one waiter is woken to retry acquisition.
///
/// Internally protected by a `SpinMutex` so the wait queue itself is safe
/// under multi-core contention.
pub struct BlockingMutex<T: ?Sized, S: MutexSupport> {
    inner: SpinMutex<BlockingInner, S>,
    fair: bool,
    owner_hart: AtomicUsize,
    owner_pid: AtomicUsize,
    owner_line: AtomicUsize,
    data: UnsafeCell<T>,
}

struct BlockingInner {
    locked: bool,
    handoff: Option<Weak<TaskControlBlock>>,
    handoff_started_ns: usize,
    wait_queue: VecDeque<Weak<TaskControlBlock>>,
}

/// A bounded reservation prevents active callers from repeatedly overtaking a
/// woken waiter while still allowing recovery if that waiter exits or cannot
/// run. This is intentionally short relative to filesystem operation latency.
const FAIR_HANDOFF_TIMEOUT_NS: usize = 10_000_000;

fn monotonic_now_ns() -> usize {
    polyhal::timer::current_time().as_nanos() as usize
}

/// Non-blocking diagnostic snapshot of a [`BlockingMutex`].
#[derive(Debug, Clone, Copy)]
pub struct BlockingMutexStats {
    pub inner_busy: bool,
    pub locked: bool,
    pub handoff: bool,
    pub waiters: usize,
    pub live_waiters: usize,
    pub owner_hart: usize,
    pub owner_pid: usize,
    pub owner_line: usize,
}

/// RAII guard for `BlockingMutex`.
pub struct BlockingMutexGuard<'a, T: ?Sized, S: MutexSupport> {
    mutex: &'a BlockingMutex<T, S>,
    _nosend: PhantomData<*mut ()>,
}

unsafe impl<T: ?Sized + Send, S: MutexSupport> Send for BlockingMutex<T, S> {}
unsafe impl<T: ?Sized + Send, S: MutexSupport> Sync for BlockingMutex<T, S> {}

impl<T: ?Sized, S: MutexSupport> BlockingMutex<T, S> {
    /// Return lock state without waiting for the internal wait-queue lock.
    pub fn stats(&self) -> BlockingMutexStats {
        let Some(inner) = self.inner.try_lock() else {
            return BlockingMutexStats {
                inner_busy: true,
                locked: false,
                handoff: false,
                waiters: 0,
                live_waiters: 0,
                owner_hart: self.owner_hart.load(Ordering::Acquire),
                owner_pid: self.owner_pid.load(Ordering::Acquire),
                owner_line: self.owner_line.load(Ordering::Acquire),
            };
        };
        BlockingMutexStats {
            inner_busy: false,
            locked: inner.locked,
            handoff: inner
                .handoff
                .as_ref()
                .is_some_and(|task| task.strong_count() > 0),
            waiters: inner.wait_queue.len(),
            live_waiters: inner
                .wait_queue
                .iter()
                .filter(|task| task.strong_count() > 0)
                .count(),
            owner_hart: self.owner_hart.load(Ordering::Acquire),
            owner_pid: self.owner_pid.load(Ordering::Acquire),
            owner_line: self.owner_line.load(Ordering::Acquire),
        }
    }
}

impl<T, S: MutexSupport> BlockingMutex<T, S> {
    #[inline]
    pub const fn new(user_data: T) -> Self {
        Self::new_with_fairness(user_data, false)
    }

    /// Create a blocking mutex that gives a bounded acquisition reservation to
    /// the oldest live waiter when the current owner releases the lock.
    #[inline]
    pub const fn new_fair(user_data: T) -> Self {
        Self::new_with_fairness(user_data, true)
    }

    const fn new_with_fairness(user_data: T, fair: bool) -> Self {
        BlockingMutex {
            inner: SpinMutex::new(BlockingInner {
                locked: false,
                handoff: None,
                handoff_started_ns: 0,
                wait_queue: VecDeque::new(),
            }),
            fair,
            owner_hart: AtomicUsize::new(usize::MAX),
            owner_pid: AtomicUsize::new(usize::MAX),
            owner_line: AtomicUsize::new(0),
            data: UnsafeCell::new(user_data),
        }
    }

    fn current_owner_identity() -> (Option<Arc<TaskControlBlock>>, usize) {
        let task = current_task();
        let pid = task
            .as_ref()
            .and_then(|task| task.process.upgrade())
            .map(|process| process.getpid())
            .unwrap_or(usize::MAX);
        (task, pid)
    }

    /// Return whether `waiting_task` may acquire an unlocked fair mutex.
    /// Expired or dead reservations are cleared so a cancelled waiter cannot
    /// strand the lock indefinitely.
    fn handoff_allows_acquire(
        &self,
        inner: &mut BlockingInner,
        waiting_task: Option<&Arc<TaskControlBlock>>,
    ) -> bool {
        if !self.fair {
            return true;
        }
        let Some(reserved) = inner.handoff.as_ref().and_then(Weak::upgrade) else {
            inner.handoff = None;
            inner.handoff_started_ns = 0;
            return true;
        };
        if waiting_task.is_some_and(|task| Arc::ptr_eq(task, &reserved)) {
            inner.handoff = None;
            inner.handoff_started_ns = 0;
            return true;
        }
        let elapsed = monotonic_now_ns().saturating_sub(inner.handoff_started_ns);
        if elapsed >= FAIR_HANDOFF_TIMEOUT_NS {
            inner.handoff = None;
            inner.handoff_started_ns = 0;
            return true;
        }
        false
    }

    /// Acquire the lock, blocking the current task if necessary.
    #[inline]
    #[track_caller]
    pub fn lock(&self) -> BlockingMutexGuard<'_, T, S> {
        // Resolve the current task before taking the wait-queue spinlock.  The
        // old order nested PROCESSORS below BlockingMutex::inner and could
        // invert against scheduler-side diagnostics and wakeup paths.
        let (waiting_task, owner_pid) = Self::current_owner_identity();
        let owner_line = Location::caller().line() as usize;
        loop {
            let mut inner = self.inner.lock();

            if !inner.locked && self.handoff_allows_acquire(&mut inner, waiting_task.as_ref()) {
                inner.locked = true;
                self.owner_hart
                    .store(polyhal::arch::hart_id(), Ordering::Relaxed);
                self.owner_pid.store(owner_pid, Ordering::Relaxed);
                self.owner_line.store(owner_line, Ordering::Release);
                break;
            }

            let Some(task) = waiting_task.as_ref().map(Arc::clone) else {
                drop(inner);
                core::hint::spin_loop();
                continue;
            };
            let still_queued = inner.wait_queue.iter().any(|queued| {
                queued
                    .upgrade()
                    .is_some_and(|queued| Arc::ptr_eq(&queued, &task))
            });
            if !still_queued {
                inner.wait_queue.push_back(Arc::downgrade(&task));
            }
            drop(inner); // release the inner spinlock BEFORE blocking
            block_current_and_run_next();
        }
        BlockingMutexGuard {
            mutex: self,
            _nosend: PhantomData,
        }
    }

    /// Try to acquire without blocking.
    #[inline]
    #[track_caller]
    pub fn try_lock(&self) -> Option<BlockingMutexGuard<'_, T, S>> {
        let owner_line = Location::caller().line() as usize;
        let mut inner = self.inner.lock();
        if inner.locked || !self.handoff_allows_acquire(&mut inner, None) {
            return None;
        }
        inner.locked = true;
        self.owner_hart
            .store(polyhal::arch::hart_id(), Ordering::Relaxed);
        // Keep try_lock genuinely non-blocking: resolving current_task() may
        // wait for the per-CPU PROCESSORS lock. Hart and source line remain
        // sufficient to identify this rare task-less/diagnostic acquisition.
        self.owner_pid.store(usize::MAX, Ordering::Relaxed);
        self.owner_line.store(owner_line, Ordering::Release);
        Some(BlockingMutexGuard {
            mutex: self,
            _nosend: PhantomData,
        })
    }
}

impl<'a, T: ?Sized, S: MutexSupport> Deref for BlockingMutexGuard<'a, T, S> {
    type Target = T;
    #[inline(always)]
    fn deref(&self) -> &T {
        unsafe { &*self.mutex.data.get() }
    }
}

impl<'a, T: ?Sized, S: MutexSupport> DerefMut for BlockingMutexGuard<'a, T, S> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<'a, T: ?Sized, S: MutexSupport> Drop for BlockingMutexGuard<'a, T, S> {
    #[inline]
    fn drop(&mut self) {
        let mut inner = self.mutex.inner.lock();
        self.mutex.owner_hart.store(usize::MAX, Ordering::Relaxed);
        self.mutex.owner_pid.store(usize::MAX, Ordering::Relaxed);
        self.mutex.owner_line.store(0, Ordering::Release);
        inner.locked = false;
        loop {
            let next = loop {
                let Some(task) = inner.wait_queue.pop_front() else {
                    inner.handoff = None;
                    inner.handoff_started_ns = 0;
                    return;
                };
                let Some(task) = task.upgrade() else {
                    continue;
                };
                if task.exec_exit_requested() || task.process.upgrade().is_none() {
                    continue;
                }
                break task;
            };
            if self.mutex.fair {
                inner.handoff = Some(Arc::downgrade(&next));
                inner.handoff_started_ns = monotonic_now_ns();
            }
            drop(inner);
            if wakeup_task_front(Arc::clone(&next)) {
                return;
            }
            inner = self.mutex.inner.lock();
            let reserved_for_failed_task = inner
                .handoff
                .as_ref()
                .and_then(Weak::upgrade)
                .is_some_and(|reserved| Arc::ptr_eq(&reserved, &next));
            if reserved_for_failed_task {
                inner.handoff = None;
                inner.handoff_started_ns = 0;
            }
        }
    }
}

/// Blocking mutex that disables interrupts while manipulating the wait queue.
pub type SleepLock<T> = BlockingMutex<T, SpinNoIrq>;
