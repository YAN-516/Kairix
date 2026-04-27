// src/syscall/mod.rs

use crate::error::{SysError, SyscallResult};
use crate::net::route::route_lookup;
use crate::net::skb::Skb;
use crate::socket::SOCKET_MANAGER;
use crate::socket::raw::{self, RawSocket, register_raw_socket, send_raw_packet};
use crate::socket::tcp::{self, TcpSocket};
use crate::socket::udp::{UdpSocket, register_udp_socket, send_udp_packet};
use crate::socket::{Socket, SocketFile, SocketInner, SocketState};
use crate::task::*;
use crate::trap::_set_sum_bit;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::mem;
use log::error;
use polyhal::println;
use spin::Mutex;
/// socket() 系统调用
///
/// # 参数
/// - `domain`: 协议族 (AF_INET = 2)
/// - `type_`: 套接字类型 (SOCK_STREAM=1, SOCK_DGRAM=2, SOCK_RAW=3)
/// - `protocol`: 协议号 (IPPROTO_ICMP=1, IPPROTO_UDP=17, IPPROTO_TCP=6)
///
/// # 返回
/// - 成功: 文件描述符
/// - 失败: 错误信息
pub fn sys_socket(domain: i32, type_: i32, protocol: i32) -> SyscallResult {
    // 检查协议族
    if domain != 2 {
        return Err(SysError::EINVAL);
    }

    // 检查协议类型
    if protocol < 0 || protocol > 255 {
        return Err(SysError::EINVAL);
    }

    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    let fd;
    let pid = process.getpid();
    let socket = match type_ {
        1 => {
            fd = inner.alloc_fd();
            let tcp = TcpSocket::new();
            Socket::new(SocketInner::Tcp(Arc::new(Mutex::new(tcp))), fd, pid)
        }
        2 => {
            fd = inner.alloc_fd();
            let udp = UdpSocket::new();
            Socket::new(SocketInner::Udp(Arc::new(Mutex::new(udp))), fd, pid)
        }
        3 => {
            fd = inner.alloc_fd();
            let raw = Arc::new(Mutex::new(RawSocket::new(protocol as u8)));
            register_raw_socket(protocol as u8, raw.clone());
            Socket::new(SocketInner::Raw(raw), fd, pid)
        }
        _ => return Err(SysError::EINVAL),
    };
    inner.fd_table[fd] = Some(Arc::new(SocketFile { _fd: fd, _pid: pid }));
    let _ret = SOCKET_MANAGER.lock().add_socket(fd, socket, pid);

    log::info!(
        "sys_socket: created socket fd={}, type={}, protocol={}",
        fd,
        type_,
        protocol
    );
    Ok(fd)
}

/// bind() 系统调用
///
/// # 参数
/// - `fd`: 文件描述符
/// - `addr_ptr`: sockaddr_in 结构指针
/// - `addr_len`: 地址结构长度
pub fn sys_bind(fd: usize, addr_ptr: *const u8, addr_len: usize) -> SyscallResult {
    _set_sum_bit();
    // 检查地址长度
    if addr_len != mem::size_of::<SockaddrIn>() {
        return Err(SysError::EINVAL);
    }

    let process = current_process();
    let pid = process.getpid();
    let mut manager = SOCKET_MANAGER.lock();
    let socket = {
        if let Some(sock) = manager.get_socket_mut(fd, pid) {
            sock
        } else {
            panic!("Invalid")
        }
    };
    // 检查套接字状态
    if socket.is_closed() {
        return Err(SysError::EINVAL);
    }

    if socket.state != SocketState::Open {
        return Err(SysError::EINVAL);
    }

    // 解析地址
    let sockaddr = unsafe { &*(addr_ptr as *const SockaddrIn) };

    if sockaddr.sin_family != 2 {
        return Err(SysError::EINVAL);
    }

    let port = u16::from_be(sockaddr.sin_port);
    let addr = sockaddr.sin_addr;

    // 根据套接字类型执行绑定
    match &mut socket.inner {
        SocketInner::Tcp(tcp_socket) => {
            if tcp_socket.lock().bind(addr, port).is_err() {
                return Err(SysError::EINVAL);
            }
            socket.state = SocketState::Bound;
            log::debug!("sys_bind: TCP socket fd={} bound to {}:{}", fd, addr, port);
        }
        SocketInner::Udp(udp) => {
            let mut udp_guard = udp.lock();
            if udp_guard.bind(addr, port).is_err() {
                return Err(SysError::EINVAL);
            }
            register_udp_socket(port, udp.clone());
            socket.state = SocketState::Bound;
            log::debug!("sys_bind: UDP socket fd={} bound to {}:{}", fd, addr, port);
        }
        SocketInner::Raw(_) => {
            // 原始套接字不需要绑定
            socket.state = SocketState::Bound;
            log::debug!("sys_bind: Raw socket fd={} (no actual bind needed)", fd);
        }
    }
    Ok(0)
}

