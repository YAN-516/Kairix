use super::{MutexSupport, SpinNoIrq};
use crate::sync::mutex::spin_mutex::SpinMutex;
use crate::task::{TaskControlBlock, block_current_and_run_next, current_task, wakeup_task};
use alloc::collections::VecDeque;
use alloc::sync::{Arc, Weak};
use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};

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
    data: UnsafeCell<T>,
}

struct BlockingInner {
    locked: bool,
    handoff: Option<Weak<TaskControlBlock>>,
    wait_queue: VecDeque<Weak<TaskControlBlock>>,
}

/// RAII guard for `BlockingMutex`.
pub struct BlockingMutexGuard<'a, T: ?Sized, S: MutexSupport> {
    mutex: &'a BlockingMutex<T, S>,
    _nosend: PhantomData<*mut ()>,
}

unsafe impl<T: ?Sized + Send, S: MutexSupport> Send for BlockingMutex<T, S> {}
unsafe impl<T: ?Sized + Send, S: MutexSupport> Sync for BlockingMutex<T, S> {}

impl<T, S: MutexSupport> BlockingMutex<T, S> {
    #[inline]
    pub const fn new(user_data: T) -> Self {
        BlockingMutex {
            inner: SpinMutex::new(BlockingInner {
                locked: false,
                handoff: None,
                wait_queue: VecDeque::new(),
            }),
            data: UnsafeCell::new(user_data),
        }
    }

    /// Acquire the lock, blocking the current task if necessary.
    #[inline]
    pub fn lock(&self) -> BlockingMutexGuard<'_, T, S> {
        let mut waiting_task: Option<Arc<TaskControlBlock>> = None;
        loop {
            let mut inner = self.inner.lock();
            let current = match waiting_task.as_ref() {
                Some(task) => Some(Arc::clone(task)),
                None => current_task().inspect(|task| {
                    waiting_task = Some(Arc::clone(task));
                }),
            };

            let handoff_target = inner.handoff.as_ref().and_then(Weak::upgrade);
            if let (Some(target), Some(task)) = (handoff_target.as_ref(), current.as_ref()) {
                if Arc::ptr_eq(target, task) {
                    inner.handoff = None;
                    inner.locked = true;
                    break;
                }
            } else if inner.handoff.is_some() && handoff_target.is_none() {
                inner.handoff = None;
                inner.locked = false;
            }

            if !inner.locked && inner.handoff.is_none() {
                inner.locked = true;
                break;
            }

            let Some(task) = current else {
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
    pub fn try_lock(&self) -> Option<BlockingMutexGuard<'_, T, S>> {
        let mut inner = self.inner.lock();
        if inner.handoff.is_some() {
            if let Some(target) = inner.handoff.as_ref().and_then(Weak::upgrade) {
                if current_task()
                    .as_ref()
                    .is_some_and(|task| Arc::ptr_eq(task, &target))
                {
                    inner.handoff = None;
                    inner.locked = true;
                    return Some(BlockingMutexGuard {
                        mutex: self,
                        _nosend: PhantomData,
                    });
                }
                return None;
            }
            inner.handoff = None;
            inner.locked = false;
        }
        if inner.locked {
            return None;
        }
        inner.locked = true;
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
        while let Some(task) = inner.wait_queue.pop_front() {
            if let Some(task) = task.upgrade() {
                // Reserve the lock for the selected waiter so heavy contention
                // cannot starve it by letting later arrivals barge in first.
                inner.locked = true;
                inner.handoff = Some(Arc::downgrade(&task));
                drop(inner);
                wakeup_task(task);
                return;
            }
        }
        inner.handoff = None;
        inner.locked = false;
    }
}

/// Blocking mutex that disables interrupts while manipulating the wait queue.
pub type SleepLock<T> = BlockingMutex<T, SpinNoIrq>;
