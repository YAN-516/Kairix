#![allow(missing_docs)]
use crate::alloc::string::ToString;
use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::get_filesystem;
use crate::fs::page::pagecache::Page;
use crate::fs::page::pagecache::PAGE_CACHE;
use crate::fs::vfs::inode::InodeMode;
use crate::fs::vfs::kstat::Kstat;
use crate::fs::vfs::path::split_parent_and_name;
use crate::fs::vfs::path::{resolve_path, resolve_path_nofollow_last};
use crate::fs::vfs::Dentry;
use crate::fs::vfs::OpenFlags;
use crate::fs::Inode;
use crate::fs::GLOBAL_DCACHE;
use crate::mm::UserBuffer;
use crate::mm::{translated_ref, translated_refmut};
use crate::task::current_user_token;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use polyhal::common::FrameTracker;
use spin::rwlock::RwLock;
use spin::MutexGuard;
#[allow(unused)]
pub struct FileInner {
    pub offset: usize,
    pub dentry: Arc<dyn Dentry>,
    pub flags: OpenFlags,
}

pub const FS_IOC_GETFLAGS: usize = 0x8008_6601;
pub const FS_IOC_SETFLAGS: usize = 0x4008_6602;

pub fn ioctl_get_fs_flags(inode: Arc<dyn Inode>, argp: usize) -> SyscallResult {
    if argp == 0 {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    *translated_refmut(token, argp as *mut i32)? = inode.get_fs_flags() as i32;
    Ok(0)
}

pub fn ioctl_set_fs_flags(inode: Arc<dyn Inode>, argp: usize) -> SyscallResult {
    if argp == 0 {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let flags = *translated_ref(token, argp as *const i32)? as u32;
    inode.set_fs_flags(flags);
    Ok(0)
}

/// File trait
pub trait File: Send + Sync {
    ///Get the FileInner
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner>;
    /// If readable
    fn readable(&self) -> bool;
    /// If writable
    fn writable(&self) -> bool;
    /// Read file to `UserBuffer`
    fn read(&self, buf: UserBuffer) -> SysResult<usize>;
    /// Write `UserBuffer` to file
    fn write(&self, buf: UserBuffer) -> SysResult<usize>;
    ///get inode from the Dentry of FileInner
    fn get_inode(&self) -> Option<Arc<dyn Inode>> {
        self.get_fileinner().dentry.get_inode()
    }
    fn cache_inode_id(&self) -> Option<usize> {
        self.get_inode().and_then(|inode| inode.cache_inode_id())
    }
    /// Do something when the node is opened.
    fn open(&self) -> SyscallResult {
        Ok(0)
    }
    /// Do something when the node is closed.
    fn release(&self) -> SyscallResult {
        Ok(0)
    }
    #[allow(unused)]
    ///chaneg the offset of file
    ///
    fn seek(&self, new_offset: usize) -> SysResult<usize> {
        unimplemented!()
    }
    fn ls(&self) -> Vec<(String, u64, u8)> {
        alloc::vec::Vec::new()
    }
    /// Whether this file is a socket
    fn is_socket(&self) -> bool {
        false
    }
    /// Whether this file is opened with O_APPEND
    fn is_append(&self) -> bool {
        false
    }
    /// File status flags returned by fcntl(F_GETFL).
    fn status_flags(&self) -> u32 {
        let flags = self.get_fileinner().flags.bits();
        let status_flags = OpenFlags::O_APPEND | OpenFlags::O_NONBLOCK | OpenFlags::O_NOATIME;
        (flags & 0o3) | (flags & status_flags.bits())
    }
    /// Update mutable file status flags through fcntl(F_SETFL).
    fn set_status_flags(&self, flags: u32) {
        let mut inner = self.get_fileinner();
        let access_mode = inner.flags.bits() & 0o3;
        let settable = OpenFlags::O_APPEND | OpenFlags::O_NONBLOCK | OpenFlags::O_NOATIME;
        inner.flags = OpenFlags::from_bits_truncate(access_mode | (flags & settable.bits()));
    }
    /// Whether this file is a pipe
    fn is_pipe(&self) -> bool {
        false
    }
    /// Whether this file is a pidfd
    fn is_pidfd(&self) -> bool {
        false
    }
    /// Whether this file is an epoll instance.
    fn is_epoll(&self) -> bool {
        false
    }
    /// Whether this file can be registered in an epoll interest set.
    fn supports_epoll(&self) -> bool {
        false
    }
    /// Snapshot epoll metadata used to reject self-registration and cycles.
    fn epoll_id(&self) -> Option<usize> {
        None
    }
    /// Whether this epoll instance already watches another epoll instance.
    fn epoll_watches_epoll(&self) -> bool {
        false
    }
    /// Longest epoll nesting depth reachable from this file.
    fn epoll_nesting_depth(&self) -> usize {
        0
    }
    /// Whether this epoll graph contains the supplied epoll instance id.
    fn epoll_contains_id(&self, _id: usize) -> bool {
        false
    }
    /// Add an interest to an epoll instance.
    fn epoll_add(
        &self,
        _fd: i32,
        _file: Arc<dyn File + Send + Sync>,
        _events: u32,
        _data: u64,
    ) -> SyscallResult {
        Err(SysError::EINVAL)
    }
    /// Modify an interest in an epoll instance.
    fn epoll_modify(&self, _fd: i32, _events: u32, _data: u64) -> SyscallResult {
        Err(SysError::EINVAL)
    }
    /// Delete an interest from an epoll instance.
    fn epoll_delete(&self, _fd: i32) -> SyscallResult {
        Err(SysError::EINVAL)
    }
    /// Return ready epoll events as `(events, data)` pairs.
    fn epoll_ready_events(&self, _maxevents: usize) -> Vec<(u32, u64)> {
        Vec::new()
    }
    /// Register the supplied task on every watched file.
    fn epoll_register_interest_wakers(&self, _task: Arc<crate::task::TaskControlBlock>) {}
    /// Clear the supplied task from every watched file.
    fn epoll_clear_interest_wakers(&self, _task: &Arc<crate::task::TaskControlBlock>) {}
    /// Whether this file is a Landlock ruleset fd.
    fn is_landlock_ruleset(&self) -> bool {
        false
    }
    /// Snapshot the Landlock ruleset carried by this fd.
    fn landlock_ruleset(&self) -> Option<Arc<crate::syscall::landlock::LandlockRuleset>> {
        None
    }
    /// Mutate the Landlock ruleset carried by this fd.
    fn with_landlock_ruleset_mut(
        &self,
        _f: &mut dyn FnMut(&mut crate::syscall::landlock::LandlockRuleset) -> SyscallResult,
    ) -> SyscallResult {
        Err(SysError::EBADFD)
    }
    /// Get the pid associated with this pidfd
    fn pidfd_pid(&self) -> Option<usize> {
        None
    }
    /// For pipe poll: whether pipe has data to read
    fn pipe_has_data(&self) -> bool {
        false
    }
    /// For pipe: bytes currently available to read
    fn pipe_read_len(&self) -> Option<usize> {
        None
    }
    /// For pipe poll: whether pipe has space to write
    fn pipe_has_space(&self) -> bool {
        false
    }
    /// Optional readiness override for special files such as inotify.
    fn read_ready(&self) -> Option<bool> {
        None
    }
    /// Register a task waker for poll/select
    fn register_poll_waker(&self, _task: Arc<crate::task::TaskControlBlock>) {}
    /// Clear a task waker for poll/select
    fn clear_poll_waker(&self, _task: &Arc<crate::task::TaskControlBlock>) {}
    /// Wake all poll/select waiters
    fn wake_poll_waiters(&self) {}
    /// For pipe: get pipe capacity
    fn pipe_capacity(&self) -> Option<usize> {
        None
    }
    /// For pipe: set pipe capacity
    fn set_pipe_capacity(&self, _capacity: usize) -> SyscallResult {
        Err(SysError::EINVAL)
    }
    fn get_offset(&self) -> usize {
        self.get_fileinner().offset
    }
    fn set_offset(&self, new_offset: usize) {
        self.get_fileinner().offset = new_offset;
    }
    fn get_dentry(&self) -> Arc<dyn Dentry> {
        self.get_fileinner().dentry.clone()
    }
    fn get_stat(&self, stat: &mut Kstat) -> SysResult<()> {
        let inode = self.get_inode().ok_or(SysError::EIO)?;
        stat.st_ino = inode.get_ino() as u64;
        stat.st_nlink = inode.get_nlink() as u32;
        stat.st_size = inode.get_size() as i64;
        stat.st_mode = inode.get_mode().bits();
        stat.st_blksize = 512;
        stat.st_blocks = ((stat.st_size as u64 + 511) / 512)
            .saturating_sub(inode.get_punched_hole_pages() as u64 * 8);
        stat.st_rdev = inode.get_rdev() as u64;
        let (atime_sec, atime_nsec) = inode.get_atime();
        let (mtime_sec, mtime_nsec) = inode.get_mtime();
        let (ctime_sec, ctime_nsec) = inode.get_ctime();
        stat.st_atime_sec = atime_sec;
        stat.st_atime_nsec = atime_nsec;
        stat.st_mtime_sec = mtime_sec;
        stat.st_mtime_nsec = mtime_nsec;
        stat.st_ctime_sec = ctime_sec;
        stat.st_ctime_nsec = ctime_nsec;
        Ok(())
    }
    /// 把内存里的脏页刷入底层存储
    fn flush(&self) {}

    /// 专门为 mmap / sendfile 提供：获取文件指定页的物理帧（Miss时自动读盘）
    fn get_cache_frame(&self, _page_id: usize) -> Option<Arc<FrameTracker>> {
        None
    }

    fn read_all(&self) -> Vec<u8> {
        todo!()
    }
    /// ioctl
    fn ioctl(&self, _request: usize, _argp: usize) -> SyscallResult {
        Err(SysError::ENOTTY)
    }
    /// Truncate the file to the given size.
    fn truncate(&self, _size: u64) -> SyscallResult {
        Err(SysError::ENOSYS)
    }
}

impl dyn File {
    // /// 获取指定的缓存页，如果 Miss 则自动从磁盘加载并放入缓存
    // fn get_or_load_cache_page(&self, ino: usize, page_id: usize, old_size: usize) -> Arc<RwLock<Page>> {
    //     {
    //         let cache = PAGE_CACHE.lock();
    //         if let Some(page) = cache.get_page(ino, page_id) {
    //             return page;
    //         }
    //     }
    //     let mut cache_writer = PAGE_CACHE.lock();
    //     if let Some(page) = cache_writer.get_page(ino, page_id) {
    //         return page;
    //     }
    //     let new_page = self.load_page_from_disk(page_id, old_size);
    //     cache_writer.insert_page(ino, page_id, new_page.clone());
    //     new_page
    // }
}

#[allow(unused)]
/// find the dentry by the absolute path, if can not find, return Err(SysError::ENOENT)
/// find from the root dentry, and fill the dcache when find the dentry
pub fn find_dentry(path: &str) -> SysResult<Arc<dyn Dentry>> {
    if let Some(cached) = GLOBAL_DCACHE.get(path) {
        // 校验缓存 dentry 的路径是否仍然有效（防止 parent 被 LRU 淘汰后 path() 失真）
        if cached.path() == path {
            return Ok(cached);
        }
    }
    let rootfs = get_filesystem("ext4");
    let root_dentry = rootfs.get_sb("/").unwrap().root();
    if path == "/" || path.is_empty() {
        GLOBAL_DCACHE.insert("/".to_string(), root_dentry.clone());
        return Ok(root_dentry);
    }

    let mut current_dentry = root_dentry;
    let mut current_path = String::new();
    for part in path.split('/').filter(|s| !s.is_empty()) {
        current_path.push('/');
        current_path.push_str(part);
        if let Some(cached_parent) = GLOBAL_DCACHE.get(&current_path) {
            current_dentry = cached_parent;
            continue;
        }
        if let Ok(next_dentry) = current_dentry.find(part) {
            GLOBAL_DCACHE.insert(current_path.clone(), next_dentry.clone());
            current_dentry = next_dentry;
        } else {
            return Err(SysError::ENOENT);
        }
    }
    Ok(current_dentry)
}

#[allow(unused)]
/// path will be resolved to an absolute path, flags is the open flags
pub fn open_file(
    start_dentry: Arc<dyn Dentry>,
    path: &str,
    flags: OpenFlags,
    mode: InodeMode,
) -> SysResult<Arc<dyn File>> {
    let (readable, writable) = flags.read_write();
    let target_dentry = if flags.contains(OpenFlags::O_CREAT) {
        let (parent_path, name) = split_parent_and_name(path);
        let parent = resolve_path(start_dentry, parent_path.as_str())?;
        match parent.find(name.as_str()) {
            Ok(d) => {
                if flags.contains(OpenFlags::O_NOFOLLOW) {
                    if let Some(inode) = d.get_inode() {
                        if inode.get_mode().contains(InodeMode::LINK) {
                            return Err(SysError::ELOOP);
                        }
                    }
                }
                d
            }
            Err(_) => parent.create(name.as_str(), mode)?,
        }
    } else if flags.contains(OpenFlags::O_NOFOLLOW) {
        resolve_path_nofollow_last(start_dentry, path)?
    } else {
        resolve_path(start_dentry, path)?
    };
    let inode = target_dentry.get_inode().ok_or(SysError::EIO)?;
    if flags.contains(OpenFlags::O_TRUNC) {
        match inode.truncate(0) {
            Ok(_) => {}
            Err(SysError::ENOSYS) => {}
            Err(e) => return Err(e),
        }
    }
    let is_append = flags.contains(OpenFlags::O_APPEND);
    let file = target_dentry.open(flags, inode.get_mode())?;
    if is_append {
        file.set_offset(inode.get_size());
    }
    Ok(file)
}