/// sendto() 系统调用
///
/// # 参数
/// - `fd`: 文件描述符
/// - `buf_ptr`: 数据缓冲区指针
/// - `len`: 数据长度
/// - `flags`: 标志位（暂未使用）
/// - `addr_ptr`: 目标地址结构指针
/// - `addr_len`: 地址结构长度
pub fn sys_sendto(
    fd: usize,
    buf_ptr: *const u8,
    len: usize,
    _flags: i32,
    addr_ptr: *const u8,
    addr_len: usize,
) -> SyscallResult {
    // 检查参数
    _set_sum_bit();
    println!("enter sys sendto...");
    if buf_ptr.is_null() && len > 0 {
        return Err(SysError::EINVAL);
    }

    // 读取数据
    // println!("{:?}", len);
    let data = if len > 0 {
        unsafe { core::slice::from_raw_parts(buf_ptr, len) }
    } else {
        &[]
    };
    // 解析目标地址
    let (dst_addr, dst_port) = if addr_ptr.is_null() {
        // 如果没有提供地址，使用默认回环地址
        (0x7F000001, 0)
    } else {
        if addr_len != mem::size_of::<SockaddrIn>() {
            return Err(SysError::EINVAL);
        }
        let sockaddr = unsafe { &*(addr_ptr as *const SockaddrIn) };
        if sockaddr.sin_family != 2 {
            return Err(SysError::EINVAL);
        }
        (sockaddr.sin_addr, u16::from_be(sockaddr.sin_port))
    };

    // 获取套接字
    let process = current_process();
    let pid = process.getpid();
    let mut manager: spin::MutexGuard<'_, crate::socket::SocketManager> = SOCKET_MANAGER.lock();

    let (udp_socket, raw_socket, tcp_socket) = if let Some(sock) = manager.get_socket_mut(fd, pid) {
        if sock.is_closed() {
            return Err(SysError::EINVAL);
        }
        let mut udp_socket = None;
        let mut raw_socket = None;
        let mut tcp_socket = None;
        match &sock.inner {
            SocketInner::Tcp(tcp) => tcp_socket = Some(tcp.clone()),
            SocketInner::Udp(udp) => udp_socket = Some(udp.clone()),
            SocketInner::Raw(raw) => raw_socket = Some(raw.clone()),
        }
        (udp_socket, raw_socket, tcp_socket)
    } else {
        return Err(SysError::EINVAL);
    };
    drop(manager);

    // 根据套接字类型执行发送
    if let Some(udp) = udp_socket {
        let src = {
            let udp_guard = udp.lock();
            match udp_guard.local_addr() {
                Some(v) => v,
                None => return Err(SysError::EINVAL),
            }
        };
        println!("sendto udp {} bytes to {}:{}", len, dst_addr, dst_port);
        if let Err(e) = send_udp_packet(src, data, dst_addr, dst_port) {
            println!("sys_sendto udp failed: {}", e);
            return Err(SysError::EINVAL);
        }
        log::debug!(
            "sys_sendto: UDP socket fd={} sent {} bytes to {}:{}",
            fd,
            len,
            dst_addr,
            dst_port
        );
        Ok(len)
    } else if let Some(tcp) = tcp_socket {
        let (target_addr, target_port) = if addr_ptr.is_null() {
            (0, 0)
        } else {
            (dst_addr, dst_port)
        };
        let sent = {
            let tcp_guard = tcp.lock();
            match tcp_guard.send_to(data, target_addr, target_port) {
                Ok(n) => n,
                Err(e) => {
                    log::debug!("sys_sendto: TCP send failed fd={} err={}", fd, e);
                    return Err(SysError::EINVAL);
                }
            }
        };
        error!(
            "sys_sendto: TCP socket fd={} sent {} bytes to {}:{}",
            fd,
            sent,
            if target_addr == 0 {
                0x7F000001
            } else {
                target_addr
            },
            target_port
        );
        Ok(sent)
    } else if let Some(raw) = raw_socket {
        let protocol = { raw.lock().protocol() };

        // 回环目的地址使用 127.0.0.1；其余目的地址按路由选择出接口源地址。
        let src_addr = if (dst_addr & 0xFF00_0000) == 0x7F00_0000 {
            0x7F00_0001
        } else {
            match route_lookup(dst_addr) {
                Ok((dev, _)) => {
                    let ip = dev.ip_addr();
                    if ip == 0 {
                        return Err(SysError::EINVAL);
                    }
                    ip
                }
                Err(_) => return Err(SysError::EINVAL),
            }
        };

        if send_raw_packet(src_addr, protocol, data, dst_addr).is_err() {
            return Err(SysError::EINVAL);
        }
        log::debug!(
            "sys_sendto: Raw socket fd={} sent {} bytes to {} (src={})",
            fd,
            len,
            dst_addr,
            src_addr
        );
        Ok(len)
    } else {
        Err(SysError::EINVAL)
    }
}

