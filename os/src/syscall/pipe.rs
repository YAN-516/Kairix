// use crate::config::PAGE_SIZE;
use crate::error::{SysError, SyscallResult};
use crate::fs::File;
// use crate::fs::open_file;
use crate::error::SysResult;
use crate::fs::vfs::{Inode, OpenFlags};
use crate::mm::UserBuffer;
use crate::mm::{PageTable, PhysAddr, VirtAddr, VirtPageNum};
use crate::mm::{
    VMSpace, translated_byte_buffer, translated_ref, translated_refmut, translated_str,
};
use crate::sync::SpinLock;
use crate::task::Tms;
use crate::task::{
    TaskControlBlock, block_current_and_run_next, current_process, current_task,
    current_user_token, exit_current_and_run_next, pid2process, suspend_current_and_run_next,
    wakeup_task,
};
use polyhal::consts::PAGE_SIZE;
// use crate::timer::get_time_us;
use crate::fs::vfs::FileInner;
use crate::trap::_set_sum_bit;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec;
use alloc::vec::Vec;
use log::{error, info, warn};
use spin::*;
pub struct Pipe {
    readable: bool,
    writable: bool,
    buffer: Arc<SpinLock<PipeRingBuffer>>,
    status_flags: SpinLock<u32>,
}

impl Pipe {
    pub fn read_end_with_buffer(buffer: Arc<SpinLock<PipeRingBuffer>>) -> Self {
        Self {
            readable: true,
            writable: false,
            buffer,
            status_flags: SpinLock::new(OpenFlags::RDONLY.bits()),
        }
    }
    pub fn write_end_with_buffer(buffer: Arc<SpinLock<PipeRingBuffer>>) -> Self {
        Self {
            readable: false,
            writable: true,
            buffer,
            status_flags: SpinLock::new(OpenFlags::WRONLY.bits()),
        }
    }

    fn nonblock(&self) -> bool {
        *self.status_flags.lock() & OpenFlags::O_NONBLOCK.bits() != 0
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        let mut ring_buffer = self.buffer.lock();
        if self.readable {
            ring_buffer.wake_write_waiters();
        }
        if self.writable {
            ring_buffer.wake_read_waiters();
        }
        ring_buffer.wake_poll_waiters();
    }
}

const DEFAULT_PIPE_CAPACITY: usize = 4096 * 16;
const PIPE_BUF: usize = 4096;
const PIPE_MAX_SIZE: usize = 1024 * 1024;
const PIPE_SIZE_LIMIT: usize = 1usize << 31;

#[derive(Copy, Clone, PartialEq)]
enum RingBufferStatus {
    Full,
    Empty,
    Normal,
}

pub struct PipeRingBuffer {
    arr: Vec<u8>,
    capacity: usize,
    head: usize,
    tail: usize,
    status: RingBufferStatus,
    read_end: Option<Weak<Pipe>>,
    write_end: Option<Weak<Pipe>>,
    read_waiters: VecDeque<Arc<TaskControlBlock>>,
    write_waiters: VecDeque<Arc<TaskControlBlock>>,
    poll_waiters: VecDeque<Arc<TaskControlBlock>>,
}

impl PipeRingBuffer {
    pub fn new() -> Self {
        Self {
            arr: Vec::new(),
            capacity: DEFAULT_PIPE_CAPACITY,
            head: 0,
            tail: 0,
            status: RingBufferStatus::Empty,
            read_end: None,
            write_end: None,
            read_waiters: VecDeque::new(),
            write_waiters: VecDeque::new(),
            poll_waiters: VecDeque::new(),
        }
    }

