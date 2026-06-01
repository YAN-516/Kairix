// src/syscall/mod.rs

use crate::error::{SysError, SyscallResult};
use crate::net::route::route_lookup;
use crate::net::skb::Skb;
use crate::socket::raw::{self, register_raw_socket, send_raw_packet, RawSocket};
use crate::socket::tcp::{self, tcp_send, TcpSocket, TCP_FLAG_ACK, TCP_FLAG_PSH};
use crate::socket::udp::{register_udp_socket, send_udp_packet, UdpSocket};
use crate::socket::SOCKET_MANAGER;
use crate::socket::{Socket, SocketFile, SocketInner, SocketState, UnixSocket};
use crate::task::*;
use crate::trap::_set_sum_bit;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::mem;
use core::ptr;
use log::{error, info};
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
#[allow(unused)]
pub fn sys_socket(domain: i32, type_: i32, protocol: i32) -> SyscallResult {
    // 检查协议族
    const AF_UNIX: i32 = 1;
    const AF_INET: i32 = 2;
    const SOCK_TYPE_MASK: i32 = 0xf;
    const SOCK_NONBLOCK: i32 = 0o0004000;
    const SOCK_CLOEXEC: i32 = 0o2000000;

    // 检查协议类型
    if protocol < 0 || protocol > 255 {
        return Err(SysError::EPROTONOSUPPORT);
    }

    // 提取基础 socket 类型，兼容 SOCK_NONBLOCK / SOCK_CLOEXEC 标志位。
    let sock_type = type_ & SOCK_TYPE_MASK;
    let extra_bits = type_ & !(SOCK_TYPE_MASK | SOCK_NONBLOCK | SOCK_CLOEXEC);
    if extra_bits != 0 {
        return Err(SysError::EINVAL);
    }

    match domain {
        AF_UNIX => {
            if protocol != 0 {
                return Err(SysError::EPROTONOSUPPORT);
            }
            match sock_type {
                1 | 2 => {}
                _ => return Err(SysError::EPROTONOSUPPORT),
            }
        }
        AF_INET => match sock_type {
            1 | 2 | 3 => {}
            _ => return Err(SysError::EPROTONOSUPPORT),
        },
        _ => return Err(SysError::EAFNOSUPPORT),
    }

    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    let fd = inner.alloc_fd()?;
    let pid = process.getpid();
    let socket_inner = match domain {
        AF_UNIX => SocketInner::Unix(UnixSocket::new(sock_type, protocol)),
        AF_INET => match sock_type {
            1 => SocketInner::Tcp(Arc::new(Mutex::new(TcpSocket::new()))),
            2 => SocketInner::Udp(Arc::new(Mutex::new(UdpSocket::new()))),
            3 => {
                let raw = Arc::new(Mutex::new(RawSocket::new(protocol as u8)));
                register_raw_socket(protocol as u8, raw.clone());
                SocketInner::Raw(raw)
            }
            _ => unreachable!(),
        },
        _ => unreachable!(),
    };
    let mut socket = Socket::new(socket_inner, fd, pid);
    if (type_ & SOCK_NONBLOCK) != 0 {
        socket.flags |= 0o4000; // O_NONBLOCK
    }
    if (type_ & SOCK_CLOEXEC) != 0 {
        socket.flags |= 1; // FD_CLOEXEC
        if fd < inner.fd_flags.len() {
            inner.fd_flags[fd] |= 1;
        }
    }
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
            return Err(SysError::EBADF);
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
    let addr = u32::from_be(sockaddr.sin_addr);

    // 根据套接字类型执行绑定
    match &mut socket.inner {
        SocketInner::Tcp(tcp_socket) => {
            tcp_socket.lock().bind(addr, port)?;
            socket.state = SocketState::Bound;
            log::debug!("sys_bind: TCP socket fd={} bound to {}:{}", fd, addr, port);
        }
        SocketInner::Udp(udp) => {
            let mut udp_guard = udp.lock();
            if udp_guard.bind(addr, port).is_err() {
                return Err(SysError::EINVAL);
            }
            if let Some((_, real_port)) = udp_guard.local_addr() {
                register_udp_socket(real_port, udp.clone());
            }
            socket.state = SocketState::Bound;
            log::debug!("sys_bind: UDP socket fd={} bound to {}:{}", fd, addr, port);
        }
        SocketInner::Raw(_) => {
            // 原始套接字不需要绑定
            socket.state = SocketState::Bound;
            log::debug!("sys_bind: Raw socket fd={} (no actual bind needed)", fd);
        }
        SocketInner::Unix(_) => return Err(SysError::EOPNOTSUPP),
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
    // println!("enter sys sendto...");
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
        (
            u32::from_be(sockaddr.sin_addr),
            u16::from_be(sockaddr.sin_port),
        )
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
            SocketInner::Unix(_) => return Err(SysError::EOPNOTSUPP),
        }
        (udp_socket, raw_socket, tcp_socket)
    } else {
        return Err(SysError::EBADF);
    };
    drop(manager);

    // 根据套接字类型执行发送
    if let Some(udp) = udp_socket {
        info!(
            "sys_sendto: preparing to send UDP packet from fd={} to {}:{}",
            fd, dst_addr, dst_port
        );
        let (src, need_register) = {
            let mut udp_guard = udp.lock();
            let before = udp_guard.local_addr();
            let src = match udp_guard.ensure_local_for_dst(dst_addr) {
                Ok(v) => v,
                Err(_) => return Err(SysError::EINVAL),
            };
            let need_register = match before {
                None => true,
                Some((_, p)) => p == 0,
            };
            (src, need_register)
        };
        if need_register {
            register_udp_socket(src.1, udp.clone());
        }
        // println!("sendto udp {} bytes to {}:{}", len, dst_addr, dst_port);
        if let Err(_e) = send_udp_packet(src, data, dst_addr, dst_port) {
            // println!("sys_sendto udp failed: {}", _e);
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
        info!(
            "sys_sendto: preparing to send TCP packet from fd={} to {}:{}",
            fd, dst_addr, dst_port
        );

        if data.is_empty() {
            return Ok(0);
        }

        let (local_ip, local_port, remote_ip, remote_port, send_seq, recv_seq) = {
            let tcp_guard = tcp.lock();

            let (local_ip, local_port, remote_ip, remote_port) =
                match (tcp_guard.local_addr, tcp_guard.remote_addr) {
                    (Some(local), Some(remote)) => (local.0, local.1, remote.0, remote.1),
                    _ => {
                        log::debug!("sys_sendto: TCP socket not connected fd={}", fd);
                        return Err(SysError::EINVAL);
                    }
                };

            let check_dst = if addr_ptr.is_null() {
                remote_ip
            } else {
                dst_addr
            };

            if check_dst != 0
                && (check_dst != remote_ip || (dst_port != 0 && dst_port != remote_port))
            {
                log::debug!("sys_sendto: TCP destination mismatch fd={}", fd);
                return Err(SysError::EINVAL);
            }

            // 获取当前的序列号
            let send_seq = tcp_guard.send_seq;
            let recv_seq = tcp_guard.recv_seq;

            (
                local_ip,
                local_port,
                remote_ip,
                remote_port,
                send_seq,
                recv_seq,
            )
        }; // 锁在这里释放

        // 在锁外构造和发送 TCP 包（模仿 send_to 的内部逻辑）
        let sent = match tcp_send(
            data,
            local_ip,
            local_port,
            remote_ip,
            remote_port,
            send_seq,
            recv_seq,
        ) {
            Ok((sent_bytes, next_seq)) => {
                // 发送成功，更新序列号
                let mut tcp_guard = tcp.lock();
                tcp_guard.send_seq = next_seq;

                sent_bytes
            }
            Err(e) => {
                log::debug!("sys_sendto: TCP send failed fd={} err={}", fd, e);
                return Err(SysError::EINVAL);
            }
        };

        error!(
            "sys_sendto: TCP socket fd={} sent {} bytes to {}:{}",
            fd, sent, remote_ip, remote_port
        );
        Ok(sent)
    } else if let Some(raw) = raw_socket {
        let protocol = { raw.lock().protocol() };

        // 回环目的地址使用 127.0.0.1；其余目的地址按路由选择出接口源地址。
        let src_addr = if (dst_addr & 0xFF00_0000) == 0x7F00_0000 {
            0x7F00_0001
        } else {
            let (dev, _) = route_lookup(dst_addr).map_err(|_| SysError::ENETUNREACH)?;
            let ip = dev.ip_addr();
            if ip == 0 {
                return Err(SysError::EADDRNOTAVAIL);
            }
            ip
        };

        send_raw_packet(src_addr, protocol, data, dst_addr)?;
        log::debug!(
            "sys_sendto: Raw socket fd={} sent {} bytes to {} (src={})",
            fd,
            len,
            dst_addr,
            src_addr
        );
        Ok(len)
    } else {
        Err(SysError::ENOTSOCK)
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
#[deny(unreachable_patterns)]
pub fn sys_recvfrom(
    fd: usize,
    buf_ptr: *mut u8,
    len: usize,
    _flags: i32,
    addr_ptr: *mut u8,
    addr_len: *mut usize,
) -> SyscallResult {
    _set_sum_bit();
    // println!("enter sys recvfrom...");

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
            SocketInner::Unix(_) => return Err(SysError::EOPNOTSUPP),
        }
        (udp_socket, raw_socket, tcp_socket)
    } else {
        return Err(SysError::EBADF);
    };
    drop(manager);

    // 根据套接字类型执行接收
    if let Some(udp) = udp_socket {
        let recv_len = loop {
            let udp_guard = udp.lock();
            match udp_guard.recv_from(buf) {
                Ok((recv_len, src_addr, src_port)) => {
                    // 填充源地址（如果需要）
                    if !addr_ptr.is_null() && !addr_len.is_null() {
                        unsafe {
                            let sockaddr = addr_ptr as *mut SockaddrIn;
                            (*sockaddr).sin_family = 2; // AF_INET
                            (*sockaddr).sin_port = src_port.to_be();
                            (*sockaddr).sin_addr = src_addr.to_be();
                            (*sockaddr).sin_zero = [0; 8];
                            *addr_len = mem::size_of::<SockaddrIn>();
                        }
                    }

                    // error!(
                    //     "sys_recvfrom: UDP socket fd={} received {} bytes from {}:{}",
                    //     fd, recv_len, src_addr, src_port
                    // );
                    break recv_len;
                }
                Err(_) => {
                    drop(udp_guard);
                    // 注册 waker，让 udp_rcv 收到包后唤醒当前任务
                    if let Some(task) = crate::task::current_task() {
                        udp.lock().set_waker(Some(task));
                    }
                    suspend_current_and_run_next();
                }
            }
        };
        Ok(recv_len)
    } else if let Some(tcp) = tcp_socket {
        error!(
            "sys_recvfrom: preparing to receive TCP packet from fd={}",
            fd
        );
        let recv_len = loop {
            let tcp_guard = tcp.lock();
            match tcp_guard.recv_from(buf) {
                Ok((n, src_addr, src_port)) => {
                    if !addr_ptr.is_null() && !addr_len.is_null() {
                        unsafe {
                            let sockaddr = addr_ptr as *mut SockaddrIn;
                            (*sockaddr).sin_family = 2;
                            (*sockaddr).sin_port = src_port.to_be();
                            (*sockaddr).sin_addr = src_addr.to_be();
                            (*sockaddr).sin_zero = [0; 8];
                            *addr_len = mem::size_of::<SockaddrIn>();
                        }
                    }
                    error!(
                        "sys_recvfrom: TCP socket fd={} received {} bytes from {}:{}",
                        fd, n, src_addr, src_port
                    );
                    break n;
                }
                Err(_) => {
                    if matches!(
                        tcp_guard.state,
                        crate::socket::tcp::TcpSocketState::CloseWait
                            | crate::socket::tcp::TcpSocketState::LastAck
                            | crate::socket::tcp::TcpSocketState::Closed
                            | crate::socket::tcp::TcpSocketState::FinWait1
                            | crate::socket::tcp::TcpSocketState::FinWait2
                    ) {
                        println!(
                            "sys_recvfrom: TCP socket fd={} connection closed during recv",
                            fd
                        );
                        break 0; // EOF
                    }
                    // 注册 waker，让 tcp_rcv 收到包后唤醒当前任务
                    if let Some(task) = crate::task::current_task() {
                        let waker = crate::task::task_waker_front(task);
                        let mut waker_guard = tcp_guard.recv_waker.lock();
                        *waker_guard = Some(waker);
                    }
                    drop(tcp_guard);
                    suspend_current_and_run_next();
                }
            }
        };
        Ok(recv_len)
    } else if let Some(raw) = raw_socket {
        let recv_len = loop {
            let raw_guard = raw.lock();
            match raw_guard.recv_from(buf) {
                Ok(v) => break v,
                Err(_) => {
                    drop(raw_guard);
                    // RAW 套接字在等待期间也需要主动轮询设备 RX，
                    // 否则 echo reply 可能已到达链路层但长期不被上送。
                    crate::net::poll_rx_all();
                    suspend_current_and_run_next();
                }
            }
        };

        // 原始套接字也填充源地址（如果有）
        if !addr_ptr.is_null() && !addr_len.is_null() {
            unsafe {
                let sockaddr = addr_ptr as *mut SockaddrIn;
                (*sockaddr).sin_family = 2;
                (*sockaddr).sin_port = 0;
                (*sockaddr).sin_addr = 0x7F000001u32.to_be(); // 默认回环地址
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
        Err(SysError::ENOTSOCK)
    }
}

/// close() 系统调用（socket 专用路径）
pub fn sys_close_socket(fd: usize) -> SyscallResult {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    if fd < inner.fd_flags.len() {
        inner.fd_flags[fd] = 0;
    }
    inner.fd_table[fd] = None;
    drop(inner);
    let pid = process.getpid();
    let mut manager = SOCKET_MANAGER.lock();

    if let Some(_sock) = manager.get_socket_mut(fd, pid) {
        manager.close_socket(fd, pid)?;
    } else {
        return Err(SysError::EBADF);
    }

    log::debug!("sys_close: closed socket fd={}", fd);
    Ok(0)
}

/// shutdown() 系统调用
///
/// # 参数
/// - `fd`: 文件描述符
/// - `how`: 关闭方式 (SHUT_RD=0, SHUT_WR=1, SHUT_RDWR=2)
pub fn sys_shutdown(fd: usize, how: i32) -> SyscallResult {
    println!("enter sys shutdown...");
    #[allow(unused)]
    const ENOTSOCK: isize = -88;
    #[allow(unused)]
    const EINVAL: isize = -22;

    if how < 0 || how > 2 {
        return Err(SysError::EINVAL);
    }

    let process = current_process();
    let pid = process.getpid();
    let mut manager = SOCKET_MANAGER.lock();
    let Some(sock) = manager.get_socket_mut(fd, pid) else {
        return Err(SysError::ENOTSOCK);
    };
    if sock.is_closed() {
        return Err(SysError::ENOTSOCK);
    }

    match sock.shutdown(how) {
        Ok(_) => {
            println!("finish sys shutdown...");
            return Ok(0);
        }
        Err(_) => {
            println!("Failed to shutdown socket fd={}", fd);
            return Err(SysError::EINVAL);
        }
    }
}

/// listen() 系统调用
pub fn sys_listen(fd: usize, backlog: usize) -> SyscallResult {
    let process = current_process();
    let pid = process.getpid();
    let tcp_socket = {
        let mut manager = SOCKET_MANAGER.lock();
        let Some(sock) = manager.get_socket_mut(fd, pid) else {
            return Err(SysError::EBADF);
        };
        match &mut sock.inner {
            SocketInner::Tcp(tcp_socket) => tcp_socket.clone(),
            _ => return Err(SysError::ENOTSOCK),
        }
    };
    tcp_socket.lock().state = tcp::TcpSocketState::Listening;

    info!(
        "sys_listen: preparing to listen on TCP socket fd={} with backlog={}",
        fd, backlog
    );
    match tcp::listen(tcp_socket, backlog) {
        Ok(_) => Ok(0),
        Err(_) => Err(SysError::EINVAL),
    }
}
#[allow(unused)]
/// connect() 系统调用
pub fn sys_connect(fd: usize, addr_ptr: *const u8, addr_len: usize) -> SyscallResult {
    //error!("enter sys connect...");
    const EINVAL: isize = -22;
    const ENOTSOCK: isize = -88;
    const AF_UNIX: u16 = 1;
    const AF_INET: u16 = 2;

    _set_sum_bit();
    if addr_len < core::mem::size_of::<u16>() {
        return Err(SysError::EINVAL);
    }
    let sa_family = unsafe { *(addr_ptr as *const u16) };
    if sa_family == AF_UNIX {
        let process = current_process();
        let pid = process.getpid();
        let manager = SOCKET_MANAGER.lock();
        let Some(sock) = manager.get_socket(fd, pid) else {
            return Err(SysError::ENOTSOCK);
        };
        return match sock.inner {
            SocketInner::Unix(_) => Err(SysError::ENOENT),
            _ => Err(SysError::EAFNOSUPPORT),
        };
    }
    if sa_family != AF_INET {
        return Err(SysError::EAFNOSUPPORT);
    }
    if addr_len != mem::size_of::<SockaddrIn>() {
        return Err(SysError::EINVAL);
    }
    let sockaddr = unsafe { &*(addr_ptr as *const SockaddrIn) };
    if sockaddr.sin_family != AF_INET {
        return Err(SysError::EINVAL);
    }

    let process = current_process();
    let pid = process.getpid();
    let (tcp_socket, udp_socket) = {
        let mut manager = SOCKET_MANAGER.lock();
        let Some(sock) = manager.get_socket_mut(fd, pid) else {
            return Err(SysError::ENOTSOCK);
        };
        match &mut sock.inner {
            SocketInner::Tcp(tcp_socket) => (Some(tcp_socket.clone()), None),
            SocketInner::Udp(udp_socket) => (None, Some(udp_socket.clone())),
            _ => return Err(SysError::EINVAL),
        }
    };

    if let Some(tcp_socket) = tcp_socket {
        let is_nonblock = {
            let manager = SOCKET_MANAGER.lock();
            let sock = manager.get_socket(fd, pid).unwrap();
            (sock.flags & 0o4000) != 0
        };
        if is_nonblock {
            // 非阻塞 socket：发送 SYN 后立即返回 EINPROGRESS
            let ret = tcp::connect_nonblock(
                tcp_socket,
                u32::from_be(sockaddr.sin_addr),
                u16::from_be(sockaddr.sin_port),
            );
            match ret {
                Ok(()) => Ok(0),
                Err(e) => Err(e),
            }
        } else {
            let tcp_connect_result = tcp::connect(
                tcp_socket,
                u32::from_be(sockaddr.sin_addr),
                u16::from_be(sockaddr.sin_port),
            );
            match tcp_connect_result {
                Ok(_) => Ok(0),
                Err(e) => Err(e),
            }
        }
    } else if let Some(udp_socket) = udp_socket {
        let mut udp = udp_socket.lock();
        let old_local = udp.local_addr();
        match udp.connect(
            u32::from_be(sockaddr.sin_addr),
            u16::from_be(sockaddr.sin_port),
        ) {
            Ok(_) => {
                let new_local = udp.local_addr();
                drop(udp);
                if old_local.is_none() {
                    if let Some((_, port)) = new_local {
                        register_udp_socket(port, udp_socket.clone());
                    }
                }
                Ok(0)
            }
            Err(_) => Err(SysError::EINVAL),
        }
    } else {
        Err(SysError::EINVAL)
    }
}
#[allow(unused)]
fn write_sockaddr(addr_ptr: *mut u8, addr_len: *mut usize, ip: u32, port: u16) -> SyscallResult {
    const EFAULT: isize = -14;

    if addr_ptr.is_null() || addr_len.is_null() {
        return Err(SysError::EFAULT);
    }

    let out = SockaddrIn {
        sin_family: 2,
        sin_port: port.to_be(),
        sin_addr: ip.to_be(),
        sin_zero: [0; 8],
    };

    let cap = unsafe { *addr_len };
    let full = mem::size_of::<SockaddrIn>();
    let copy_len = cap.min(full);
    unsafe {
        ptr::copy_nonoverlapping(&out as *const SockaddrIn as *const u8, addr_ptr, copy_len);
        *addr_len = full;
    }
    Ok(0)
}
#[allow(unused)]
/// getsockname() 系统调用
pub fn sys_getsockname(fd: usize, addr_ptr: *mut u8, addr_len: *mut usize) -> SyscallResult {
    const ENOTSOCK: isize = -88;

    let process = current_process();
    let pid = process.getpid();
    let mut manager = SOCKET_MANAGER.lock();
    let Some(sock) = manager.get_socket_mut(fd, pid) else {
        return Err(SysError::ENOTSOCK);
    };
    if sock.is_closed() {
        return Err(SysError::ENOTSOCK);
    }

    let (ip, port) = match &sock.inner {
        SocketInner::Tcp(tcp) => tcp.lock().local_addr.unwrap_or((0, 0)),
        SocketInner::Udp(udp) => udp.lock().local_addr().unwrap_or((0, 0)),
        SocketInner::Raw(_) | SocketInner::Unix(_) => (0, 0),
    };
    write_sockaddr(addr_ptr, addr_len, ip, port)
}
#[allow(unused)]
/// getpeername() 系统调用
pub fn sys_getpeername(fd: usize, addr_ptr: *mut u8, addr_len: *mut usize) -> SyscallResult {
    const ENOTSOCK: isize = -88;
    const ENOTCONN: isize = -107;

    let process = current_process();
    let pid = process.getpid();
    let mut manager = SOCKET_MANAGER.lock();
    let Some(sock) = manager.get_socket_mut(fd, pid) else {
        return Err(SysError::ENOTSOCK);
    };
    if sock.is_closed() {
        return Err(SysError::ENOTSOCK);
    }

    let peer = match &sock.inner {
        SocketInner::Tcp(tcp) => tcp.lock().remote_addr,
        SocketInner::Udp(udp) => udp.lock().remote_addr(),
        SocketInner::Raw(_) | SocketInner::Unix(_) => None,
    };

    if let Some((ip, port)) = peer {
        write_sockaddr(addr_ptr, addr_len, ip, port)
    } else {
        Err(SysError::ENOTCONN)
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
            return Err(SysError::EBADF);
        };
        match &mut sock.inner {
            SocketInner::Tcp(tcp_socket) => tcp_socket.clone(),
            _ => return Err(SysError::ENOTSOCK),
        }
    };

    //添加超时计数
    let mut retry_count = 0;
    const MAX_RETRY: usize = 100000; // 大约 1 秒
    let child = loop {
        if let Some(child) = tcp::accept(tcp_socket.clone()) {
            break child;
        }

        // // // 检查 socket 是否仍然有效
        // // {
        // //     let sock_guard = tcp_socket.lock();
        // //     if sock_guard.state == tcp::TcpSocketState::Listening {
        // //         error!("sys_accept: socket no longer listening");
        // //         return -1;
        // //     }
        // // }

        // 检查超时
        retry_count += 1;
        if retry_count > MAX_RETRY {
            info!("sys_accept: timeout after {} retries", retry_count);
            return Err(SysError::ETIMEDOUT);
        }

        // 注册 waker
        {
            let sock_guard = tcp_socket.lock();
            let mut waker_guard = sock_guard.accept_waker.lock();
            let waker = task_waker_front(current_task().unwrap());
            *waker_guard = Some(waker);
        }

        suspend_current_and_run_next();
    };

    // 清除 waker
    {
        let sock_guard = tcp_socket.lock();
        let mut waker_guard = sock_guard.accept_waker.lock();
        *waker_guard = None;
    }

    let fd_new = {
        let mut inner = process.inner_exclusive_access();
        let fd_new = inner.alloc_fd()?;
        inner.fd_table[fd_new] = Some(Arc::new(SocketFile {
            _fd: fd_new,
            _pid: pid,
        }));
        fd_new
    };

    let socket = Socket::new(SocketInner::Tcp(child.clone()), fd_new, pid);
    error!(
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
                (*sockaddr).sin_addr = ip.to_be();
                (*sockaddr).sin_zero = [0; 8];
                *addr_len = mem::size_of::<SockaddrIn>();
            }
        }
    }

    Ok(fd_new)
}
#[allow(unused)]
/// setsockopt() 系统调用（兼容实现）
pub fn sys_setsockopt(
    fd: usize,
    level: i32,
    _optname: i32,
    _optval: *const u8,
    _optlen: usize,
) -> SyscallResult {
    const ENOTSOCK: isize = -88;
    const ENOPROTOOPT: isize = -92;
    const SOL_SOCKET: i32 = 1;
    const IPPROTO_IP: i32 = 0;
    const IPPROTO_TCP: i32 = 6;
    const IPPROTO_UDP: i32 = 17;

    let process = current_process();
    let pid = process.getpid();
    let mut manager = SOCKET_MANAGER.lock();
    let Some(sock) = manager.get_socket_mut(fd, pid) else {
        return Err(SysError::ENOTSOCK);
    };
    if sock.is_closed() {
        return Err(SysError::ENOTSOCK);
    }

    match level {
        SOL_SOCKET | IPPROTO_IP | IPPROTO_TCP | IPPROTO_UDP => Ok(0),
        _ => Err(SysError::ENOPROTOOPT),
    }
}
#[allow(unused)]
/// getsockopt() 系统调用（兼容实现）
pub fn sys_getsockopt(
    fd: usize,
    level: i32,
    optname: i32,
    optval: *mut u8,
    optlen: *mut usize,
) -> SyscallResult {
    const ENOTSOCK: isize = -88;
    const EFAULT: isize = -14;
    const EINVAL: isize = -22;
    const ENOPROTOOPT: isize = -92;

    const SOL_SOCKET: i32 = 1;
    const SO_TYPE: i32 = 3;
    const SO_ERROR: i32 = 4;
    const SO_SNDBUF: i32 = 7;
    const SO_RCVBUF: i32 = 8;
    const SO_DOMAIN: i32 = 39;
    const SO_PROTOCOL: i32 = 38;

    const AF_UNIX: i32 = 1;
    const AF_INET: i32 = 2;
    const SOCK_STREAM: i32 = 1;
    const SOCK_DGRAM: i32 = 2;
    const SOCK_RAW: i32 = 3;

    if optval.is_null() || optlen.is_null() {
        return Err(SysError::EFAULT);
    }

    let process = current_process();
    let pid = process.getpid();
    let mut manager = SOCKET_MANAGER.lock();
    let Some(sock) = manager.get_socket_mut(fd, pid) else {
        return Err(SysError::ENOTSOCK);
    };
    if sock.is_closed() {
        return Err(SysError::ENOTSOCK);
    }

    let value: i32 = match (level, optname) {
        (SOL_SOCKET, SO_ERROR) => 0,
        (SOL_SOCKET, SO_SNDBUF) => 212_992,
        (SOL_SOCKET, SO_RCVBUF) => 212_992,
        (SOL_SOCKET, SO_DOMAIN) => match &sock.inner {
            SocketInner::Unix(_) => AF_UNIX,
            _ => AF_INET,
        },
        (SOL_SOCKET, SO_TYPE) => match &sock.inner {
            SocketInner::Tcp(_) => SOCK_STREAM,
            SocketInner::Udp(_) => SOCK_DGRAM,
            SocketInner::Raw(_) => SOCK_RAW,
            SocketInner::Unix(unix) => unix.sock_type,
        },
        (SOL_SOCKET, SO_PROTOCOL) => match &sock.inner {
            SocketInner::Tcp(_) => 6,
            SocketInner::Udp(_) => 17,
            SocketInner::Raw(raw) => raw.lock().protocol() as i32,
            SocketInner::Unix(unix) => unix.protocol,
        },
        _ => return Err(SysError::ENOPROTOOPT),
    };

    let user_len = unsafe { *optlen };
    if user_len == 0 {
        return Err(SysError::EINVAL);
    }

    let src = &value as *const i32 as *const u8;
    let copy_len = user_len.min(mem::size_of::<i32>());
    unsafe {
        ptr::copy_nonoverlapping(src, optval, copy_len);
        *optlen = copy_len;
    }
    Ok(0)
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
            sin_addr: addr.to_be(),
            sin_zero: [0; 8],
        }
    }

    /// 创建回环地址结构
    pub fn loopback(port: u16) -> Self {
        Self::new(0x7F000001, port)
    }
}