/// recvfrom() 系统调用
///
/// # 参数
/// - `fd`: 文件描述符
/// - `buf_ptr`: 接收缓冲区指针
/// - `len`: 缓冲区长度
/// - `flags`: 标志位（暂未使用）
/// - `addr_ptr`: 源地址结构指针（输出）
/// - `addr_len`: 地址结构长度指针（输入输出）
pub fn sys_recvfrom(
    fd: usize,
    buf_ptr: *mut u8,
    len: usize,
    _flags: i32,
    addr_ptr: *mut u8,
    addr_len: *mut usize,
) -> SyscallResult {
    _set_sum_bit();
    // 检查参数
    if buf_ptr.is_null() && len > 0 {
        return Err(SysError::EINVAL);
    }

    let buf = if len > 0 {
        unsafe { core::slice::from_raw_parts_mut(buf_ptr, len) }
    } else {
        &mut []
    };

    let process = current_process();
    let pid = process.getpid();
    let mut manager = SOCKET_MANAGER.lock();
    let (udp_socket, raw_socket, tcp_socket) = if let Some(sock) = manager.get_socket_mut(fd, pid) {
        if sock.is_closed() {
            return Err(SysError::EINVAL);
        }
        let mut udp_socket = None;
        let mut raw_socket = None;
        let mut tcp_socket = None;
        match &sock.inner {
            SocketInner::Tcp(tcp) => tcp_socket = Some(tcp.clone()),
            SocketInner::Udp(udp) => udp_socket = Some(udp.clone()),
            SocketInner::Raw(raw) => raw_socket = Some(raw.clone()),
        }
        (udp_socket, raw_socket, tcp_socket)
    } else {
        return Err(SysError::EINVAL);
    };
    drop(manager);

    // 根据套接字类型执行接收
    if let Some(udp) = udp_socket {
        let udp_guard = udp.lock();
        let (recv_len, src_addr, src_port) = match udp_guard.recv_from(buf) {
            Ok(v) => v,
            Err(_) => return Err(SysError::EINVAL),
        };

        // 填充源地址（如果需要）
        if !addr_ptr.is_null() && !addr_len.is_null() {
            unsafe {
                let sockaddr = addr_ptr as *mut SockaddrIn;
                (*sockaddr).sin_family = 2; // AF_INET
                (*sockaddr).sin_port = src_port.to_be();
                (*sockaddr).sin_addr = src_addr;
                (*sockaddr).sin_zero = [0; 8];
                *addr_len = mem::size_of::<SockaddrIn>();
            }
        }

        log::debug!(
            "sys_recvfrom: UDP socket fd={} received {} bytes from {}:{}",
            fd,
            recv_len,
            src_addr,
            src_port
        );
        Ok(recv_len)
    } else if let Some(tcp) = tcp_socket {
        let (recv_len, src_addr, src_port) = loop {
            let tcp_guard = tcp.lock();
            match tcp_guard.recv_from(buf) {
                Ok(v) => break v,
                Err(_) => {
                    if matches!(
                        tcp_guard.state,
                        crate::socket::tcp::TcpSocketState::CloseWait
                            | crate::socket::tcp::TcpSocketState::LastAck
                            | crate::socket::tcp::TcpSocketState::Closed
                    ) {
                        return Ok(0);
                    }
                }
            }
            drop(tcp_guard);
            suspend_current_and_run_next();
        };

        if !addr_ptr.is_null() && !addr_len.is_null() {
            unsafe {
                let sockaddr = addr_ptr as *mut SockaddrIn;
                (*sockaddr).sin_family = 2;
                (*sockaddr).sin_port = src_port.to_be();
                (*sockaddr).sin_addr = src_addr;
                (*sockaddr).sin_zero = [0; 8];
                *addr_len = mem::size_of::<SockaddrIn>();
            }
        }

        log::debug!(
            "sys_recvfrom: TCP socket fd={} received {} bytes from {}:{}",
            fd,
            recv_len,
            src_addr,
            src_port
        );
        Ok(recv_len)
    } else if let Some(raw) = raw_socket {
        let recv_len = match raw.lock().recv_from(buf) {
            Ok(v) => v,
            Err(_) => return Err(SysError::EINVAL),
        };

        // 原始套接字也填充源地址（如果有）
        if !addr_ptr.is_null() && !addr_len.is_null() {
            unsafe {
                let sockaddr = addr_ptr as *mut SockaddrIn;
                (*sockaddr).sin_family = 2;
                (*sockaddr).sin_port = 0;
                (*sockaddr).sin_addr = 0x7F000001; // 默认回环地址
                (*sockaddr).sin_zero = [0; 8];
                *addr_len = mem::size_of::<SockaddrIn>();
            }
        }

        log::debug!(
            "sys_recvfrom: Raw socket fd={} received {} bytes",
            fd,
            recv_len
        );
        Ok(recv_len)
    } else {
        Err(SysError::EINVAL)
    }
}

