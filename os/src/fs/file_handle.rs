//! Linux file-handle encoding helpers.

pub(crate) const FILE_HANDLE_BYTES: u32 = 8;
pub(crate) const FILE_HANDLE_TYPE_INO: i32 = 1;

/// Userspace header for `name_to_handle_at`.
#[repr(C)]
pub struct FileHandleHeader {
    /// Size of the opaque handle payload in bytes.
    pub handle_bytes: u32,
    /// Kernel-defined handle encoding type.
    pub handle_type: i32,
}

/// Encode an inode number into the kernel's simple file-handle format.
pub fn encode_file_handle(ino: u64) -> [u8; FILE_HANDLE_BYTES as usize] {
    ino.to_ne_bytes()
}
