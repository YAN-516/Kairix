use alloc::sync::Arc;
use spin::Mutex;

pub mod raw;
pub mod tcp;
pub mod udp;

use raw::RawSocket;
use tcp::TcpSocket;
use udp::UdpSocket;

/// 套接字类型
pub enum SocketInner {
    Raw(RawSocket),
    Udp(Arc<Mutex<UdpSocket>>),
    Tcp(TcpSocket),
}

/// 套接字
pub struct Socket {
    pub inner: SocketInner,
    pub fd: usize,
}

/// 套接字管理器
pub struct SocketManager {
    sockets: Vec<Option<Socket>>,
    next_fd: usize,
}

impl SocketManager {
    pub fn new() -> Self {
        Self {
            sockets: Vec::new(),
            next_fd: 3, // 0,1,2 是标准文件描述符
        }
    }

    pub fn add_socket(&mut self, socket: Socket) -> usize {
        let fd = self.next_fd;
        self.next_fd += 1;

        while self.sockets.len() <= fd {
            self.sockets.push(None);
        }
        self.sockets[fd] = Some(socket);
        fd
    }

    pub fn get_socket(&self, fd: usize) -> Option<&Socket> {
        self.sockets.get(fd).and_then(|s| s.as_ref())
    }

    pub fn remove_socket(&mut self, fd: usize) -> Option<Socket> {
        if fd < self.sockets.len() {
            self.sockets[fd].take()
        } else {
            None
        }
    }
}

static SOCKET_MANAGER: Mutex<Option<SocketManager>> = Mutex::new(None);

pub fn init() {
    *SOCKET_MANAGER.lock() = Some(SocketManager::new());
}

pub fn socket_manager() -> &'static Mutex<Option<SocketManager>> {
    &SOCKET_MANAGER
}