    pub fn resize(&mut self, new_capacity: usize) -> SyscallResult {
        let data_len = self.available_read();
        if new_capacity < data_len {
            return Err(SysError::EBUSY);
        }
        let mut temp = Vec::with_capacity(data_len);
        for _ in 0..data_len {
            temp.push(self.read_byte());
        }
        self.arr = if data_len == 0 {
            Vec::new()
        } else {
            vec![0; new_capacity]
        };
        self.capacity = new_capacity;
        self.head = 0;
        self.tail = 0;
        self.status = RingBufferStatus::Empty;
        for byte in temp {
            self.write_byte(byte);
        }
        Ok(0)
    }
    pub fn set_read_end(&mut self, read_end: &Arc<Pipe>) {
        self.read_end = Some(Arc::downgrade(read_end));
    }
    pub fn set_write_end(&mut self, write_end: &Arc<Pipe>) {
        self.write_end = Some(Arc::downgrade(write_end));
    }
    pub fn all_read_ends_closed(&self) -> bool {
        self.read_end.as_ref().unwrap().upgrade().is_none()
    }
    pub fn write_byte(&mut self, byte: u8) {
        if self.arr.is_empty() {
            self.arr = vec![0; self.capacity];
        }
        self.status = RingBufferStatus::Normal;
        self.arr[self.tail] = byte;
        self.tail = (self.tail + 1) % self.capacity;
        if self.tail == self.head {
            self.status = RingBufferStatus::Full;
        }
    }
    pub fn read_byte(&mut self) -> u8 {
        self.status = RingBufferStatus::Normal;
        let c = self.arr[self.head];
        self.head = (self.head + 1) % self.capacity;
        if self.head == self.tail {
            self.status = RingBufferStatus::Empty;
        }
        c
    }
    fn contiguous_read_len(&self) -> usize {
        if self.status == RingBufferStatus::Empty {
            0
        } else if self.tail > self.head {
            self.tail - self.head
        } else {
            self.capacity - self.head
        }
    }
    fn contiguous_write_len(&self) -> usize {
        if self.status == RingBufferStatus::Full {
            0
        } else if self.tail >= self.head {
            self.capacity - self.tail
        } else {
            self.head - self.tail
        }
    }
    pub fn read_slice(&mut self, dst: &mut [u8]) -> usize {
        let target = dst.len().min(self.available_read());
        let mut copied = 0usize;
        while copied < target {
            let copy_len = self.contiguous_read_len().min(target - copied);
            if copy_len == 0 {
                break;
            }
            dst[copied..copied + copy_len]
                .copy_from_slice(&self.arr[self.head..self.head + copy_len]);
            self.head = (self.head + copy_len) % self.capacity;
            self.status = if self.head == self.tail {
                RingBufferStatus::Empty
            } else {
                RingBufferStatus::Normal
            };
            copied += copy_len;
        }
        copied
    }
    pub fn write_slice(&mut self, src: &[u8]) -> usize {
        if src.is_empty() {
            return 0;
        }
        if self.arr.is_empty() {
            self.arr = vec![0; self.capacity];
        }
        let target = src.len().min(self.available_write());
        let mut copied = 0usize;
        while copied < target {
            let copy_len = self.contiguous_write_len().min(target - copied);
            if copy_len == 0 {
                break;
            }
            self.arr[self.tail..self.tail + copy_len]
                .copy_from_slice(&src[copied..copied + copy_len]);
            self.tail = (self.tail + copy_len) % self.capacity;
            self.status = if self.tail == self.head {
                RingBufferStatus::Full
            } else {
                RingBufferStatus::Normal
            };
            copied += copy_len;
        }
        copied
    }
    pub fn available_read(&self) -> usize {
        if self.status == RingBufferStatus::Empty {
            0
        } else if self.tail > self.head {
            self.tail - self.head
        } else {
            self.tail + self.capacity - self.head
        }
    }
    pub fn available_write(&self) -> usize {
        if self.status == RingBufferStatus::Full {
            0
        } else {
            self.capacity - self.available_read()
        }
    }
    pub fn all_write_ends_closed(&self) -> bool {
        self.write_end.as_ref().unwrap().upgrade().is_none()
    }

