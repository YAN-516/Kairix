// src/syscall/mod.rs

use crate::net::route::route_lookup;
use crate::net::skb::Skb;
use crate::socket::SOCKET_MANAGER;
use crate::socket::raw::{self, RawSocket, register_raw_socket, send_raw_packet};
use crate::socket::udp::{UdpSocket, register_udp_socket, send_udp_packet};
use crate::socket::{Socket, SocketFile, SocketInner, SocketState};
use crate::task::*;
use crate::trap::_set_sum_bit;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::mem;
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
pub fn sys_socket(domain: i32, type_: i32, protocol: i32) -> isize {
    // 检查协议族
    if domain != 2 {
        return -1;
    }

    // 检查协议类型
    if protocol < 0 || protocol > 255 {
        return -1;
    }

    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    let fd;
    let pid = process.getpid();
    let socket = match type_ {
        2 => {
            // SOCK_DGRAM (UDP)
            //println!("new udp socket");
            fd = inner.alloc_fd();
            let udp = UdpSocket::new();
            Socket::new(SocketInner::Udp(Arc::new(Mutex::new(udp))), fd, pid)
        }
        3 => {
            // SOCK_RAW (Raw socket for ICMP)
            fd = inner.alloc_fd();
            let raw = Arc::new(Mutex::new(RawSocket::new(protocol as u8)));
            register_raw_socket(protocol as u8, raw.clone());
            Socket::new(SocketInner::Raw(raw), fd, pid)
        }
        _ => return -1,
    };
    inner.fd_table[fd] = Some(Arc::new(SocketFile { _fd: fd, _pid: pid }));
    let _ret = SOCKET_MANAGER.lock().add_socket(fd, socket, pid);

    log::info!(
        "sys_socket: created socket fd={}, type={}, protocol={}",
        fd,
        type_,
        protocol
    );
    fd as isize
}

/// bind() 系统调用
///
/// # 参数
/// - `fd`: 文件描述符
/// - `addr_ptr`: sockaddr_in 结构指针
/// - `addr_len`: 地址结构长度
pub fn sys_bind(fd: usize, addr_ptr: *const u8, addr_len: usize) -> isize {
    _set_sum_bit();
    // 检查地址长度
    if addr_len != mem::size_of::<SockaddrIn>() {
        return -1;
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
        return -1;
    }

    if socket.state != SocketState::Open {
        return -1;
    }

    // 解析地址
    let sockaddr = unsafe { &*(addr_ptr as *const SockaddrIn) };

    if sockaddr.sin_family != 2 {
        return -1;
    }

    let port = u16::from_be(sockaddr.sin_port);
    let addr = sockaddr.sin_addr;

    // 根据套接字类型执行绑定
    match &mut socket.inner {
        SocketInner::Udp(udp) => {
            let mut udp_guard = udp.lock();
            if udp_guard.bind(addr, port).is_err() {
                return -1;
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
    0
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
) -> isize {
    // 检查参数
    _set_sum_bit();
    println!("enter sys sendto...");
    if buf_ptr.is_null() && len > 0 {
        return -1;
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
            return -1;
        }
        let sockaddr = unsafe { &*(addr_ptr as *const SockaddrIn) };
        if sockaddr.sin_family != 2 {
            return -1;
        }
        (sockaddr.sin_addr, u16::from_be(sockaddr.sin_port))
    };

    // 获取套接字
    let process = current_process();
    let pid = process.getpid();
    let mut manager: spin::MutexGuard<'_, crate::socket::SocketManager> = SOCKET_MANAGER.lock();

    let (udp_socket, raw_socket) = {
        if let Some(sock) = manager.get_socket_mut(fd, pid) {
            if sock.is_closed() {
                return -1;
            }
            match &sock.inner {
                SocketInner::Udp(udp) => (Some(udp.clone()), None),
                SocketInner::Raw(raw) => (None, Some(raw.clone())),
            }
        } else {
            return -1;
        }
    };
    drop(manager);

    // 根据套接字类型执行发送
    if let Some(udp) = udp_socket {
        let src = {
            let udp_guard = udp.lock();
            match udp_guard.local_addr() {
                Some(v) => v,
                None => return -1,
            }
        };
        println!("sendto udp {} bytes to {}:{}", len, dst_addr, dst_port);
        if send_udp_packet(src, data, dst_addr, dst_port).is_err() {
            return -1;
        }
        log::debug!(
            "sys_sendto: UDP socket fd={} sent {} bytes to {}:{}",
            fd,
            len,
            dst_addr,
            dst_port
        );
        len as isize
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
                        return -1;
                    }
                    ip
                }
                Err(_) => return -1,
            }
        };

        if send_raw_packet(src_addr, protocol, data, dst_addr).is_err() {
            return -1;
        }
        log::debug!(
            "sys_sendto: Raw socket fd={} sent {} bytes to {} (src={})",
            fd,
            len,
            dst_addr,
            src_addr
        );
        len as isize
    } else {
        -1
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
) -> isize {
    _set_sum_bit();
    // 检查参数
    if buf_ptr.is_null() && len > 0 {
        return -1;
    }

    let buf = if len > 0 {
        unsafe { core::slice::from_raw_parts_mut(buf_ptr, len) }
    } else {
        &mut []
    };

    let process = current_process();
    let pid = process.getpid();
    let mut manager = SOCKET_MANAGER.lock();
    let (udp_socket, raw_socket) = {
        if let Some(sock) = manager.get_socket_mut(fd, pid) {
            if sock.is_closed() {
                return -1;
            }
            match &sock.inner {
                SocketInner::Udp(udp) => (Some(udp.clone()), None),
                SocketInner::Raw(raw) => (None, Some(raw.clone())),
            }
        } else {
            return -1;
        }
    };
    drop(manager);

    // 根据套接字类型执行接收
    if let Some(udp) = udp_socket {
        let udp_guard = udp.lock();
        let (recv_len, src_addr, src_port) = match udp_guard.recv_from(buf) {
            Ok(v) => v,
            Err(_) => return -1,
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
        recv_len as isize
    } else if let Some(raw) = raw_socket {
        let recv_len = match raw.lock().recv_from(buf) {
            Ok(v) => v,
            Err(_) => return -1,
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
        recv_len as isize
    } else {
        -1
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
