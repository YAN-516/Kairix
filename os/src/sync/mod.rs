//! Synchronization and interior mutability primitives
#[allow(missing_docs)]
pub mod mutex;

// Re-export the most commonly used new-style locks for convenience.
pub use mutex::{
    BlockingMutex, BlockingMutexGuard,
    IrqGuard,
    ReentrantLock, ReentrantMutex, ReentrantMutexGuard,
    SleepLock,
    Spin, SpinLock, SpinMutex, SpinMutexGuard,
    SpinNoIrq, SpinNoIrqLock,
};
