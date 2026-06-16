use crate::error::{SysError, SysResult, SyscallResult};
use crate::task::*;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::{Mutex, MutexGuard};
pub mod raw;
#[allow(missing_docs)]
pub mod tcp;
pub mod udp;
use crate::fs::File;
use crate::fs::vfs::FileInner;
use crate::fs::vfs::inode::Inode;
use crate::mm::UserBuffer;
use crate::net::tcp::{tcp_send_data, tcp_send_segment};
use crate::net::tcp::{TCP_FLAG_ACK, TCP_FLAG_FIN};
use crate::socket::raw::RawSocket;
use crate::socket::tcp::TcpSocketState;
use lazy_static::lazy_static;
use log::error;
use raw::unregister_raw_socket;
use tcp::TcpSocket;
use udp::UdpSocket;
use udp::unregister_udp_socket;
lazy_static! {
    pub static ref SOCKET_MANAGER: Mutex<SocketManager> = Mutex::new(SocketManager::new());
}
#[allow(unused)]
/// 套接字类型
#[derive(Clone)]
pub enum SocketInner {
    Raw(Arc<Mutex<RawSocket>>),
    Udp(Arc<Mutex<UdpSocket>>),
    Tcp(Arc<Mutex<TcpSocket>>),
    Unix(UnixSocket),
}

#[allow(unused)]
/// Minimal AF_UNIX socket placeholder.
#[derive(Clone)]
pub struct UnixSocket {
    pub sock_type: i32,
    pub protocol: i32,
    pub abstract_name: Option<alloc::string::String>,
    pub peer_pid: Option<usize>,
}

