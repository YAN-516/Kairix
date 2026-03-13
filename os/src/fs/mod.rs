//! File system in os
mod inode;
mod stdio;
use crate::mm::UserBuffer;
use alloc::boxed::Box;
use async_trait::async_trait;
/// File trait
#[async_trait]
#[allow(missing_docs)]
pub trait File: Send + Sync {
    /// If readable
    fn readable(&self) -> bool;
    /// If writable
    fn writable(&self) -> bool;
    /// Read file to `UserBuffer`
    fn read(&self, buf: UserBuffer) -> usize;
    /// Write `UserBuffer` to file
    fn write(&self, buf: UserBuffer) -> usize;

    async fn read_async(&self, buf: UserBuffer) -> usize;
    /// Write `UserBuffer` to file
    async fn write_async(&self, buf: UserBuffer) -> usize;
}

pub use inode::{OSInode, OpenFlags, list_apps, open_file};
pub use stdio::{Stdin, Stdout};
