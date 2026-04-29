use super::{MutexSupport, SpinNoIrq};
use crate::task::current_task;
use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// A reentrant mutex: the same task may lock it multiple times without
/// deadlocking. A recursion counter tracks how many times the current task
/// has entered, and the lock is only released when the outermost guard drops.
pub struct ReentrantMutex<T: ?Sized, S: MutexSupport> {
    owner: AtomicUsize,
    recurse_count: AtomicUsize,
    lock: AtomicBool,
    _marker: PhantomData<S>,
    data: UnsafeCell<T>,
}

/// RAII guard for `ReentrantMutex`.
pub struct ReentrantMutexGuard<'a, T: ?Sized, S: MutexSupport> {
    mutex: &'a ReentrantMutex<T, S>,
    support_guard: S::GuardData,
    _nosend: PhantomData<*mut ()>,
}

unsafe impl<T: ?Sized + Send, S: MutexSupport> Send for ReentrantMutex<T, S> {}
unsafe impl<T: ?Sized + Send, S: MutexSupport> Sync for ReentrantMutex<T, S> {}

impl<T, S: MutexSupport> ReentrantMutex<T, S> {
    #[inline]
    pub const fn new(user_data: T) -> Self {
        ReentrantMutex {
            owner: AtomicUsize::new(0),
            recurse_count: AtomicUsize::new(0),
            lock: AtomicBool::new(false),
            _marker: PhantomData,
            data: UnsafeCell::new(user_data),
        }
    }

    #[inline]
    pub fn lock(&self) -> ReentrantMutexGuard<'_, T, S> {
        let support_guard = S::before_lock();
        let tid = current_tid();

        if self.owner.load(Ordering::Relaxed) == tid {
            // Same task already holds the lock: increment recursion count.
            self.recurse_count.fetch_add(1, Ordering::Relaxed);
            return ReentrantMutexGuard {
                mutex: self,
                support_guard,
                _nosend: PhantomData,
            };
        }

        loop {
            self.wait_unlock();
            if self
                .lock
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
        }
        self.owner.store(tid, Ordering::Relaxed);
        self.recurse_count.store(1, Ordering::Relaxed);
        ReentrantMutexGuard {
            mutex: self,
            support_guard,
            _nosend: PhantomData,
        }
    }

    #[inline]
    pub fn try_lock(&self) -> Option<ReentrantMutexGuard<'_, T, S>> {
        let support_guard = S::before_lock();
        let tid = current_tid();

        if self.owner.load(Ordering::Relaxed) == tid {
            self.recurse_count.fetch_add(1, Ordering::Relaxed);
            return Some(ReentrantMutexGuard {
                mutex: self,
                support_guard,
                _nosend: PhantomData,
            });
        }

        if self
            .lock
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            self.owner.store(tid, Ordering::Relaxed);
            self.recurse_count.store(1, Ordering::Relaxed);
            Some(ReentrantMutexGuard {
                mutex: self,
                support_guard,
                _nosend: PhantomData,
            })
        } else {
            None
        }
    }

    #[inline(always)]
    fn wait_unlock(&self) {
        let mut try_count = 0usize;
        while self.lock.load(Ordering::Relaxed) {
            core::hint::spin_loop();
            try_count += 1;
            if try_count == 0x10000000 {
                panic!(
                    "ReentrantMutex: deadlock detected after {:#x} retries\n",
                    try_count
                );
            }
        }
    }
}

impl<'a, T: ?Sized, S: MutexSupport> Deref for ReentrantMutexGuard<'a, T, S> {
    type Target = T;
    #[inline(always)]
    fn deref(&self) -> &T {
        unsafe { &*self.mutex.data.get() }
    }
}

impl<'a, T: ?Sized, S: MutexSupport> DerefMut for ReentrantMutexGuard<'a, T, S> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<'a, T: ?Sized, S: MutexSupport> Drop for ReentrantMutexGuard<'a, T, S> {
    #[inline]
    fn drop(&mut self) {
        let count = self.mutex.recurse_count.fetch_sub(1, Ordering::Release);
        if count == 1 {
            // Outermost guard: really release the lock.
            self.mutex.owner.store(0, Ordering::Relaxed);
            self.mutex.lock.store(false, Ordering::Release);
        }
        S::after_unlock(&mut self.support_guard);
    }
}

/// Reentrant lock that disables interrupts around the critical section.
pub type ReentrantLock<T> = ReentrantMutex<T, SpinNoIrq>;

/// Helper: get current task's TID.
#[inline]
fn current_tid() -> usize {
    // Safety: if called from a normal task context, current_task() is Some.
    // In interrupt/idle context it may be None; callers should handle that.
    current_task()
        .map(|t| t.inner_exclusive_access().res.as_ref().unwrap().tid)
        .unwrap_or(usize::MAX)
}
