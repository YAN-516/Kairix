//! File system in os
mod osinode;
mod stdio;
mod disk;
mod ext4fs;

mod vfs;
mod superblock;
pub use osinode::{OSInode, OpenFlags, list_apps, open_file};
pub use stdio::{Stdin, Stdout};
pub use vfs::file::File;