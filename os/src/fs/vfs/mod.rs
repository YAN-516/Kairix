#![allow(missing_docs)]
pub mod dcache;
pub mod dentry;
pub mod file;
pub mod fstype;
pub mod inode;
pub mod kstat;
pub mod path;
pub use superblock::SuperBlock;
pub mod superblock;
pub use inode::Inode;

//dentry部分
pub use dentry::{Dentry, DentryInner, DentryState};

//file部分
pub use file::{File, FileInner};

bitflags! {
    ///Open file flags
    pub struct OpenFlags: u32 {
        ///Read only
        const RDONLY = 0;
        ///Write only
        const WRONLY = 1;
        ///Read & Write
        const RDWR = 2;

        ///Allow create
        const O_CREAT       = 0o100;
        const O_EXCL        = 0o200;
        const O_TRUNC       = 0o1000;
        const O_APPEND      = 0o2000;
        const O_NONBLOCK    = 0o4000;
        const O_DIRECTORY   = 0o200000;
        const O_NOFOLLOW    = 0o400000;
        const O_NOATIME     = 0o1000000;
        const O_CLOEXEC     = 0o2000000;
        const O_PATH        = 0o10000000;
        const O_TMPFILE     = 0o20200000;
    }
}

impl OpenFlags {
    pub fn writable(&self) -> bool {
        self.bits() & 0o3 != 0
    }

    pub fn read_write(&self) -> (bool, bool) {
        match self.bits() & 0o3 {
            0o1 => (false, true),
            0o2 => (true, true),
            _ => (true, false),
        }
    }
}
