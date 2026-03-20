//! Synchronization and interior mutability primitives
#[allow(missing_docs)]
pub mod mutex;
mod up;
pub use up::UPSafeCell;
