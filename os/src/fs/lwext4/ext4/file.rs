use lwext4_rust::bindings::ext4_file;
use core::ffi::CStr;
use lwext4_rust::bindings::{ext4_dir_mv,ext4_fopen,ext4_fclose,ext4_dir_mk,ext4_flink};
use log::*;
use core::mem::MaybeUninit;
///
pub struct ExtFS(pub ext4_file);

impl Drop for ExtFS {
    fn drop(&mut self) {
        unsafe {
            ext4_fclose(&mut self.0);
        }
    }
}

impl ExtFS{
    #[allow(unused)]
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

    ///create the hard link
    pub fn link(path:&CStr,hardlink_path:&CStr)->Result<(),i32>{
        let err = unsafe {ext4_flink(path.as_ptr(), hardlink_path.as_ptr())};
        match err{
            0 => Ok(()),
            _ => {
                warn!(
                    "ext4_flink failed: path = {}, hardlink_path = {}, error = {}",
                    path.to_str().unwrap_or("unknown"),
                    hardlink_path.to_str().unwrap_or("unknown"),
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
    
}
 