/// close() 系统调用
pub fn sys_close_socket(fd: usize) -> Result<(), &'static str> {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    inner.fd_table[fd] = None;
    drop(inner);
    let pid = process.getpid();
    let mut manager = SOCKET_MANAGER.lock();

    if let Some(_sock) = manager.get_socket_mut(fd, pid) {
        let _ret = manager.close_socket(fd, pid);
    } else {
        panic!("Invalid fd")
    }

    log::debug!("sys_close: closed socket fd={}", fd);
    Ok(())
}

/// listen() 系统调用
pub fn sys_listen(fd: usize, backlog: usize) -> SyscallResult {
    let process = current_process();
    let pid = process.getpid();
    let tcp_socket = {
        let mut manager = SOCKET_MANAGER.lock();
        let Some(sock) = manager.get_socket_mut(fd, pid) else {
            return Err(SysError::EINVAL);
        };
        match &mut sock.inner {
            SocketInner::Tcp(tcp_socket) => tcp_socket.clone(),
            _ => return Err(SysError::EINVAL),
        }
    };

    match tcp::listen(tcp_socket, backlog) {
        Ok(_) => Ok(0),
        Err(_) => Err(SysError::EINVAL),
    }
}

/// connect() 系统调用
pub fn sys_connect(fd: usize, addr_ptr: *const u8, addr_len: usize) -> SyscallResult {
    _set_sum_bit();
    if addr_len != mem::size_of::<SockaddrIn>() {
        return Err(SysError::EINVAL);
    }
    let sockaddr = unsafe { &*(addr_ptr as *const SockaddrIn) };
    if sockaddr.sin_family != 2 {
        return Err(SysError::EINVAL);
    }

    let process = current_process();
    let pid = process.getpid();
    let tcp_socket = {
        let mut manager = SOCKET_MANAGER.lock();
        let Some(sock) = manager.get_socket_mut(fd, pid) else {
            return Err(SysError::EINVAL);
        };
        match &mut sock.inner {
            SocketInner::Tcp(tcp_socket) => tcp_socket.clone(),
            _ => return Err(SysError::EINVAL),
        }
    };

    match tcp::connect(
        tcp_socket,
        sockaddr.sin_addr,
        u16::from_be(sockaddr.sin_port),
    ) {
        Ok(_) => Ok(0),
        Err(_) => Err(SysError::EINVAL),
    }
}

