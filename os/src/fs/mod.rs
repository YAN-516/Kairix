//! File system in os
mod osinode;
mod stdio;
mod disk;
mod ext4fs;
use crate::mm::UserBuffer;
/// File trait
pub trait File: Send + Sync {
    /// If readable
    fn readable(&self) -> bool;
    /// If writable
    fn writable(&self) -> bool;
    /// Read file to `UserBuffer`
    fn read(&self, buf: UserBuffer) -> usize;
    /// Write `UserBuffer` to file
    fn write(&self, buf: UserBuffer) -> usize;
}

pub use osinode::{OSInode, OpenFlags, list_apps, open_file};
pub use stdio::{Stdin, Stdout};
