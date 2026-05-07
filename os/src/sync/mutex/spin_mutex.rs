use super::{MutexSupport, Spin, SpinNoIrq};
use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

/// A spinlock parameterized by a `MutexSupport` type.
///
/// `SpinMutex<T, Spin>` is a plain spinlock.
/// `SpinMutex<T, SpinNoIrq>` disables interrupts while locked.
pub struct SpinMutex<T: ?Sized, S: MutexSupport> {
    lock: AtomicBool,
    _marker: PhantomData<S>,
    data: UnsafeCell<T>,
}

/// RAII guard for `SpinMutex`.
pub struct SpinMutexGuard<'a, T: ?Sized, S: MutexSupport> {
    mutex: &'a SpinMutex<T, S>,
    support_guard: S::GuardData,
    _nosend: PhantomData<*mut ()>,
}

unsafe impl<T: ?Sized + Send, S: MutexSupport> Sync for SpinMutex<T, S> {}
unsafe impl<T: ?Sized + Send, S: MutexSupport> Send for SpinMutex<T, S> {}

impl<T, S: MutexSupport> SpinMutex<T, S> {
    #[inline]
    pub const fn new(user_data: T) -> Self {
        SpinMutex {
            lock: AtomicBool::new(false),
            _marker: PhantomData,
            data: UnsafeCell::new(user_data),
        }
    }

    /// Acquire the spinlock.
    #[inline]
    pub fn lock(&self) -> SpinMutexGuard<'_, T, S> {
        let support_guard = S::before_lock();
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
        SpinMutexGuard {
            mutex: self,
            support_guard,
            _nosend: PhantomData,
        }
    }

    /// Try to acquire the spinlock without blocking.
    #[inline]
    pub fn try_lock(&self) -> Option<SpinMutexGuard<'_, T, S>> {
        let support_guard = S::before_lock();
        if self
            .lock
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            Some(SpinMutexGuard {
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
                panic!("SpinMutex: deadlock detected after {:#x} retries\n", try_count);
            }
        }
    }
}

impl<'a, T: ?Sized, S: MutexSupport> Deref for SpinMutexGuard<'a, T, S> {
    type Target = T;
    #[inline(always)]
    fn deref(&self) -> &T {
        unsafe { &*self.mutex.data.get() }
    }
}

impl<'a, T: ?Sized, S: MutexSupport> DerefMut for SpinMutexGuard<'a, T, S> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<'a, T: ?Sized, S: MutexSupport> Drop for SpinMutexGuard<'a, T, S> {
    #[inline(always)]
    fn drop(&mut self) {
        self.mutex.lock.store(false, Ordering::Release);
        S::after_unlock(&mut self.support_guard);
    }
}

impl<T: core::fmt::Debug, S: MutexSupport> core::fmt::Debug for SpinMutex<T, S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let guard = self.lock();
        f.debug_struct("SpinMutex").field("data", &*guard).finish()
    }
}

/// Plain spinlock.
pub type SpinLock<T> = SpinMutex<T, Spin>;
/// Spinlock that disables interrupts while held.
pub type SpinNoIrqLock<T> = SpinMutex<T, SpinNoIrq>;
