// use crate::config::PAGE_SIZE;
use crate::error::{SysError, SyscallResult};
use crate::fs::File;
// use crate::fs::open_file;
use crate::error::SysResult;
use crate::fs::vfs::Inode;
use crate::mm::UserBuffer;
use crate::mm::{PageTable, PhysAddr, VirtAddr, VirtPageNum};
use crate::mm::{VMSpace, translated_ref, translated_refmut, translated_str};
use crate::sync::SpinLock;
use crate::task::Tms;
use crate::task::{
    block_current_and_run_next, current_process, current_task, current_user_token,
    exit_current_and_run_next, pid2process, suspend_current_and_run_next, wakeup_task,
    TaskControlBlock,
};
use polyhal::consts::PAGE_SIZE;
// use crate::timer::get_time_us;
use crate::fs::vfs::FileInner;
use crate::trap::_set_sum_bit;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use log::{error, warn};
use spin::*;
pub struct Pipe {
    readable: bool,
    writable: bool,
    buffer: Arc<SpinLock<PipeRingBuffer>>,
}

impl Pipe {
    pub fn read_end_with_buffer(buffer: Arc<SpinLock<PipeRingBuffer>>) -> Self {
        Self {
            readable: true,
            writable: false,
            buffer,
        }
    }
    pub fn write_end_with_buffer(buffer: Arc<SpinLock<PipeRingBuffer>>) -> Self {
        Self {
            readable: false,
            writable: true,
            buffer,
        }
    }
}

const RING_BUFFER_SIZE: usize = 512;

#[derive(Copy, Clone, PartialEq)]
enum RingBufferStatus {
    Full,
    Empty,
    Normal,
}

pub struct PipeRingBuffer {
    arr: [u8; RING_BUFFER_SIZE],
    head: usize,
    tail: usize,
    status: RingBufferStatus,
    write_end: Option<Weak<Pipe>>,
    read_waiters: VecDeque<Arc<TaskControlBlock>>,
    write_waiters: VecDeque<Arc<TaskControlBlock>>,
    poll_waiters: VecDeque<Arc<TaskControlBlock>>,
}

impl PipeRingBuffer {
    pub fn new() -> Self {
        Self {
            arr: [0; RING_BUFFER_SIZE],
            head: 0,
            tail: 0,
            status: RingBufferStatus::Empty,
            write_end: None,
            read_waiters: VecDeque::new(),
            write_waiters: VecDeque::new(),
            poll_waiters: VecDeque::new(),
        }
    }
    pub fn set_write_end(&mut self, write_end: &Arc<Pipe>) {
        self.write_end = Some(Arc::downgrade(write_end));
    }
    pub fn write_byte(&mut self, byte: u8) {
        self.status = RingBufferStatus::Normal;
        self.arr[self.tail] = byte;
        self.tail = (self.tail + 1) % RING_BUFFER_SIZE;
        if self.tail == self.head {
            self.status = RingBufferStatus::Full;
        }
    }
    pub fn read_byte(&mut self) -> u8 {
        self.status = RingBufferStatus::Normal;
        let c = self.arr[self.head];
        self.head = (self.head + 1) % RING_BUFFER_SIZE;
        if self.head == self.tail {
            self.status = RingBufferStatus::Empty;
        }
        c
    }
    pub fn available_read(&self) -> usize {
        if self.status == RingBufferStatus::Empty {
            0
        } else if self.tail > self.head {
            self.tail - self.head
        } else {
            self.tail + RING_BUFFER_SIZE - self.head
        }
    }
    pub fn available_write(&self) -> usize {
        if self.status == RingBufferStatus::Full {
            0
        } else {
            RING_BUFFER_SIZE - self.available_read()
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
    buffer.lock().set_write_end(&write_end);
    (read_end, write_end)
}

impl File for Pipe {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        panic!("[Stdout]: don not support get file_inner")
    }
    fn get_inode(&self) -> Option<Arc<dyn Inode>> {
        None
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
    fn is_pipe(&self) -> bool {
        true
    }
    fn pipe_has_data(&self) -> bool {
        let ring_buffer = self.buffer.lock();
        ring_buffer.available_read() > 0
    }
    fn pipe_has_space(&self) -> bool {
        let ring_buffer = self.buffer.lock();
        ring_buffer.available_write() > 0
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
        let mut buf_iter = buf.into_iter();
        let mut already_read = 0usize;
        loop {
            let mut ring_buffer = self.buffer.lock();
            let loop_read = ring_buffer.available_read();
            if loop_read == 0 {
                if ring_buffer.all_write_ends_closed() {
                    ring_buffer.wake_poll_waiters();
                    return Ok(already_read);
                }
                // 真正阻塞等待数据
                let task = current_task().unwrap();
                ring_buffer.read_waiters.push_back(task);
                drop(ring_buffer);
                block_current_and_run_next();
                continue;
            }
            for _ in 0..loop_read {
                if let Some(byte_ref) = buf_iter.next() {
                    unsafe {
                        *byte_ref = ring_buffer.read_byte();
                    }
                    already_read += 1;
                    if already_read == want_to_read {
                        ring_buffer.wake_write_waiters();
                        ring_buffer.wake_poll_waiters();
                        return Ok(want_to_read);
                    }
                } else {
                    ring_buffer.wake_write_waiters();
                    ring_buffer.wake_poll_waiters();
                    return Ok(already_read);
                }
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
        let mut buf_iter = buf.into_iter();
        let mut already_write = 0usize;
        loop {
            let mut ring_buffer = self.buffer.lock();
            let loop_write = ring_buffer.available_write();
            if loop_write == 0 {
                // 真正阻塞等待空间
                let task = current_task().unwrap();
                ring_buffer.write_waiters.push_back(task);
                drop(ring_buffer);
                block_current_and_run_next();
                continue;
            }
            // write at most loop_write bytes
            for _ in 0..loop_write {
                if let Some(byte_ref) = buf_iter.next() {
                    ring_buffer.write_byte(unsafe { *byte_ref });
                    already_write += 1;
                    if already_write == want_to_write {
                        ring_buffer.wake_read_waiters();
                        ring_buffer.wake_poll_waiters();
                        return Ok(want_to_write);
                    }
                } else {
                    ring_buffer.wake_read_waiters();
                    ring_buffer.wake_poll_waiters();
                    return Ok(already_write);
                }
            }
        }
    }
}

pub fn sys_pipe(pipe: *mut i32) -> SyscallResult {
    _set_sum_bit();
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    let (pipe_read, pipe_write) = make_pipe();

    let read_fd = inner.alloc_fd()?;
    inner.fd_table[read_fd] = Some(pipe_read);
    let write_fd = match inner.alloc_fd() {
        Ok(fd) => fd,
        Err(e) => {
            inner.fd_table[read_fd] = None;
            return Err(e);
        }
    };
    inner.fd_table[write_fd] = Some(pipe_write);
    unsafe {
        *pipe.offset(0) = read_fd as i32;
        *pipe.offset(1) = write_fd as i32;
    }
    Ok(0)
}