    pub fn wake_read_waiters(&mut self) {
        while let Some(task) = self.read_waiters.pop_front() {
            wakeup_task(task);
        }
    }
    pub fn wake_write_waiters(&mut self) {
        while let Some(task) = self.write_waiters.pop_front() {
            wakeup_task(task);
        }
    }
    pub fn wake_poll_waiters(&mut self) {
        while let Some(task) = self.poll_waiters.pop_front() {
            wakeup_task(task);
        }
    }
    pub fn register_poll_waker(&mut self, task: Arc<TaskControlBlock>) {
        let task_ptr = Arc::as_ptr(&task);
        if !self.poll_waiters.iter().any(|t| Arc::as_ptr(t) == task_ptr) {
            self.poll_waiters.push_back(task);
        }
    }
    pub fn clear_poll_waker(&mut self, task: &Arc<TaskControlBlock>) {
        let task_ptr = Arc::as_ptr(task);
        self.poll_waiters.retain(|t| Arc::as_ptr(t) != task_ptr);
    }
}

/// Return (read_end, write_end)
pub fn make_pipe() -> (Arc<Pipe>, Arc<Pipe>) {
    let buffer = Arc::new(SpinLock::new(PipeRingBuffer::new()));
    let read_end = Arc::new(Pipe::read_end_with_buffer(buffer.clone()));
    let write_end = Arc::new(Pipe::write_end_with_buffer(buffer.clone()));
    buffer.lock().set_read_end(&read_end);
    buffer.lock().set_write_end(&write_end);
    (read_end, write_end)
}

pub struct SocketPairFile {
    read_end: Arc<Pipe>,
    write_end: Arc<Pipe>,
}

impl SocketPairFile {
    fn new(read_end: Arc<Pipe>, write_end: Arc<Pipe>, nonblock: bool) -> Self {
        if nonblock {
            read_end.set_status_flags(OpenFlags::O_NONBLOCK.bits());
            write_end.set_status_flags(OpenFlags::O_NONBLOCK.bits());
        }
        Self {
            read_end,
            write_end,
        }
    }
}

impl File for SocketPairFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        panic!("[SocketPairFile]: don not support get file_inner")
    }

    fn get_inode(&self) -> Option<Arc<dyn Inode>> {
        None
    }

    fn get_stat(&self, stat: &mut crate::fs::vfs::kstat::Kstat) -> SysResult<()> {
        stat.st_ino = Arc::as_ptr(&self.read_end.buffer) as u64;
        stat.st_mode = 0o140000 | 0o777; // S_IFSOCK | rwxrwxrwx
        stat.st_nlink = 1;
        stat.st_uid = 0;
        stat.st_gid = 0;
        stat.st_size = self.read_end.buffer.lock().available_read() as i64;
        stat.st_blksize = 4096;
        stat.st_blocks = 0;
        stat.st_atime_sec = 0;
        stat.st_atime_nsec = 0;
        stat.st_mtime_sec = 0;
        stat.st_mtime_nsec = 0;
        stat.st_ctime_sec = 0;
        stat.st_ctime_nsec = 0;
        Ok(())
    }

    fn get_offset(&self) -> usize {
        0
    }

    fn set_offset(&self, _new_offset: usize) {}

    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        true
    }

    fn is_socket(&self) -> bool {
        true
    }

    fn supports_epoll(&self) -> bool {
        true
    }

    fn status_flags(&self) -> u32 {
        OpenFlags::RDWR.bits() | (self.read_end.status_flags() & OpenFlags::O_NONBLOCK.bits())
    }

    fn set_status_flags(&self, flags: u32) {
        let nonblock = flags & OpenFlags::O_NONBLOCK.bits();
        self.read_end.set_status_flags(nonblock);
        self.write_end.set_status_flags(nonblock);
    }

    fn read(&self, buf: UserBuffer) -> SysResult<usize> {
        self.read_end.read(buf)
    }

    fn write(&self, buf: UserBuffer) -> SysResult<usize> {
        self.write_end.write(buf)
    }

    fn read_ready(&self) -> Option<bool> {
        let ring_buffer = self.read_end.buffer.lock();
        Some(ring_buffer.available_read() > 0 || ring_buffer.all_write_ends_closed())
    }

    fn write_ready(&self) -> Option<bool> {
        let ring_buffer = self.write_end.buffer.lock();
        Some(ring_buffer.available_write() > 0 && !ring_buffer.all_read_ends_closed())
    }

    fn register_poll_waker(&self, task: Arc<crate::task::TaskControlBlock>) {
        self.read_end.register_poll_waker(task.clone());
        self.write_end.register_poll_waker(task);
    }

    fn clear_poll_waker(&self, task: &Arc<crate::task::TaskControlBlock>) {
        self.read_end.clear_poll_waker(task);
        self.write_end.clear_poll_waker(task);
    }

    fn wake_poll_waiters(&self) {
        self.read_end.wake_poll_waiters();
        self.write_end.wake_poll_waiters();
    }

    fn ioctl(&self, request: usize, argp: usize) -> SyscallResult {
        self.read_end.ioctl(request, argp)
    }
}

