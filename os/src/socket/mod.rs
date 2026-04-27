use crate::task::*;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::{Mutex, MutexGuard};
pub mod raw;
#[allow(missing_docs)]
pub mod tcp;
pub mod udp;
use crate::error::SysResult;
use crate::fs::File;
use crate::fs::vfs::FileInner;
use crate::fs::vfs::inode::Inode;
use crate::mm::UserBuffer;
use lazy_static::lazy_static;
use raw::RawSocket;
use raw::unregister_raw_socket;
use tcp::TcpSocket;
use udp::UdpSocket;
use udp::unregister_udp_socket;
lazy_static! {
    pub static ref SOCKET_MANAGER: Mutex<SocketManager> = Mutex::new(SocketManager::new());
}
#[allow(unused)]
/// 套接字类型
pub enum SocketInner {
    Raw(Arc<Mutex<RawSocket>>),
    Udp(Arc<Mutex<UdpSocket>>),
    Tcp(Arc<Mutex<TcpSocket>>),
}

#[allow(unused)]
/// 套接字状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketState {
    Open,
    Bound,
    Closed,
}

#[allow(unused)]
/// 套接字
pub struct Socket {
    pub inner: SocketInner,
    pub fd: usize,
    pub pid: usize,
    pub state: SocketState,
    pub closed: AtomicBool,
}

#[allow(unused)]
impl Socket {
    /// 创建新的套接字
    pub fn new(inner: SocketInner, fd: usize, pid: usize) -> Self {
        Self {
            inner,
            fd,
            pid,
            state: SocketState::Open,
            closed: AtomicBool::new(false),
        }
    }

    /// 关闭套接字
    pub fn close(&mut self) -> Result<(), &'static str> {
        if self.closed.load(Ordering::Acquire) {
            return Err("Socket already closed");
        }

        self.closed.store(true, Ordering::Release);
        self.state = SocketState::Closed;

        // 清理接收队列等资源
        match &mut self.inner {
            SocketInner::Udp(udp_socket) => {
                let mut udp = udp_socket.lock();
                if let Some((_, port)) = udp.local_addr() {
                    unregister_udp_socket(port, udp_socket.clone());
                }
                udp.clear_queue();
            }
            SocketInner::Raw(raw_socket) => {
                let protocol = raw_socket.lock().protocol();
                unregister_raw_socket(protocol, raw_socket.clone());
                raw_socket.lock().clear_queue();
            }
            SocketInner::Tcp(tcp_socket) => {
                let _ = tcp_socket.lock().close();
            }
        }

        Ok(())
    }

    /// 检查是否已关闭
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}

#[allow(unused)]
/// 套接字管理器 - 每个进程独立拥有
pub struct SocketManager {
    pub sockets: Vec<Socket>,
}

#[allow(unused)]
impl SocketManager {
    pub fn new() -> Self {
        Self {
            sockets: Vec::new(),
        }
    }

    /// 添加套接字（fd 由调用者提供，来自进程的 fd_allocator）
    pub fn add_socket(
        &mut self,
        fd: usize,
        mut socket: Socket,
        pid: usize,
    ) -> Result<usize, &'static str> {
        socket.fd = fd;
        socket.pid = pid;

        if self
            .sockets
            .iter_mut()
            .find(|x| x.pid == pid && x.fd == fd)
            .is_none()
        {
            self.sockets.push(socket);
        }

        log::debug!("SocketManager: added socket fd={}", fd);
        Ok(fd)
    }

    /// 获取套接字（不可变引用）
    pub fn get_socket(&self, fd: usize, pid: usize) -> Option<&Socket> {
        if let Some(socket) = self.sockets.iter().find(|x| x.pid == pid && x.fd == fd) {
            Some(socket)
        } else {
            None
        }
    }

    /// 获取套接字（可变引用）
    pub fn get_socket_mut(&mut self, fd: usize, pid: usize) -> Option<&mut Socket> {
        if let Some(socket) = self.sockets.iter_mut().find(|x| x.pid == pid && x.fd == fd) {
            Some(socket)
        } else {
            None
        }
    }

    /// 移除套接字
    pub fn remove_socket(&mut self, fd: usize, pid: usize) -> Option<Socket> {
        let pos = self.sockets.iter().position(|x| x.pid == pid && x.fd == fd);
        pos.map(|p| self.sockets.remove(p))
    }

    /// 关闭并移除套接字
    pub fn close_socket(&mut self, fd: usize, pid: usize) -> Result<(), &'static str> {
        if let Some(mut socket) = self.remove_socket(fd, pid) {
            socket.close()?;
            Ok(())
        } else {
            Err("Socket not found")
        }
    }

    /// 检查文件描述符是否有效
    pub fn is_valid_fd(&self, fd: usize, pid: usize) -> bool {
        self.sockets
            .iter()
            .find(|x| x.pid == pid && x.fd == fd)
            .is_some()
    }

    /// 清理所有套接字
    pub fn cleanup(&mut self) {
        for socket in self.sockets.iter_mut() {
            let _ = socket.close();
        }
        self.sockets.clear();
    }
}

pub struct SocketFile {
    pub _fd: usize,
    pub _pid: usize,
}

impl File for SocketFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        // 假设 Socket 内部有 inner 字段
        panic!("[Stdout]: don not support get file_inner")
    }

    fn readable(&self) -> bool {
        true
    }

        fn get_inode(&self) -> Option<Arc<dyn Inode>> {
            None
        }

        fn get_offset(&self) -> usize {
            0
        }

        fn set_offset(&self, _new_offset: usize) {
            // socket 不支持 seek。
        }

    fn writable(&self) -> bool {
        true
    }

    fn read(&self, _buf: UserBuffer) -> SysResult<usize> {
        Ok(0)
    }

    fn write(&self, _buf: UserBuffer) -> SysResult<usize> {
        Ok(0)
    }
}
