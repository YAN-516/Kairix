#![allow(missing_docs)]

use crate::fs::FS_MANAGER;
use crate::fs::vfs::fstype::FileSystemFlags;
use alloc::format;
use alloc::string::String;

/// Generate `/proc/filesystems` from the filesystem types currently
/// registered with the VFS. Linux prefixes types which do not require a block
/// device with `nodev`.
pub fn content() -> String {
    let filesystems = FS_MANAGER.lock();
    let mut output = String::new();
    for filesystem in filesystems.values() {
        if filesystem.flags().contains(FileSystemFlags::REQUIRES_DEV) {
            output.push_str(&format!("\t{}\n", filesystem.name()));
        } else {
            output.push_str(&format!("nodev\t{}\n", filesystem.name()));
        }
    }
    output
}
