//! Filesystem-wide constants shared by VFS, syscalls, and notify code.

pub(crate) const FD_CLOEXEC_FLAG: u32 = 1;
pub(crate) const FD_FANOTIFY_EVENT: u32 = 1 << 31;
