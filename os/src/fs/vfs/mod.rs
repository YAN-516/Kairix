#![allow(missing_docs)]
pub mod inode;    
pub mod file;
pub mod superblock;
pub mod dentry;
pub mod dcache;
pub mod path;
pub mod kstat;
pub use superblock::SuperBlock;
pub use inode::Inode;

//dentry部分
pub use dentry::{DentryInner,Dentry,DentryState};

//file部分
pub use file::{FileInner,File};


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
        const O_TRUNC       = 0o1000;
        const O_DIRECTORY   = 0o200000;
    }
}
