///借用了NighthawkOS的思路，封装了lwext4_rust的目录操作接口
use alloc::string::String;
use core::{
    ffi::CStr,
    mem::MaybeUninit,
};
use log::*;
use lwext4_rust::{
    InodeTypes,
    bindings::{
        ext4_dir, ext4_dir_close, ext4_dir_entry_next, ext4_dir_entry_rewind, ext4_dir_mk,
        ext4_dir_mv, ext4_dir_open, ext4_dir_rm, ext4_direntry,ext4_fopen,ext4_fclose
    },
};

/// Wrapper for `lwext4_rust` crate's `ext4_dir` struct which represents a directory
/// file which can reads and writes directory entries.
pub struct ExtDir(pub ext4_dir);

/// Wrapper for `lwext4_rust` crate's `ext4_direntry` struct which represents a directory
/// entry.
pub struct ExtDirEntry<'a>(&'a ext4_direntry);

impl Drop for ExtDir {
    fn drop(&mut self) {
        unsafe {
            ext4_dir_close(&mut self.0);
        }
    }
}

impl ExtDir {
    /// Opens a directory file at the given path and returns a handle to it.
    ///
    /// `path` is the absolute path to the file to be opened.
    pub fn open(path: &CStr) -> Result<Self, i32> {
        let mut dir = MaybeUninit::uninit();
        let err = unsafe { ext4_dir_open(dir.as_mut_ptr(), path.as_ptr()) };
        match err {
            0 => unsafe { Ok(Self(dir.assume_init())) },
            _ => {
                warn!(
                    "ext4_dir_open failed: path = {}, error = {}",
                    path.to_str().unwrap_or("unknown"),
                    err
                );
                Err(err)
            }
        }
    }

    /// Creates a directory at the given path.
    pub fn create(path: &CStr) -> Result<(), i32> {
        let err = unsafe { ext4_dir_mk(path.as_ptr()) };
        match err {
            0 => Ok(()),
            _ => {
                warn!(
                    "ext4_dir_mk failed: path = {}, error = {}",
                    path.to_str().unwrap_or("unknown"),
                    err
                );
                Err(err)
            }
        }
    }
    ///
    // create a file at the given path, the path should be absolute path
    pub fn create_file(path: &CStr) -> Result<(), i32> {
        let mut file_struct = MaybeUninit::uninit();
        let c_mode = core::ffi::CStr::from_bytes_with_nul(b"wb\0").unwrap();
        let err = unsafe { 
            ext4_fopen(file_struct.as_mut_ptr(), path.as_ptr(), c_mode.as_ptr()) 
        };       
        match err {
            0 => unsafe {
                ext4_fclose(file_struct.as_mut_ptr());
                Ok(())
            },
            _ => {
                warn!("ext4_fopen (create file) failed: error = {}", err);
                Err(err)
            }
        }
    }
    
    /// Returns a shared reference to the next directory entry in the directory.
    /// Returns `None` if there are no more entries.
    pub fn next(&mut self) -> Option<ExtDirEntry> {
        unsafe { ext4_dir_entry_next(&mut self.0).as_ref() }.map(ExtDirEntry)
    }
    #[allow(unused)]
    /// Rewinds the directory entry offset to the beginning of the directory file.
    pub fn rewind(&mut self) {
        unsafe {
            ext4_dir_entry_rewind(&mut self.0);
        }
    }
    #[allow(unused)]
    /// Recursively removes a directory and all its contents.
    pub fn remove_recur(path: &CStr) -> Result<(), i32> {
        let err = unsafe { ext4_dir_rm(path.as_ptr()) };
        match err {
            0 => Ok(()),
            _ => {
                warn!(
                    "ext4_dir_rm failed: path = {}, error = {}",
                    path.to_str().unwrap_or("unknown"),
                    err
                );
                Err(err)
            }
        }
    }
    #[allow(unused)]
    /// Change the name or location of a directory.
    pub fn rename(path: &CStr, new_path: &CStr) -> Result<(), i32> {
        let err = unsafe { ext4_dir_mv(path.as_ptr(), new_path.as_ptr()) };
        match err {
            0 => Ok(()),
            _ => {
                warn!(
                    "ext4_dir_mv failed: old_path = {}, new_path = {}, error = {}",
                    path.to_str().unwrap_or("unknown"),
                    new_path.to_str().unwrap_or("unknown"),
                    err
                );
                Err(err)
            }
        }
    }
}

impl ExtDirEntry<'_> {
    /// Returns the inode number of the directory entry.
    pub fn ino(&self) -> u32 {
        self.0.inode
    }
    ///
    pub fn file_type(&self) -> InodeTypes {
        InodeTypes::from(self.0.inode_type as usize)
    }

    /// Returns the name of the directory entry.
    pub fn name(&self) -> Result<String, ()> {
        let name_bytes = self.0.name[..self.0.name_length as usize].to_vec();
        String::from_utf8(name_bytes).map_err(|_| ())
    }
}