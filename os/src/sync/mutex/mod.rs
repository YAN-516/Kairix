//! Synchronization primitives: SpinMutex, BlockingMutex, ReentrantMutex.
//!
//! This module provides a unified locking framework based on `MutexSupport`.
//! All locks are `no_std` compatible and support both RISC-V 64 and LoongArch64
//! through polyhal's cross-architecture IRQ API.
//!
//! ## Lock types
//!
//! | Type | Behavior | Use case |
//! |------|----------|----------|
//! | `SpinLock<T>` | Busy-wait | Very short critical sections |
//! | `SpinNoIrqLock<T>` | Busy-wait + disable interrupts | Short critical sections that may be accessed from interrupt context |
//! | `BlockingMutex<T>` | Block current task and yield CPU | Long critical sections (e.g. filesystem, page tables) |
//! | `ReentrantLock<T>` | Reentrant (same task may lock recursively) | Complex call graphs where recursion is possible |
//!
//! ## Legacy types (kept for backward compatibility)
//!
//! `ConsoleMutex`, `MutexSpin`, `MutexBlocking` and the `Mutex` trait are
//! preserved so existing code continues to compile. New code should prefer
//! the types above.

pub mod remutex;
pub mod sleep_mutex;
pub mod spin_mutex;

pub use remutex::{ReentrantLock, ReentrantMutex, ReentrantMutexGuard};
pub use sleep_mutex::{BlockingMutex, BlockingMutexGuard, SleepLock};
pub use spin_mutex::{SpinLock, SpinMutex, SpinMutexGuard, SpinNoIrqLock};

use core::sync::atomic::{AtomicBool, Ordering};

// =============================================================================
// 1. MutexSupport trait & IRQ control
// =============================================================================

/// Low-level support for mutexes (spinlock, sleeplock, etc).
///
/// `before_lock` is called before the lock is acquired;
/// `after_unlock` is called when the guard is dropped.
pub trait MutexSupport {
    /// Guard data passed through the critical section.
    type GuardData;
    /// Called before `lock()` / `try_lock()`.
    fn before_lock() -> Self::GuardData;
    /// Called when `MutexGuard` drops.
    fn after_unlock(_: &mut Self::GuardData);
}

/// No-op support: plain spinlock without side effects.
pub struct Spin;

impl MutexSupport for Spin {
    type GuardData = ();
    #[inline(always)]
    fn before_lock() -> Self::GuardData {}
    #[inline(always)]
    fn after_unlock(_: &mut Self::GuardData) {}
}

/// Disable/restore interrupts around the critical section.
///
/// This prevents deadlocks when an interrupt handler tries to acquire the same
/// lock that the interrupted context currently holds.
pub struct SpinNoIrq;

/// Saves the previous interrupt state and restores it on drop.
pub struct IrqGuard {
    was_enabled: bool,
}

impl IrqGuard {
    #[inline]
    pub fn new() -> Self {
        let was_enabled = polyhal::irq::IRQ::int_enabled();
        polyhal::irq::IRQ::int_disable();
        Self { was_enabled }
    }
}

impl Drop for IrqGuard {
    #[inline]
    fn drop(&mut self) {
        if self.was_enabled {
            polyhal::irq::IRQ::int_enable();
        }
    }
}

impl MutexSupport for SpinNoIrq {
    type GuardData = IrqGuard;
    #[inline(always)]
    fn before_lock() -> Self::GuardData {
        IrqGuard::new()
    }
    #[inline(always)]
    fn after_unlock(_: &mut Self::GuardData) {}
}

// =============================================================================
// 2. Legacy types (kept for backward compatibility)
// =============================================================================

use crate::sync::UPSafeCell;
use crate::task::{TaskControlBlock, block_current_and_run_next, current_task, suspend_current_and_run_next, wakeup_task};
use alloc::{collections::VecDeque, sync::Arc};

/// Legacy trait-based mutex interface.
#[allow(unused)]
#[allow(missing_docs)]
pub trait Mutex: Sync + Send {
    fn lock(&self);
    fn unlock(&self);
}

/// Legacy console mutex (pure busy-wait).
#[allow(missing_docs)]
pub struct ConsoleMutex {
    locked: UPSafeCell<bool>,
}

#[allow(unused)]
#[allow(missing_docs)]
impl ConsoleMutex {
    pub fn new() -> Self {
        Self {
            locked: unsafe { UPSafeCell::new(false) },
        }
    }
}

impl Mutex for ConsoleMutex {
    fn lock(&self) {
        loop {
            let mut locked = self.locked.exclusive_access();
            if *locked {
                drop(locked);
                continue;
            } else {
                *locked = true;
                return;
            }
        }
    }

    fn unlock(&self) {
        let mut locked = self.locked.exclusive_access();
        *locked = false;
    }
}

/// Legacy spin mutex (yields CPU on contention).
#[allow(missing_docs)]
pub struct MutexSpin {
    locked: UPSafeCell<bool>,
}

#[allow(unused)]
#[allow(missing_docs)]
impl MutexSpin {
    pub fn new() -> Self {
        Self {
            locked: unsafe { UPSafeCell::new(false) },
        }
    }
}

impl Mutex for MutexSpin {
    fn lock(&self) {
        loop {
            let mut locked = self.locked.exclusive_access();
            if *locked {
                drop(locked);
                suspend_current_and_run_next();
                continue;
            } else {
                *locked = true;
                return;
            }
        }
    }

    fn unlock(&self) {
        let mut locked = self.locked.exclusive_access();
        *locked = false;
    }
}

/// Legacy blocking mutex.
#[allow(missing_docs)]
pub struct MutexBlocking {
    inner: UPSafeCell<MutexBlockingInner>,
}

#[allow(missing_docs)]
pub struct MutexBlockingInner {
    locked: bool,
    wait_queue: VecDeque<Arc<TaskControlBlock>>,
}

#[allow(missing_docs)]
#[allow(unused)]
impl MutexBlocking {
    pub fn new() -> Self {
        Self {
            inner: unsafe {
                UPSafeCell::new(MutexBlockingInner {
                    locked: false,
                    wait_queue: VecDeque::new(),
                })
            },
        }
    }
}

impl Mutex for MutexBlocking {
    fn lock(&self) {
        let mut mutex_inner = self.inner.exclusive_access();
        if mutex_inner.locked {
            mutex_inner.wait_queue.push_back(current_task().unwrap());
            drop(mutex_inner);
            block_current_and_run_next();
        } else {
            mutex_inner.locked = true;
        }
    }

    fn unlock(&self) {
        let mut mutex_inner = self.inner.exclusive_access();
        assert!(mutex_inner.locked);
        if let Some(waking_task) = mutex_inner.wait_queue.pop_front() {
            wakeup_task(waking_task);
        } else {
            mutex_inner.locked = false;
        }
    }
}
