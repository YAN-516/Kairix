use super::{MutexSupport, SpinNoIrq};
use crate::sync::mutex::spin_mutex::SpinMutex;
use crate::task::{TaskControlBlock, block_current_and_run_next, current_task, wakeup_task};
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};

/// A mutex that blocks the current task instead of spinning.
///
/// When contention occurs, the current task is moved to a wait queue and
/// another task is scheduled. When the lock is released, the next waiter is
/// woken up.
///
/// Internally protected by a `SpinMutex` so the wait queue itself is safe
/// under multi-core contention.
pub struct BlockingMutex<T: ?Sized, S: MutexSupport> {
    inner: SpinMutex<BlockingInner, S>,
    data: UnsafeCell<T>,
}

struct BlockingInner {
    locked: bool,
    wait_queue: VecDeque<Arc<TaskControlBlock>>,
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
                wait_queue: VecDeque::new(),
            }),
            data: UnsafeCell::new(user_data),
        }
    }

    /// Acquire the lock, blocking the current task if necessary.
    #[inline]
    pub fn lock(&self) -> BlockingMutexGuard<'_, T, S> {
        let mut inner = self.inner.lock();
        if inner.locked {
            let task = current_task().expect("BlockingMutex::lock called without current task");
            inner.wait_queue.push_back(task);
            drop(inner); // release the inner spinlock BEFORE blocking
            block_current_and_run_next();
        } else {
            inner.locked = true;
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
        if inner.locked {
            None
        } else {
            inner.locked = true;
            Some(BlockingMutexGuard {
                mutex: self,
                _nosend: PhantomData,
            })
        }
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
        if let Some(task) = inner.wait_queue.pop_front() {
            // Hand the lock directly to the next waiter.
            drop(inner);
            wakeup_task(task);
        } else {
            inner.locked = false;
        }
    }
}

/// Blocking mutex that disables interrupts while manipulating the wait queue.
pub type SleepLock<T> = BlockingMutex<T, SpinNoIrq>;