/// accept() 系统调用
pub fn sys_accept(fd: usize, addr_ptr: *mut u8, addr_len: *mut usize) -> SyscallResult {
    _set_sum_bit();
    let process = current_process();
    let pid = process.getpid();
    let tcp_socket = {
        let mut manager = SOCKET_MANAGER.lock();
        let Some(sock) = manager.get_socket_mut(fd, pid) else {
            return Err(SysError::EINVAL);
        };
        match &mut sock.inner {
            SocketInner::Tcp(tcp_socket) => tcp_socket.clone(),
            _ => return Err(SysError::EINVAL),
        }
    };

    let child = loop {
        if let Some(child) = tcp::accept(tcp_socket.clone()) {
            log::debug!("sys_accept: got pending child on fd={}", fd);
            log::debug!("sys_accept: child ptr={:p}", Arc::as_ptr(&child));
            break child;
        }
        suspend_current_and_run_next();
    };

    let fd_new = {
        let mut inner = process.inner_exclusive_access();
        let fd_new = inner.alloc_fd();
        inner.fd_table[fd_new] = Some(Arc::new(SocketFile {
            _fd: fd_new,
            _pid: pid,
        }));
        fd_new
    };

    let socket = Socket::new(SocketInner::Tcp(child.clone()), fd_new, pid);
    log::debug!(
        "sys_accept: fd_new={} child ptr={:p}",
        fd_new,
        Arc::as_ptr(&child)
    );
    let _ = SOCKET_MANAGER.lock().add_socket(fd_new, socket, pid);

    if !addr_ptr.is_null() && !addr_len.is_null() {
        let tcp = child.lock();
        if let Some((ip, port)) = tcp.remote_addr {
            unsafe {
                let sockaddr = addr_ptr as *mut SockaddrIn;
                (*sockaddr).sin_family = 2;
                (*sockaddr).sin_port = port.to_be();
                (*sockaddr).sin_addr = ip;
                (*sockaddr).sin_zero = [0; 8];
                *addr_len = mem::size_of::<SockaddrIn>();
            }
        }
    }

    Ok(fd_new)
}

/// sockaddr_in 结构（与 C 兼容）
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct SockaddrIn {
    sin_family: u16,
    sin_port: u16,
    sin_addr: u32,
    sin_zero: [u8; 8],
}
#[allow(unused)]
impl SockaddrIn {
    /// 创建新的 IPv4 地址结构
    pub fn new(addr: u32, port: u16) -> Self {
        Self {
            sin_family: 2, // AF_INET
            sin_port: port.to_be(),
            sin_addr: addr,
            sin_zero: [0; 8],
        }
    }

    /// 创建回环地址结构
    pub fn loopback(port: u16) -> Self {
        Self::new(0x7F000001, port)
    }
}