pub(super) fn make_socket_pair(nonblock: bool) -> (Arc<SocketPairFile>, Arc<SocketPairFile>) {
    let (read0, write1) = make_pipe();
    let (read1, write0) = make_pipe();
    (
        Arc::new(SocketPairFile::new(read0, write0, nonblock)),
        Arc::new(SocketPairFile::new(read1, write1, nonblock)),
    )
}

impl File for Pipe {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        panic!("[Stdout]: don not support get file_inner")
    }
    fn get_inode(&self) -> Option<Arc<dyn Inode>> {
        None
    }
    fn get_stat(&self, stat: &mut crate::fs::vfs::kstat::Kstat) -> SysResult<()> {
        let ring_buffer = self.buffer.lock();
        stat.st_ino = Arc::as_ptr(&self.buffer) as u64;
        stat.st_mode = 0o010000 | 0o600; // S_IFIFO | rw-------
        stat.st_nlink = 1;
        stat.st_uid = 0;
        stat.st_gid = 0;
        stat.st_size = ring_buffer.available_read() as i64;
        stat.st_blksize = 4096;
        stat.st_blocks = 0;
        stat.st_atime_sec = 0;
        stat.st_atime_nsec = 0;
        stat.st_mtime_sec = 0;
        stat.st_mtime_nsec = 0;
        stat.st_ctime_sec = 0;
        stat.st_ctime_nsec = 0;
        Ok(())
    }
    fn get_offset(&self) -> usize {
        0
    }
    fn set_offset(&self, _new_offset: usize) {
        // pipe 不支持 seek，忽略偏移设置。
    }
    fn readable(&self) -> bool {
        self.readable
    }
    fn writable(&self) -> bool {
        self.writable
    }
    fn status_flags(&self) -> u32 {
        *self.status_flags.lock()
    }
    fn set_status_flags(&self, flags: u32) {
        let mut status_flags = self.status_flags.lock();
        let access_mode = *status_flags & 0o3;
        *status_flags = access_mode | (flags & OpenFlags::O_NONBLOCK.bits());
    }
    fn is_pipe(&self) -> bool {
        true
    }
    fn supports_epoll(&self) -> bool {
        true
    }
    fn pipe_capacity(&self) -> Option<usize> {
        Some(self.buffer.lock().capacity)
    }
    fn set_pipe_capacity(&self, capacity: usize) -> SyscallResult {
        if capacity > PIPE_SIZE_LIMIT {
            return Err(SysError::EINVAL);
        }
        if capacity > PIPE_MAX_SIZE {
            return Err(SysError::EPERM);
        }
        let capacity = capacity.max(PIPE_BUF);
        self.buffer.lock().resize(capacity)
    }
    fn pipe_has_data(&self) -> bool {
        let ring_buffer = self.buffer.lock();
        ring_buffer.available_read() > 0
    }
    fn pipe_all_write_ends_closed(&self) -> bool {
        self.buffer.lock().all_write_ends_closed()
    }
    fn pipe_read_len(&self) -> Option<usize> {
        Some(self.buffer.lock().available_read())
    }
    fn pipe_has_space(&self) -> bool {
        let ring_buffer = self.buffer.lock();
        ring_buffer.available_write() > 0
    }
    fn pipe_all_read_ends_closed(&self) -> bool {
        self.buffer.lock().all_read_ends_closed()
    }
    fn register_poll_waker(&self, task: Arc<crate::task::TaskControlBlock>) {
        let mut ring_buffer = self.buffer.lock();
        ring_buffer.register_poll_waker(task);
    }
    fn clear_poll_waker(&self, task: &Arc<crate::task::TaskControlBlock>) {
        let mut ring_buffer = self.buffer.lock();
        ring_buffer.clear_poll_waker(task);
    }
    fn wake_poll_waiters(&self) {
        let mut ring_buffer = self.buffer.lock();
        ring_buffer.wake_poll_waiters();
    }
    fn read(&self, buf: UserBuffer) -> SysResult<usize> {
        assert!(self.readable());
        let want_to_read = buf.len();
        if want_to_read == 0 {
            return Ok(0);
        }
        let mut buffers = buf.buffers;
        let mut current_buffer = 0usize;
        let mut current_offset = 0usize;
        let mut already_read = 0usize;
        loop {
            let mut ring_buffer = self.buffer.lock();
            let loop_read = ring_buffer.available_read();
            if loop_read == 0 {
                if ring_buffer.all_write_ends_closed() {
                    ring_buffer.wake_poll_waiters();
                    return Ok(already_read);
                }
                if self.nonblock() {
                    return Err(SysError::EAGAIN);
                }
                // 真正阻塞等待数据
                let task = current_task().unwrap();
                ring_buffer.read_waiters.push_back(task);
                drop(ring_buffer);
                block_current_and_run_next();
                // 被唤醒后检查是否被强制终止或被信号中断（Linux 标准行为）
                if crate::task::current_process()
                    .inner_exclusive_access()
                    .is_zombie
                    || crate::syscall::signal::should_interrupt_syscall()
                {
                    return Err(SysError::EINTR);
                }
                continue;
            }
            let mut round_read = 0usize;
            while round_read < loop_read
                && already_read < want_to_read
                && current_buffer < buffers.len()
            {
                if current_offset == buffers[current_buffer].len() {
                    current_buffer += 1;
                    current_offset = 0;
                    continue;
                }
                let read_len = {
                    let dst = &mut buffers[current_buffer][current_offset..];
                    ring_buffer.read_slice(dst)
                };
                if read_len == 0 {
                    break;
                }
                round_read += read_len;
                already_read += read_len;
                current_offset += read_len;
                if current_offset == buffers[current_buffer].len() {
                    current_buffer += 1;
                    current_offset = 0;
                }
            }
            if already_read == want_to_read {
                ring_buffer.wake_write_waiters();
                ring_buffer.wake_poll_waiters();
                return Ok(want_to_read);
            }
            // 管道中当前可读数据已读完，但已经读取了部分数据：立即返回（短读）
            if already_read > 0 {
                ring_buffer.wake_write_waiters();
                ring_buffer.wake_poll_waiters();
                return Ok(already_read);
            }
        }
    }
    fn write(&self, buf: UserBuffer) -> SysResult<usize> {
        assert!(self.writable());
        let want_to_write = buf.len();
        if want_to_write == 0 {
            return Ok(0);
        }
        let buffers = buf.buffers;
        let mut current_buffer = 0usize;
        let mut current_offset = 0usize;
        let mut already_write = 0usize;
        loop {
            let mut ring_buffer = self.buffer.lock();
            if ring_buffer.all_read_ends_closed() {
                drop(ring_buffer);
                crate::syscall::signal::deliver_signal(
                    &current_process(),
                    crate::task::signal::Signal::SigPipe,
                );
                return Err(SysError::EPIPE);
            }
            let loop_write = ring_buffer.available_write();
            if loop_write == 0 {
                if self.nonblock() {
                    return if already_write > 0 {
                        Ok(already_write)
                    } else {
                        Err(SysError::EAGAIN)
                    };
                }
                // 真正阻塞等待空间
                let task = current_task().unwrap();
                ring_buffer.write_waiters.push_back(task);
                drop(ring_buffer);
                block_current_and_run_next();
                // 被唤醒后检查是否被强制终止或被信号中断（Linux 标准行为）
                if crate::task::current_process()
                    .inner_exclusive_access()
                    .is_zombie
                    || crate::syscall::signal::should_interrupt_syscall()
                {
                    return Err(SysError::EINTR);
                }
                continue;
            }
            let mut round_write = 0usize;
            while round_write < loop_write
                && already_write < want_to_write
                && current_buffer < buffers.len()
            {
                if current_offset == buffers[current_buffer].len() {
                    current_buffer += 1;
                    current_offset = 0;
                    continue;
                }
                let write_len = {
                    let src = &buffers[current_buffer][current_offset..];
                    ring_buffer.write_slice(src)
                };
                if write_len == 0 {
                    break;
                }
                round_write += write_len;
                already_write += write_len;
                current_offset += write_len;
                if current_offset == buffers[current_buffer].len() {
                    current_buffer += 1;
                    current_offset = 0;
                }
            }
            if already_write == want_to_write {
                ring_buffer.wake_read_waiters();
                ring_buffer.wake_poll_waiters();
                return Ok(want_to_write);
            }
            // 已经写入了一批数据但还没写完，唤醒等待的 reader 来消费数据，
            // 否则 writer 和 reader 可能互相阻塞形成死锁。
            ring_buffer.wake_read_waiters();
            ring_buffer.wake_poll_waiters();
        }
    }
    fn ioctl(&self, request: usize, argp: usize) -> SyscallResult {
        const FIONREAD: usize = 0x541B;

        match request {
            FIONREAD => {
                if argp == 0 {
                    return Err(SysError::EFAULT);
                }
                let token = current_user_token();
                *translated_refmut(token, argp as *mut i32)? =
                    self.buffer.lock().available_read() as i32;
                Ok(0)
            }
            _ => Err(SysError::ENOTTY),
        }
    }
}