impl UnixSocket {
    pub fn new(sock_type: i32, protocol: i32) -> Self {
        Self {
            sock_type,
            protocol,
            abstract_name: None,
            peer_pid: None,
        }
    }
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
    pub shut_rd: bool,
    pub shut_wr: bool,
    pub flags: u32,
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
            shut_rd: false,
            shut_wr: false,
            flags: 0,
        }
    }

    /// 关闭套接字
    pub fn close(&mut self) -> SysResult<()> {
        if self.closed.load(Ordering::Acquire) {
            return Err(SysError::EINVAL);
        }

        self.closed.store(true, Ordering::Release);
        self.state = SocketState::Closed;

        // 清理接收队列等资源
        match &mut self.inner {
            SocketInner::Unix(_) => {}
            SocketInner::Udp(udp_socket) => {
                log::info!("Closing UDP socket fd={} pid={}", self.fd, self.pid);
                let mut udp = udp_socket.lock();
                if let Some((_, port)) = udp.local_addr() {
                    unregister_udp_socket(port, udp_socket.clone());
                }
                udp.clear_queue();
            }
            SocketInner::Raw(raw_socket) => {
                log::info!("Closing RAW socket fd={} pid={}", self.fd, self.pid);
                let protocol = raw_socket.lock().protocol();
                unregister_raw_socket(protocol, raw_socket.clone());
                raw_socket.lock().clear_queue();
            }
            SocketInner::Tcp(tcp_socket) => {
                error!("Closing TCP socket fd={} pid={}", self.fd, self.pid);
                let (local_ip, local_port, remote_ip, remote_port, send_seq, recv_seq, need_fin) = {
                    let tcp = tcp_socket.lock();
                    //println!("state before close: {:?}", tcp.state);
                    if tcp.state == TcpSocketState::Closed {
                        return Ok(());
                    }
                    let (local_ip, local_port, remote_ip, remote_port) =
                        match (tcp.local_addr, tcp.remote_addr) {
                            (Some(l), Some(r)) => (l.0, l.1, r.0, r.1),
                            _ => (0, 0, 0, 0),
                        };
                    let need_fin = matches!(
                        tcp.state,
                        TcpSocketState::Established | TcpSocketState::CloseWait
                    );
                    (
                        local_ip,
                        local_port,
                        remote_ip,
                        remote_port,
                        tcp.send_seq,
                        tcp.recv_seq,
                        need_fin,
                    )
                };
                // println!(
                //     "TCP close: local=({}:{}) remote=({}:{}) send_seq={} recv_seq={} need_fin={}",
                //     local_ip, local_port, remote_ip, remote_port, send_seq, recv_seq, need_fin
                // );
                if need_fin && remote_port != 0 {
                    let _ = tcp_send_segment(
                        local_ip,
                        remote_ip,
                        local_port,
                        remote_port,
                        send_seq,
                        recv_seq,
                        TCP_FLAG_FIN | TCP_FLAG_ACK,
                        &[],
                    );
                    let mut tcp = tcp_socket.lock();
                    tcp.send_seq = tcp.send_seq.wrapping_add(1);
                    drop(tcp);
                }
                let _ = tcp_socket.lock().close();
            }
        }
        // println!("finish closing socket fd={} pid={}", self.fd, self.pid);
        Ok(())
    }

    /// 半关闭套接字
    pub fn shutdown(&mut self, how: i32) -> Result<(), &'static str> {
        match how {
            0 => {
                // SHUT_RD
                self.shut_rd = true;
                match &self.inner {
                    SocketInner::Udp(udp) => {
                        let mut udp = udp.lock();
                        udp.receive_queue.lock().clear();
                    }
                    SocketInner::Tcp(tcp) => {
                        tcp.lock().receive_queue.lock().clear();
                    }
                    SocketInner::Unix(_) => {}
                    SocketInner::Raw(_) => {}
                }
            }
            1 => {
                // SHUT_WR
                if self.shut_wr {
                    return Ok(());
                }
                self.shut_wr = true;
                if let SocketInner::Tcp(tcp) = &self.inner {
                    let (local_ip, local_port, remote_ip, remote_port, send_seq, recv_seq) = {
                        let mut tcp = tcp.lock();
                        let (local_ip, local_port, remote_ip, remote_port) =
                            match (tcp.local_addr, tcp.remote_addr) {
                                (Some(l), Some(r)) => (l.0, l.1, r.0, r.1),
                                _ => return Ok(()),
                            };
                        if !matches!(tcp.state, crate::socket::tcp::TcpSocketState::Established) {
                            return Ok(());
                        }
                        let send_seq = tcp.send_seq;
                        tcp.send_seq = tcp.send_seq.wrapping_add(1);
                        tcp.state = crate::socket::tcp::TcpSocketState::FinWait1;
                        (
                            local_ip,
                            local_port,
                            remote_ip,
                            remote_port,
                            send_seq,
                            tcp.recv_seq,
                        )
                    };
                    let _ = crate::net::tcp::tcp_send_segment(
                        local_ip,
                        remote_ip,
                        local_port,
                        remote_port,
                        send_seq,
                        recv_seq,
                        crate::socket::tcp::TCP_FLAG_FIN | crate::socket::tcp::TCP_FLAG_ACK,
                        &[],
                    );
                }
            }
            2 => {
                // SHUT_RDWR
                self.shut_rd = true;
                if !self.shut_wr {
                    self.shut_wr = true;
                    if let SocketInner::Tcp(tcp) = &self.inner {
                        let (local_ip, local_port, remote_ip, remote_port, send_seq, recv_seq) = {
                            let mut tcp = tcp.lock();
                            let (local_ip, local_port, remote_ip, remote_port) =
                                match (tcp.local_addr, tcp.remote_addr) {
                                    (Some(l), Some(r)) => (l.0, l.1, r.0, r.1),
                                    _ => return Ok(()),
                                };
                            if !matches!(tcp.state, crate::socket::tcp::TcpSocketState::Established)
                            {
                                return Ok(());
                            }
                            let send_seq = tcp.send_seq;
                            tcp.send_seq = tcp.send_seq.wrapping_add(1);
                            tcp.state = crate::socket::tcp::TcpSocketState::FinWait1;
                            (
                                local_ip,
                                local_port,
                                remote_ip,
                                remote_port,
                                send_seq,
                                tcp.recv_seq,
                            )
                        };
                        let _ = crate::net::tcp::tcp_send_segment(
                            local_ip,
                            remote_ip,
                            local_port,
                            remote_port,
                            send_seq,
                            recv_seq,
                            crate::socket::tcp::TCP_FLAG_FIN | crate::socket::tcp::TCP_FLAG_ACK,
                            &[],
                        );
                    }
                }
                match &self.inner {
                    SocketInner::Udp(udp) => {
                        let mut udp = udp.lock();
                        udp.receive_queue.lock().clear();
                    }
                    SocketInner::Tcp(tcp) => {
                        tcp.lock().receive_queue.lock().clear();
                    }
                    SocketInner::Unix(_) => {}
                    SocketInner::Raw(_) => {}
                }
            }
            _ => return Err("Invalid how"),
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
    /// 如果该 (pid, fd) 已存在，会关闭旧 socket 并替换，防止 fd 复用后身份错乱。
    pub fn add_socket(&mut self, fd: usize, mut socket: Socket, pid: usize) -> SysResult<usize> {
        socket.fd = fd;
        socket.pid = pid;

        if let Some(pos) = self.sockets.iter().position(|x| x.pid == pid && x.fd == fd) {
            log::info!(
                "SocketManager: replacing stale socket fd={} pid={}",
                fd,
                pid
            );
            let _ = self.sockets[pos].close();
            self.sockets[pos] = socket;
        } else {
            self.sockets.push(socket);
        }

        log::info!("SocketManager: added socket fd={}", fd);
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
    pub fn close_socket(&mut self, fd: usize, pid: usize) -> SysResult<()> {
        if let Some(mut socket) = self.remove_socket(fd, pid) {
            socket.close()?;
            Ok(())
        } else {
            Err(SysError::EBADF)
        }
    }

    /// 关闭并移除套接字（带引用计数：只有没有其他进程持有同一底层 socket 时才真正关闭）
    pub fn close_socket_with_refcount(
        &mut self,
        fd: usize,
        pid: usize,
    ) -> Result<(), &'static str> {
        let socket = self.remove_socket(fd, pid).ok_or("Socket not found")?;

        let still_in_use = match &socket.inner {
            SocketInner::Tcp(tcp) => self.sockets.iter().any(|other| {
                if let SocketInner::Tcp(other_tcp) = &other.inner {
                    Arc::ptr_eq(other_tcp, tcp)
                } else {
                    false
                }
            }),
            SocketInner::Udp(udp) => self.sockets.iter().any(|other| {
                if let SocketInner::Udp(other_udp) = &other.inner {
                    Arc::ptr_eq(other_udp, udp)
                } else {
                    false
                }
            }),
            SocketInner::Raw(raw) => self.sockets.iter().any(|other| {
                if let SocketInner::Raw(other_raw) = &other.inner {
                    Arc::ptr_eq(other_raw, raw)
                } else {
                    false
                }
            }),
            SocketInner::Unix(_) => false,
        };

        if !still_in_use {
            let mut socket = socket;
            let _ = socket.close();
        }
        Ok(())
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

    fn is_socket(&self) -> bool {
        true
    }

    fn supports_epoll(&self) -> bool {
        true
    }

    fn register_poll_waker(&self, task: Arc<crate::task::TaskControlBlock>) {
        let pid = crate::task::current_process().getpid();
        let (tcp_socket, udp_socket) = {
            let manager = SOCKET_MANAGER.lock();
            let Some(sock) = manager.get_socket(self._fd, pid) else {
                return;
            };
            match &sock.inner {
                SocketInner::Tcp(tcp) => (Some(tcp.clone()), None),
                SocketInner::Udp(udp) => (None, Some(udp.clone())),
                SocketInner::Raw(_) | SocketInner::Unix(_) => (None, None),
            }
        };

        if let Some(tcp) = tcp_socket {
            let waker = crate::task::task_waker_front(task);
            let tcp_guard = tcp.lock();
            if matches!(tcp_guard.state, TcpSocketState::Listening) {
                tcp_guard.accept_waker.lock().replace(waker);
            } else {
                tcp_guard.recv_waker.lock().replace(waker);
            }
        } else if let Some(udp) = udp_socket {
            udp.lock().set_waker(Some(task));
        }
    }

    fn clear_poll_waker(&self, _task: &Arc<crate::task::TaskControlBlock>) {
        let pid = crate::task::current_process().getpid();
        let (tcp_socket, udp_socket) = {
            let manager = SOCKET_MANAGER.lock();
            let Some(sock) = manager.get_socket(self._fd, pid) else {
                return;
            };
            match &sock.inner {
                SocketInner::Tcp(tcp) => (Some(tcp.clone()), None),
                SocketInner::Udp(udp) => (None, Some(udp.clone())),
                SocketInner::Raw(_) | SocketInner::Unix(_) => (None, None),
            }
        };

        if let Some(tcp) = tcp_socket {
            let tcp_guard = tcp.lock();
            tcp_guard.accept_waker.lock().take();
            tcp_guard.recv_waker.lock().take();
        } else if let Some(udp) = udp_socket {
            udp.lock().set_waker(None);
        }
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

    fn status_flags(&self) -> u32 {
        let flags = SOCKET_MANAGER
            .lock()
            .get_socket(self._fd, self._pid)
            .map(|sock| sock.flags)
            .unwrap_or(0);
        0o2 | (flags & !1)
    }

    fn set_status_flags(&self, flags: u32) {
        const SOCKET_SETFL_MASK: u32 =
            0o4000 | 0o2000 | 0o10000 | 0o40000 | 0o100000 | 0o1000000 | 0o4000000;
        if let Some(sock) = SOCKET_MANAGER.lock().get_socket_mut(self._fd, self._pid) {
            sock.flags = (sock.flags & 1) | (flags & SOCKET_SETFL_MASK);
        }
    }

    fn read(&self, _buf: UserBuffer) -> SyscallResult {
        let pid = crate::task::current_process().getpid();
        // log::error!("SocketFile::read ENTER fd={} pid={}", self._fd, pid);
        let mut buf = _buf;
        let want = buf.len();
        if want == 0 {
            // log::error!("SocketFile::read: want=0, returning 0");
            return Ok(0);
        }

        loop {
            let (tcp_socket, udp_socket, unix_socket) = {
                let mut manager = SOCKET_MANAGER.lock();
                let Some(sock) = manager.get_socket_mut(self._fd, pid) else {
                    // log::error!("SocketFile::read: no socket for fd={} pid={}", self._fd, pid);
                    return Ok(0);
                };
                if sock.is_closed() {
                    // log::error!("SocketFile::read: socket closed fd={} pid={}", self._fd, pid);
                    return Ok(0);
                }
                match &sock.inner {
                    SocketInner::Tcp(tcp) => (Some(tcp.clone()), None, false),
                    SocketInner::Udp(udp) => (None, Some(udp.clone()), false),
                    SocketInner::Raw(_) => (None, None, false),
                    SocketInner::Unix(_) => (None, None, true),
                }
            };
            if unix_socket {
                return Err(SysError::EINVAL);
            }

            if let Some(tcp) = tcp_socket {
                let n = {
                    let guard = tcp.lock();
                    match guard.recv_user_buffer(&mut buf) {
                        Ok((n, _, _)) => n,
                        Err(_) => {
                            if matches!(
                                guard.state,
                                crate::socket::tcp::TcpSocketState::CloseWait
                                    | crate::socket::tcp::TcpSocketState::LastAck
                                    | crate::socket::tcp::TcpSocketState::Closed
                                    | crate::socket::tcp::TcpSocketState::FinWait1
                                    | crate::socket::tcp::TcpSocketState::FinWait2
                            ) {
                                return Ok(0);
                            }
                            drop(guard);
                            let waker =
                                crate::task::task_waker_front(crate::task::current_task().unwrap());
                            tcp.lock().recv_waker.lock().replace(waker);
                            suspend_current_and_run_next();
                            continue;
                        }
                    }
                };

                // log::error!("SocketFile::read RETURN fd={} pid={} n={}", self._fd, pid, n);
                return Ok(n);
            }

            if let Some(udp) = udp_socket {
                let n = {
                    let guard = udp.lock();
                    match guard.recv_user_buffer(&mut buf) {
                        Ok((n, _, _)) => n,
                        Err(_) => {
                            drop(guard);
                            if let Some(task) = crate::task::current_task() {
                                udp.lock().set_waker(Some(task));
                            }
                            suspend_current_and_run_next();
                            continue;
                        }
                    }
                };

                return Ok(n);
            }

            // raw socket 暂不支持通过 read() 读取
            return Ok(0);
        }
    }

    fn write(&self, _buf: UserBuffer) -> SyscallResult {
        let pid = crate::task::current_process().getpid();
        // log::error!("SocketFile::write ENTER fd={} pid={} total={}", self._fd, pid, _buf.len());
        let buf = _buf;
        let total = buf.len();
        if total == 0 {
            // log::error!("SocketFile::write: total=0, returning 0");
            return Ok(0);
        }

        let (tcp_socket, udp_socket, unix_socket) = {
            let mut manager = SOCKET_MANAGER.lock();
            let Some(sock) = manager.get_socket_mut(self._fd, pid) else {
                // log::error!("SocketFile::write: no socket for fd={} pid={}", self._fd, pid);
                return Ok(0);
            };
            if sock.is_closed() {
                // log::error!("SocketFile::write: socket closed fd={} pid={}", self._fd, pid);
                return Ok(0);
            }
            match &sock.inner {
                SocketInner::Tcp(tcp) => (Some(tcp.clone()), None, false),
                SocketInner::Udp(udp) => (None, Some(udp.clone()), false),
                SocketInner::Raw(_) => (None, None, false),
                SocketInner::Unix(_) => (None, None, true),
            }
        };
        if unix_socket {
            return Err(SysError::EINVAL);
        }

        if let Some(tcp) = tcp_socket {
            // ✅ 关键修改：在锁内读取必要信息，然后释放锁再发送

            // 步骤1：在锁内读取发送所需参数
            let (local_ip, local_port, remote_ip, remote_port, seq, ack) = {
                let guard = tcp.lock();
                if guard.state != TcpSocketState::Established {
                    // log::error!("SocketFile::write: TCP not Established fd={} pid={} state={:?}", self._fd, pid, guard.state);
                    return Ok(0);
                }
                let (local_ip, local_port) = guard.local_addr.unwrap();
                let (remote_ip, remote_port) = guard.remote_addr.unwrap();
                (
                    local_ip,
                    local_port,
                    remote_ip,
                    remote_port,
                    guard.send_seq,
                    guard.recv_seq,
                )
            }; // ← 锁在这里释放！

            let mut next_seq = seq;
            let mut sent_total = 0usize;
            for slice in buf.buffers.iter() {
                if slice.is_empty() {
                    continue;
                }
                match tcp_send_data(
                    local_ip,
                    remote_ip,
                    local_port,
                    remote_port,
                    next_seq,
                    ack,
                    &slice[..],
                ) {
                    Ok((sent, new_seq)) => {
                        sent_total += sent;
                        next_seq = new_seq;
                        if sent < slice.len() {
                            break;
                        }
                    }
                    Err(_e) => {
                        if sent_total == 0 {
                            return Ok(0);
                        }
                        break;
                    }
                }
            }

            // 步骤3：重新获取锁更新序号
            {
                let mut guard = tcp.lock();
                guard.send_seq = next_seq;
            }

            // log::error!("SocketFile::write RETURN fd={} pid={} len={}", self._fd, pid, data.len());
            return Ok(sent_total);
        }

        if let Some(udp) = udp_socket {
            let guard = udp.lock();
            if let Some((dst_ip, dst_port)) = guard.remote_addr() {
                return Ok(guard
                    .send_user_buffer_to(&buf, dst_ip, dst_port)
                    .map(|_| total)
                    .unwrap_or(0));
            }
            return Ok(0);
        }

        Ok(0)
    }
}
