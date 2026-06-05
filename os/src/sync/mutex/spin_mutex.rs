use super::{MutexSupport, Spin, SpinNoIrq};
use core::cell::UnsafeCell;
use core::panic::Location;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[inline]
fn current_hart() -> usize {
    #[cfg(target_arch = "riscv64")]
    {
        use polyhal::arch::hart_id;
        hart_id()
    }
    #[cfg(target_arch = "loongarch64")]
    {
        use polyhal::arch::hart_id;
        hart_id()
    }
    #[cfg(not(any(target_arch = "riscv64", target_arch = "loongarch64")))]
    {
        0
    }
}

fn file_from_parts(ptr: usize, len: usize) -> &'static str {
    if ptr == 0 || len == 0 {
        "<unknown>"
    } else {
        unsafe { core::str::from_utf8_unchecked(core::slice::from_raw_parts(ptr as *const u8, len)) }
    }
}

/// A spinlock parameterized by a `MutexSupport` type.
///
/// `SpinMutex<T, Spin>` is a plain spinlock.
/// `SpinMutex<T, SpinNoIrq>` disables interrupts while locked.
pub struct SpinMutex<T: ?Sized, S: MutexSupport> {
    lock: AtomicBool,
    owner_hart: AtomicUsize,
    owner_file: AtomicUsize,
    owner_file_len: AtomicUsize,
    owner_line: AtomicUsize,
    waiter_hart: AtomicUsize,
    waiter_file: AtomicUsize,
    waiter_file_len: AtomicUsize,
    waiter_line: AtomicUsize,
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
            owner_hart: AtomicUsize::new(usize::MAX),
            owner_file: AtomicUsize::new(0),
            owner_file_len: AtomicUsize::new(0),
            owner_line: AtomicUsize::new(0),
            waiter_hart: AtomicUsize::new(usize::MAX),
            waiter_file: AtomicUsize::new(0),
            waiter_file_len: AtomicUsize::new(0),
            waiter_line: AtomicUsize::new(0),
            _marker: PhantomData,
            data: UnsafeCell::new(user_data),
        }
    }

    /// Acquire the spinlock.
    #[inline]
    #[track_caller]
    pub fn lock(&self) -> SpinMutexGuard<'_, T, S> {
        let support_guard = S::before_lock();
        let caller = Location::caller();
        let caller_file = caller.file().as_ptr() as usize;
        let caller_file_len = caller.file().len();
        let caller_line = caller.line() as usize;
        loop {
            self.wait_unlock();
            if self
                .lock
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                self.owner_hart.store(current_hart(), Ordering::Relaxed);
                self.owner_file.store(caller_file, Ordering::Relaxed);
                self.owner_file_len
                    .store(caller_file_len, Ordering::Relaxed);
                self.owner_line.store(caller_line, Ordering::Relaxed);
                self.waiter_hart.store(usize::MAX, Ordering::Relaxed);
                self.waiter_file.store(0, Ordering::Relaxed);
                self.waiter_file_len.store(0, Ordering::Relaxed);
                self.waiter_line.store(0, Ordering::Relaxed);
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
    #[track_caller]
    pub fn try_lock(&self) -> Option<SpinMutexGuard<'_, T, S>> {
        let mut support_guard = S::before_lock();
        if self
            .lock
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            let caller = Location::caller();
            self.owner_hart.store(current_hart(), Ordering::Relaxed);
            self.owner_file
                .store(caller.file().as_ptr() as usize, Ordering::Relaxed);
            self.owner_file_len
                .store(caller.file().len(), Ordering::Relaxed);
            self.owner_line
                .store(caller.line() as usize, Ordering::Relaxed);
            Some(SpinMutexGuard {
                mutex: self,
                support_guard,
                _nosend: PhantomData,
            })
        } else {
            S::after_unlock(&mut support_guard);
            None
        }
    }

    #[inline(always)]
    #[track_caller]
    fn wait_unlock(&self) {
        let caller = Location::caller();
        self.waiter_hart.store(current_hart(), Ordering::Relaxed);
        self.waiter_file
            .store(caller.file().as_ptr() as usize, Ordering::Relaxed);
        self.waiter_file_len
            .store(caller.file().len(), Ordering::Relaxed);
        self.waiter_line
            .store(caller.line() as usize, Ordering::Relaxed);
        let mut try_count = 0usize;
        while self.lock.load(Ordering::Relaxed) {
            core::hint::spin_loop();
            try_count += 1;
            if try_count == 0x10000000 {
                let owner_file = file_from_parts(
                    self.owner_file.load(Ordering::Relaxed),
                    self.owner_file_len.load(Ordering::Relaxed),
                );
                let waiter_file = file_from_parts(
                    self.waiter_file.load(Ordering::Relaxed),
                    self.waiter_file_len.load(Ordering::Relaxed),
                );
                panic!(
                    "SpinMutex: deadlock detected after {:#x} retries on hart {} at addr {:p} type={} owner_hart={} owner={}:{} waiter_hart={} waiter={}:{}\n",
                    try_count,
                    current_hart(),
                    self,
                    core::any::type_name::<T>(),
                    self.owner_hart.load(Ordering::Relaxed),
                    owner_file,
                    self.owner_line.load(Ordering::Relaxed),
                    self.waiter_hart.load(Ordering::Relaxed),
                    waiter_file,
                    self.waiter_line.load(Ordering::Relaxed)
                );
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
        self.mutex.owner_hart.store(usize::MAX, Ordering::Relaxed);
        self.mutex.owner_file.store(0, Ordering::Relaxed);
        self.mutex.owner_file_len.store(0, Ordering::Relaxed);
        self.mutex.owner_line.store(0, Ordering::Relaxed);
        self.mutex.lock.store(false, Ordering::Release);
        S::after_unlock(&mut self.support_guard);
    }
}

/// Plain spinlock.
pub type SpinLock<T> = SpinMutex<T, Spin>;
/// Spinlock that disables interrupts while held.
pub type SpinNoIrqLock<T> = SpinMutex<T, SpinNoIrq>;