pub fn sys_pipe(pipe: *mut i32, flags: u32) -> SyscallResult {
    let valid_flags = OpenFlags::O_CLOEXEC.bits() | OpenFlags::O_NONBLOCK.bits();
    if flags & !valid_flags != 0 {
        return Err(SysError::EINVAL);
    }

    _set_sum_bit();
    let token = current_user_token();
    let mut user_bufs =
        translated_byte_buffer(token, pipe as *const u8, 2 * core::mem::size_of::<i32>())?;

    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    let (pipe_read, pipe_write) = make_pipe();
    if flags & OpenFlags::O_NONBLOCK.bits() != 0 {
        pipe_read.set_status_flags(OpenFlags::O_NONBLOCK.bits());
        pipe_write.set_status_flags(OpenFlags::O_NONBLOCK.bits());
    }

    let read_fd = inner.alloc_fd()?;
    if flags & OpenFlags::O_CLOEXEC.bits() != 0 && read_fd < inner.fd_flags.len() {
        inner.fd_flags[read_fd] |= crate::syscall::fs::FD_CLOEXEC_FLAG;
    }
    inner.fd_table[read_fd] = Some(pipe_read);
    let write_fd = match inner.alloc_fd() {
        Ok(fd) => fd,
        Err(e) => {
            inner.fd_table[read_fd] = None;
            if read_fd < inner.fd_flags.len() {
                inner.fd_flags[read_fd] = 0;
            }
            return Err(e);
        }
    };
    if flags & OpenFlags::O_CLOEXEC.bits() != 0 && write_fd < inner.fd_flags.len() {
        inner.fd_flags[write_fd] |= crate::syscall::fs::FD_CLOEXEC_FLAG;
    }
    inner.fd_table[write_fd] = Some(pipe_write);
    drop(inner);

    let fds = [read_fd as i32, write_fd as i32];
    let bytes = unsafe {
        core::slice::from_raw_parts(fds.as_ptr() as *const u8, 2 * core::mem::size_of::<i32>())
    };
    let mut copied = 0usize;
    for buf in user_bufs.iter_mut() {
        let n = buf.len().min(bytes.len() - copied);
        buf[..n].copy_from_slice(&bytes[copied..copied + n]);
        copied += n;
        if copied == bytes.len() {
            break;
        }
    }
    Ok(0)
}
