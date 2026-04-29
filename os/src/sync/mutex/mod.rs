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

pub mod remutex;
pub mod sleep_mutex;
pub mod spin_mutex;

pub use remutex::{ReentrantLock, ReentrantMutex, ReentrantMutexGuard};
pub use sleep_mutex::{BlockingMutex, BlockingMutexGuard, SleepLock};
pub use spin_mutex::{SpinLock, SpinMutex, SpinMutexGuard, SpinNoIrqLock};

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
