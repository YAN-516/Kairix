// src/syscall/mod.rs

use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::find_superblock_by_path;
use crate::fs::vfs::inode::InodeMode;
use crate::fs::vfs::path::{AT_FDCWD, get_start_dentry, resolve_path, split_parent_and_name};
use crate::mm::{
    UserBuffer, translated_byte_buffer, translated_byte_buffer_for_write, translated_ref,
    translated_refmut,
};
use crate::net::route::route_lookup;
use crate::net::skb::Skb;
use crate::socket::SOCKET_MANAGER;
use crate::socket::raw::{self, RawSocket, register_raw_socket, send_raw_packet};
use crate::socket::tcp::{self, TcpSocket};
use crate::socket::udp::{UdpSocket, register_udp_socket, send_udp_packet};
use crate::socket::{Socket, SocketFile, SocketInner, SocketState, UnixSocket};
use crate::syscall::landlock::{
    LANDLOCK_ACCESS_FS_MAKE_SOCK, LANDLOCK_ACCESS_NET_BIND_TCP, LANDLOCK_ACCESS_NET_CONNECT_TCP,
    landlock_can_connect_abstract_unix, landlock_check_dentry, landlock_check_net_port,
};
use crate::task::*;
use crate::trap::_set_sum_bit;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::mem;
use core::ptr;
use log::{error, info};
use spin::Mutex;
use spin::MutexGuard;

lazy_static::lazy_static! {
    static ref ABSTRACT_UNIX_SOCKETS: Mutex<Vec<(String, usize)>> = Mutex::new(Vec::new());
}

const O_NONBLOCK: u32 = 0o4000;
const MSG_DONTWAIT: i32 = 0x40;

fn socket_no_wait(sock_flags: u32, msg_flags: i32) -> bool {
    (sock_flags & O_NONBLOCK) != 0 || (msg_flags & MSG_DONTWAIT) != 0
}

fn socket_deadline(timeout_us: Option<usize>) -> Option<usize> {
    timeout_us.map(|timeout| crate::timer::get_time_us().saturating_add(timeout))
}

fn socket_deadline_expired(deadline: Option<usize>) -> bool {
    deadline.is_some_and(|deadline| crate::timer::get_time_us() >= deadline)
}

fn socket_timeout_from_user(optval: *const u8, optlen: usize) -> SysResult<Option<usize>> {
    if optval.is_null() || optlen == 0 {
        return Err(SysError::EFAULT);
    }
    if optlen < 8 {
        return Err(SysError::EINVAL);
    }

    let raw = read_user_bytes_flat(current_user_token(), optval, optlen.min(16))?;
    let (sec, usec) = if raw.len() >= 16 {
        let mut sec = [0u8; 8];
        let mut usec = [0u8; 8];
        sec.copy_from_slice(&raw[..8]);
        usec.copy_from_slice(&raw[8..16]);
        (i64::from_ne_bytes(sec), i64::from_ne_bytes(usec))
    } else {
        let mut sec = [0u8; 4];
        let mut usec = [0u8; 4];
        sec.copy_from_slice(&raw[..4]);
        usec.copy_from_slice(&raw[4..8]);
        (i32::from_ne_bytes(sec) as i64, i32::from_ne_bytes(usec) as i64)
    };

    if sec < 0 || usec < 0 {
        return Err(SysError::EINVAL);
    }
    if sec == 0 && usec == 0 {
        return Ok(None);
    }

    let total = (sec as u128)
        .saturating_mul(1_000_000)
        .saturating_add(usec as u128)
        .min(usize::MAX as u128) as usize;
    Ok(Some(total.max(1)))
}

fn write_socket_timeout(
    optval: *mut u8,
    optlen: *mut u32,
    timeout_us: Option<usize>,
) -> SyscallResult {
    if optval.is_null() || optlen.is_null() {
        return Err(SysError::EFAULT);
    }
    let user_len = unsafe { *optlen as usize };
    if user_len == 0 {
        return Err(SysError::EINVAL);
    }

    let timeout = timeout_us.unwrap_or(0);
    let sec = (timeout / 1_000_000) as i64;
    let usec = (timeout % 1_000_000) as i64;
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&sec.to_ne_bytes());
    out[8..].copy_from_slice(&usec.to_ne_bytes());
    let copy_len = user_len.min(out.len());
    unsafe {
        ptr::copy_nonoverlapping(out.as_ptr(), optval, copy_len);
        *optlen = copy_len as u32;
    }
    Ok(0)
}

enum UnixSockaddr {
    Abstract(String),
    Pathname(String),
}
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
            2 if protocol == 1 => {
                let raw = Arc::new(Mutex::new(RawSocket::new(protocol as u8)));
                register_raw_socket(protocol as u8, raw.clone());
                SocketInner::Raw(raw)
            }
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

    error!(
        "sys_socket: created socket fd={}, type={}, protocol={}",
        fd, type_, protocol
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
    const AF_UNIX: u16 = 1;
    if addr_ptr.is_null() {
        return Err(SysError::EFAULT);
    }

    // 检查地址长度
    if addr_len >= core::mem::size_of::<u16>() {
        let sa_family = unsafe { *(addr_ptr as *const u16) };
        if sa_family == AF_UNIX {
            let process = current_process();
            let pid = process.getpid();
            let unix_addr = read_unix_sockaddr(addr_ptr, addr_len)?;
            match unix_addr {
                UnixSockaddr::Abstract(name) => {
                    let mut manager = SOCKET_MANAGER.lock();
                    let socket = manager.get_socket_mut(fd, pid).ok_or(SysError::EBADF)?;
                    if socket.is_closed() || socket.state != SocketState::Open {
                        return Err(SysError::EINVAL);
                    }
                    match &mut socket.inner {
                        SocketInner::Unix(unix) => {
                            unix.abstract_name = Some(name.clone());
                            socket.state = SocketState::Bound;
                            register_abstract_unix_socket(name, pid);
                            return Ok(0);
                        }
                        _ => return Err(SysError::EINVAL),
                    }
                }
                UnixSockaddr::Pathname(path) => {
                    {
                        let mut manager = SOCKET_MANAGER.lock();
                        let socket = manager.get_socket_mut(fd, pid).ok_or(SysError::EBADF)?;
                        if socket.is_closed() || socket.state != SocketState::Open {
                            return Err(SysError::EINVAL);
                        }
                        if !matches!(&socket.inner, SocketInner::Unix(_)) {
                            return Err(SysError::EINVAL);
                        }
                    }
                    bind_pathname_unix_socket(&path)?;
                    let mut manager = SOCKET_MANAGER.lock();
                    let socket = manager.get_socket_mut(fd, pid).ok_or(SysError::EBADF)?;
                    match &mut socket.inner {
                        SocketInner::Unix(unix) => {
                            unix.abstract_name = None;
                            socket.state = SocketState::Bound;
                            return Ok(0);
                        }
                        _ => return Err(SysError::EINVAL),
                    }
                }
            }
        }
    }
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
            landlock_check_net_port(port, LANDLOCK_ACCESS_NET_BIND_TCP)?;
            tcp_socket.lock().bind(addr, port)?;
            socket.state = SocketState::Bound;
            error!("sys_bind: TCP socket fd={} bound to {}:{}", fd, addr, port);
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
            error!("sys_bind: UDP socket fd={} bound to {}:{}", fd, addr, port);
        }
        SocketInner::Raw(_) => {
            // 原始套接字不需要绑定
            socket.state = SocketState::Bound;
            log::info!("sys_bind: Raw socket fd={} (no actual bind needed)", fd);
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

    let process = current_process();
    let pid = process.getpid();
    if SOCKET_MANAGER.lock().get_socket(fd, pid).is_none() {
        let file = unmanaged_socket_file(&process, fd)?;
        let user_buf = UserBuffer::new(translated_byte_buffer(current_user_token(), buf_ptr, len)?);
        return file.write(user_buf);
    }

    // 读取数据
    // println!("{:?}", len);
    let data = if len > 0 {
        unsafe { core::slice::from_raw_parts(buf_ptr, len) }
    } else {
        &[]
    };
    let explicit_dst = if addr_ptr.is_null() {
        None
    } else {
        if addr_len < mem::size_of::<SockaddrIn>() {
            return Err(SysError::EINVAL);
        }
        let sockaddr = unsafe { &*(addr_ptr as *const SockaddrIn) };
        if sockaddr.sin_family != 2 {
            return Err(SysError::EINVAL);
        }
        Some((
            u32::from_be(sockaddr.sin_addr),
            u16::from_be(sockaddr.sin_port),
        ))
    };
    // 获取套接字
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
        let (dst_addr, dst_port) = if let Some(dst) = explicit_dst {
            dst
        } else {
            udp.lock().remote_addr().ok_or(SysError::ENOTCONN)?
        };
        error!(
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
        error!(
            "sys_sendto: UDP socket fd={} sent {} bytes to {}:{}",
            fd, len, dst_addr, dst_port
        );
        Ok(len)
    } else if let Some(tcp) = tcp_socket {
        let (dst_addr, dst_port) = explicit_dst.unwrap_or((0, 0));
        log::info!(
            "sys_sendto: preparing to send TCP packet from fd={} to {}:{}",
            fd,
            dst_addr,
            dst_port
        );

        if data.is_empty() {
            return Ok(0);
        }

        {
            let tcp_guard = tcp.lock();
            let Some((remote_ip, remote_port)) = tcp_guard.remote_addr else {
                log::info!("sys_sendto: TCP socket not connected fd={}", fd);
                return Err(SysError::EINVAL);
            };

            let check_dst = if addr_ptr.is_null() {
                remote_ip
            } else {
                dst_addr
            };
            if check_dst != 0
                && (check_dst != remote_ip || (dst_port != 0 && dst_port != remote_port))
            {
                log::info!("sys_sendto: TCP destination mismatch fd={}", fd);
                return Err(SysError::EINVAL);
            }
        }

        let sent = match tcp::send_tracked(tcp.clone(), data) {
            Ok(sent) => sent,
            Err(e) => {
                log::info!("sys_sendto: TCP send failed fd={} err={:?}", fd, e);
                return Err(SysError::EINVAL);
            }
        };

        error!("sys_sendto: TCP socket fd={} sent {} bytes", fd, sent);
        Ok(sent)
    } else if let Some(raw) = raw_socket {
        let (dst_addr, _) = explicit_dst.unwrap_or((0x7F000001, 0));
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
        log::info!(
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
    flags: i32,
    addr_ptr: *mut u8,
    addr_len: *mut u32,
) -> SyscallResult {
    _set_sum_bit();
    // println!("enter sys recvfrom...");

    // 检查参数
    if buf_ptr.is_null() && len > 0 {
        return Err(SysError::EINVAL);
    }
    if len == 0 {
        return Ok(0);
    }

    let buf = if len > 0 {
        unsafe { core::slice::from_raw_parts_mut(buf_ptr, len) }
    } else {
        &mut []
    };

    let process = current_process();
    let pid = process.getpid();
    if SOCKET_MANAGER.lock().get_socket(fd, pid).is_none() {
        let file = unmanaged_socket_file(&process, fd)?;
        let user_buf = UserBuffer::new(translated_byte_buffer_for_write(
            current_user_token(),
            buf_ptr,
            len,
        )?);
        let recv_len = file.read(user_buf)?;
        if !addr_len.is_null() {
            unsafe {
                *addr_len = 0;
            }
        }
        return Ok(recv_len);
    }
    let mut manager = SOCKET_MANAGER.lock();
    let (udp_socket, raw_socket, tcp_socket, no_wait, deadline) =
        if let Some(sock) = manager.get_socket_mut(fd, pid) {
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
            (
                udp_socket,
                raw_socket,
                tcp_socket,
                socket_no_wait(sock.flags, flags),
                socket_deadline(sock.recv_timeout_us),
            )
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
                            *addr_len = mem::size_of::<SockaddrIn>() as u32;
                        }
                    }

                    error!(
                        "sys_recvfrom: UDP socket fd={} received {} bytes from {}:{}",
                        fd, recv_len, src_addr, src_port
                    );
                    break recv_len;
                }
                Err(_) => {
                    drop(udp_guard);
                    if no_wait || socket_deadline_expired(deadline) {
                        return Err(SysError::EAGAIN);
                    }
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
                            *addr_len = mem::size_of::<SockaddrIn>() as u32;
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
                    ) {
                        error!(
                            "sys_recvfrom: TCP socket fd={} connection closed during recv",
                            fd
                        );
                        break 0; // EOF
                    }
                    if no_wait || socket_deadline_expired(deadline) {
                        return Err(SysError::EAGAIN);
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
        let (recv_len, src_addr) = loop {
            let raw_guard = raw.lock();
            match raw_guard.recv_from(buf) {
                Ok(v) => break v,
                Err(_) => {
                    drop(raw_guard);
                    if raw_recv_should_interrupt(&process) {
                        return Err(SysError::EINTR);
                    }
                    if no_wait || socket_deadline_expired(deadline) {
                        return Err(SysError::EAGAIN);
                    }
                    // RAW 套接字在等待期间也需要主动轮询设备 RX，
                    // 否则 echo reply 可能已到达链路层但长期不被上送。
                    crate::net::poll_rx_all();
                    suspend_current_and_run_next();
                    if raw_recv_should_interrupt(&process) {
                        return Err(SysError::EINTR);
                    }
                }
            }
        };

        // 原始套接字也填充源地址（如果有）
        if !addr_ptr.is_null() && !addr_len.is_null() {
            unsafe {
                let sockaddr = addr_ptr as *mut SockaddrIn;
                (*sockaddr).sin_family = 2;
                (*sockaddr).sin_port = 0;
                (*sockaddr).sin_addr = src_addr.to_be();
                (*sockaddr).sin_zero = [0; 8];
                *addr_len = mem::size_of::<SockaddrIn>() as u32;
            }
        }

        log::info!(
            "sys_recvfrom: Raw socket fd={} received {} bytes",
            fd,
            recv_len
        );
        Ok(recv_len)
    } else {
        Err(SysError::ENOTSOCK)
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct UserIovec {
    base: usize,
    len: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct UserMsghdr {
    msg_name: usize,
    msg_namelen: u32,
    __pad1: u32,
    msg_iov: usize,
    msg_iovlen: usize,
    msg_control: usize,
    msg_controllen: usize,
    msg_flags: i32,
    __pad2: i32,
}

const MSG_IOV_MAX: usize = 1024;

fn read_user_bytes_flat(token: usize, ptr: *const u8, len: usize) -> SysResult<Vec<u8>> {
    if len == 0 {
        return Ok(Vec::new());
    }
    if ptr.is_null() {
        return Err(SysError::EFAULT);
    }
    let mut out = Vec::with_capacity(len);
    for part in translated_byte_buffer(token, ptr, len)? {
        out.extend_from_slice(part);
    }
    Ok(out)
}

fn read_user_iovecs(token: usize, iov_ptr: usize, iovlen: usize) -> SysResult<Vec<UserIovec>> {
    if iovlen > MSG_IOV_MAX {
        return Err(SysError::EINVAL);
    }
    if iovlen == 0 {
        return Ok(Vec::new());
    }
    if iov_ptr == 0 {
        return Err(SysError::EFAULT);
    }

    let elem_size = mem::size_of::<UserIovec>();
    let bytes_len = iovlen.checked_mul(elem_size).ok_or(SysError::EINVAL)?;
    let raw = read_user_bytes_flat(token, iov_ptr as *const u8, bytes_len)?;
    let mut iovs = Vec::with_capacity(iovlen);
    for chunk in raw.chunks_exact(elem_size) {
        let mut base_bytes = [0u8; mem::size_of::<usize>()];
        let mut len_bytes = [0u8; mem::size_of::<usize>()];
        base_bytes.copy_from_slice(&chunk[..mem::size_of::<usize>()]);
        len_bytes.copy_from_slice(&chunk[mem::size_of::<usize>()..elem_size]);
        iovs.push(UserIovec {
            base: usize::from_ne_bytes(base_bytes),
            len: usize::from_ne_bytes(len_bytes),
        });
    }
    Ok(iovs)
}

fn user_iov_total_len(iovs: &[UserIovec]) -> SysResult<usize> {
    let mut total = 0usize;
    for iov in iovs {
        total = total.checked_add(iov.len).ok_or(SysError::EINVAL)?;
    }
    Ok(total)
}

fn copy_iovs_from_user(token: usize, iovs: &[UserIovec]) -> SysResult<Vec<u8>> {
    let total = user_iov_total_len(iovs)?;
    let mut out = Vec::with_capacity(total);
    for iov in iovs {
        if iov.len == 0 {
            continue;
        }
        if iov.base == 0 {
            return Err(SysError::EFAULT);
        }
        for part in translated_byte_buffer(token, iov.base as *const u8, iov.len)? {
            out.extend_from_slice(part);
        }
    }
    Ok(out)
}

fn copy_iovs_to_user(token: usize, iovs: &[UserIovec], data: &[u8]) -> SysResult<()> {
    let mut copied = 0usize;
    for iov in iovs {
        if copied >= data.len() {
            break;
        }
        if iov.len == 0 {
            continue;
        }
        if iov.base == 0 {
            return Err(SysError::EFAULT);
        }
        let to_copy = core::cmp::min(iov.len, data.len() - copied);
        for part in translated_byte_buffer_for_write(token, iov.base as *mut u8, to_copy)? {
            let end = copied + part.len();
            part.copy_from_slice(&data[copied..end]);
            copied = end;
        }
    }
    Ok(())
}

/// sendmsg() system call.
pub fn sys_sendmsg(fd: usize, msg_ptr: usize, flags: i32) -> SyscallResult {
    if msg_ptr == 0 {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let msg = *translated_ref(token, msg_ptr as *const UserMsghdr)?;
    let iovs = read_user_iovecs(token, msg.msg_iov, msg.msg_iovlen)?;
    let data = copy_iovs_from_user(token, &iovs)?;
    let addr_ptr = if msg.msg_name == 0 {
        core::ptr::null()
    } else {
        msg.msg_name as *const u8
    };
    sys_sendto(
        fd,
        if data.is_empty() {
            core::ptr::null()
        } else {
            data.as_ptr()
        },
        data.len(),
        flags,
        addr_ptr,
        msg.msg_namelen as usize,
    )
}

/// recvmsg() system call.
pub fn sys_recvmsg(fd: usize, msg_ptr: usize, flags: i32) -> SyscallResult {
    if msg_ptr == 0 {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let msg = *translated_ref(token, msg_ptr as *const UserMsghdr)?;
    let iovs = read_user_iovecs(token, msg.msg_iov, msg.msg_iovlen)?;
    let total = user_iov_total_len(&iovs)?;
    if total == 0 {
        let msg_out = translated_refmut(token, msg_ptr as *mut UserMsghdr)?;
        msg_out.msg_flags = 0;
        return Ok(0);
    }

    let mut data = Vec::new();
    data.resize(total, 0);
    let mut name_len = msg.msg_namelen;
    let addr_ptr = if msg.msg_name == 0 || msg.msg_namelen == 0 {
        core::ptr::null_mut()
    } else {
        msg.msg_name as *mut u8
    };
    let addr_len_ptr = if addr_ptr.is_null() {
        core::ptr::null_mut()
    } else {
        &mut name_len as *mut u32
    };
    let n = sys_recvfrom(
        fd,
        data.as_mut_ptr(),
        data.len(),
        flags,
        addr_ptr,
        addr_len_ptr,
    )?;
    copy_iovs_to_user(token, &iovs, &data[..n])?;

    let msg_out = translated_refmut(token, msg_ptr as *mut UserMsghdr)?;
    if !addr_len_ptr.is_null() {
        msg_out.msg_namelen = name_len;
    }
    msg_out.msg_flags = 0;
    Ok(n)
}

/// close() 系统调用（socket 专用路径）
pub fn sys_close_socket(fd: usize) -> SyscallResult {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
        return Err(SysError::EBADF);
    }
    if fd < inner.fd_flags.len() {
        inner.fd_flags[fd] = 0;
    }
    inner.fd_table[fd] = None;
    drop(inner);
    let pid = process.getpid();
    let mut manager = SOCKET_MANAGER.lock();

    if let Some(_sock) = manager.get_socket_mut(fd, pid) {
        manager
            .close_socket_with_refcount(fd, pid)
            .map_err(|_| SysError::EBADF)?;
    } else {
        return Err(SysError::EBADF);
    }

    error!("sys_close: closed socket fd={}", fd);
    Ok(0)
}

/// shutdown() 系统调用
///
/// # 参数
/// - `fd`: 文件描述符
/// - `how`: 关闭方式 (SHUT_RD=0, SHUT_WR=1, SHUT_RDWR=2)
pub fn sys_shutdown(fd: usize, how: i32) -> SyscallResult {
    error!("enter sys shutdown...");
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
        drop(manager);
        let _ = unmanaged_socket_file(&process, fd)?;
        return Ok(0);
    };
    if sock.is_closed() {
        return Err(SysError::ENOTSOCK);
    }

    match sock.shutdown(how) {
        Ok(_) => {
            error!("finish sys shutdown...");
            return Ok(0);
        }
        Err(_) => {
            error!("Failed to shutdown socket fd={}", fd);
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
            SocketInner::Unix(_) => return Ok(0),
            _ => return Err(SysError::ENOTSOCK),
        }
    };
    tcp_socket.lock().state = tcp::TcpSocketState::Listening;

    error!(
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
        let name = read_abstract_unix_name(addr_ptr, addr_len)?;
        let server_pid = lookup_abstract_unix_socket(&name).ok_or(SysError::ENOENT)?;
        if !landlock_can_connect_abstract_unix(server_pid) {
            return Err(SysError::EPERM);
        }
        let mut manager = SOCKET_MANAGER.lock();
        let Some(sock) = manager.get_socket_mut(fd, pid) else {
            return Err(SysError::ENOTSOCK);
        };
        return match sock.inner {
            SocketInner::Unix(ref mut unix) => {
                unix.peer_pid = Some(server_pid);
                sock.state = SocketState::Bound;
                Ok(0)
            }
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
    let port = u16::from_be(sockaddr.sin_port);
    landlock_check_net_port(port, LANDLOCK_ACCESS_NET_CONNECT_TCP)?;
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
            let ret = tcp::connect_nonblock(tcp_socket, u32::from_be(sockaddr.sin_addr), port);
            match ret {
                Ok(()) => Ok(0),
                Err(e) => Err(e),
            }
        } else {
            let tcp_connect_result =
                tcp::connect(tcp_socket, u32::from_be(sockaddr.sin_addr), port);
            match tcp_connect_result {
                Ok(_) => Ok(0),
                Err(e) => Err(e),
            }
        }
    } else if let Some(udp_socket) = udp_socket {
        let mut udp = udp_socket.lock();
        let old_local = udp.local_addr();
        match udp.connect(u32::from_be(sockaddr.sin_addr), port) {
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
fn write_sockaddr(addr_ptr: *mut u8, addr_len: *mut u32, ip: u32, port: u16) -> SyscallResult {
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

    let cap = unsafe { *addr_len as usize };
    let full = mem::size_of::<SockaddrIn>();
    let copy_len = cap.min(full);
    unsafe {
        ptr::copy_nonoverlapping(&out as *const SockaddrIn as *const u8, addr_ptr, copy_len);
        *addr_len = full as u32;
    }
    Ok(0)
}
#[allow(unused)]
/// getsockname() 系统调用
pub fn sys_getsockname(fd: usize, addr_ptr: *mut u8, addr_len: *mut u32) -> SyscallResult {
    const ENOTSOCK: isize = -88;

    let process = current_process();
    let pid = process.getpid();
    let mut manager = SOCKET_MANAGER.lock();
    let Some(sock) = manager.get_socket_mut(fd, pid) else {
        drop(manager);
        let _ = unmanaged_socket_file(&process, fd)?;
        return write_sockaddr(addr_ptr, addr_len, 0, 0);
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
pub fn sys_getpeername(fd: usize, addr_ptr: *mut u8, addr_len: *mut u32) -> SyscallResult {
    const ENOTSOCK: isize = -88;
    const ENOTCONN: isize = -107;

    let process = current_process();
    let pid = process.getpid();
    let mut manager = SOCKET_MANAGER.lock();
    let Some(sock) = manager.get_socket_mut(fd, pid) else {
        drop(manager);
        let _ = unmanaged_socket_file(&process, fd)?;
        return write_sockaddr(addr_ptr, addr_len, 0, 0);
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
pub fn sys_accept(fd: usize, addr_ptr: *mut u8, addr_len: *mut u32) -> SyscallResult {
    _set_sum_bit();
    let process = current_process();
    let pid = process.getpid();
    let (tcp_socket, listener_flags, recv_timeout_us, send_timeout_us) = {
        let mut manager = SOCKET_MANAGER.lock();
        let Some(sock) = manager.get_socket_mut(fd, pid) else {
            let fd_file = {
                let inner = process.inner_exclusive_access();
                inner
                    .fd_table
                    .get(fd)
                    .and_then(|file| file.as_ref().cloned())
            };
            return match fd_file {
                Some(file) if file.is_path_only() || file.is_open_tree_fd() => Err(SysError::EBADF),
                Some(_) => Err(SysError::ENOTSOCK),
                None => Err(SysError::EBADF),
            };
        };
        match &mut sock.inner {
            SocketInner::Tcp(tcp_socket) => (
                tcp_socket.clone(),
                sock.flags,
                sock.recv_timeout_us,
                sock.send_timeout_us,
            ),
            _ => return Err(SysError::ENOTSOCK),
        }
    };
    let no_wait = (listener_flags & O_NONBLOCK) != 0;
    let deadline = socket_deadline(recv_timeout_us);

    let child = loop {
        if let Some(child) = tcp::accept(tcp_socket.clone()) {
            break child;
        }
        if no_wait || socket_deadline_expired(deadline) {
            return Err(SysError::EAGAIN);
        }

        if accept_itimer_expired(&process) {
            tcp_socket.lock().accept_waker.lock().take();
            return Err(SysError::EINTR);
        }
        if crate::syscall::signal::should_interrupt_syscall() {
            if let Some(task) = current_task() {
                task.inner_exclusive_access().interrupted_by_signal = false;
            }
            tcp_socket.lock().accept_waker.lock().take();
            return Err(SysError::EINTR);
        }

        // // // 检查 socket 是否仍然有效
        // // {
        // //     let sock_guard = tcp_socket.lock();
        // //     if sock_guard.state == tcp::TcpSocketState::Listening {
        // //         error!("sys_accept: socket no longer listening");
        // //         return -1;
        // //     }
        // // }

        // 注册 waker
        {
            let sock_guard = tcp_socket.lock();
            let mut waker_guard = sock_guard.accept_waker.lock();
            let waker = task_waker_front(current_task().unwrap());
            *waker_guard = Some(waker);
        }
        if crate::syscall::signal::should_interrupt_syscall() {
            if let Some(task) = current_task() {
                task.inner_exclusive_access().interrupted_by_signal = false;
            }
            tcp_socket.lock().accept_waker.lock().take();
            return Err(SysError::EINTR);
        }

        suspend_current_and_run_next();

        if accept_itimer_expired(&process) {
            tcp_socket.lock().accept_waker.lock().take();
            return Err(SysError::EINTR);
        }
        if socket_deadline_expired(deadline) {
            tcp_socket.lock().accept_waker.lock().take();
            return Err(SysError::EAGAIN);
        }
        if let Some(task) = current_task() {
            let mut task_inner = task.inner_exclusive_access();
            if task_inner.interrupted_by_signal {
                task_inner.interrupted_by_signal = false;
                tcp_socket.lock().accept_waker.lock().take();
                return Err(SysError::EINTR);
            }
        }
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

    let mut socket = Socket::new(SocketInner::Tcp(child.clone()), fd_new, pid);
    socket.flags = listener_flags;
    socket.recv_timeout_us = recv_timeout_us;
    socket.send_timeout_us = send_timeout_us;
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
                *addr_len = mem::size_of::<SockaddrIn>() as u32;
            }
        }
    }

    Ok(fd_new)
}

fn accept_itimer_expired(process: &Arc<ProcessControlBlock>) -> bool {
    let now = crate::timer::get_time();
    let mut inner = process.inner_exclusive_access();
    if !inner
        .itimer_real_deadline
        .map_or(false, |deadline| now >= deadline)
    {
        return false;
    }

    if let Some(interval) = inner.itimer_real_interval {
        let deadline = inner.itimer_real_deadline.unwrap_or(now);
        inner.itimer_real_deadline = Some(deadline.saturating_add(interval));
    } else {
        inner.itimer_real_deadline = None;
    }
    true
}

fn raw_recv_should_interrupt(process: &Arc<ProcessControlBlock>) -> bool {
    if consume_itimer_real(process) {
        crate::syscall::signal::deliver_signal(process, crate::task::signal::Signal::SigAlrm);
    }
    if let Some(task) = current_task() {
        let mut task_inner = task.inner_exclusive_access();
        if task_inner.interrupted_by_signal {
            task_inner.interrupted_by_signal = false;
            return true;
        }
    }
    current_process().inner_exclusive_access().is_zombie
        || crate::syscall::signal::should_interrupt_syscall()
}

fn consume_itimer_real(process: &Arc<ProcessControlBlock>) -> bool {
    let now = crate::timer::get_time();
    let mut inner = process.inner_exclusive_access();
    if !inner
        .itimer_real_deadline
        .map_or(false, |deadline| now >= deadline)
    {
        return false;
    }

    if let Some(interval) = inner.itimer_real_interval {
        let deadline = inner.itimer_real_deadline.unwrap_or(now);
        inner.itimer_real_deadline = Some(deadline.saturating_add(interval));
    } else {
        inner.itimer_real_deadline = None;
    }
    true
}

fn unmanaged_socket_file(
    process: &Arc<ProcessControlBlock>,
    fd: usize,
) -> SysResult<Arc<dyn crate::fs::File + Send + Sync>> {
    let file = {
        let inner = process.inner_exclusive_access();
        inner
            .fd_table
            .get(fd)
            .and_then(|file| file.as_ref().cloned())
    };
    match file {
        Some(file) if file.is_socket() => Ok(file),
        Some(_) => Err(SysError::ENOTSOCK),
        None => Err(SysError::EBADF),
    }
}

#[allow(unused)]
/// setsockopt() 系统调用（兼容实现）
pub fn sys_setsockopt(
    fd: usize,
    level: i32,
    optname: i32,
    optval: *const u8,
    optlen: usize,
) -> SyscallResult {
    const SOL_SOCKET: i32 = 1;
    const IPPROTO_IP: i32 = 0;
    const IPPROTO_TCP: i32 = 6;
    const IPPROTO_UDP: i32 = 17;

    const SO_REUSEADDR: i32 = 2;
    const SO_KEEPALIVE: i32 = 9;
    const SO_BROADCAST: i32 = 6;
    const SO_LINGER: i32 = 13;
    const SO_RCVTIMEO_OLD: i32 = 20;
    const SO_SNDTIMEO_OLD: i32 = 21;
    const SO_RCVTIMEO_NEW: i32 = 66;
    const SO_SNDTIMEO_NEW: i32 = 67;
    const SO_SNDBUF: i32 = 7;
    const SO_RCVBUF: i32 = 8;
    const TCP_NODELAY: i32 = 1;

    if optlen > 0 {
        if optval.is_null() {
            return Err(SysError::EFAULT);
        }
        let _ = translated_byte_buffer(current_user_token(), optval, optlen)?;
    }

    let timeout = if level == SOL_SOCKET
        && matches!(
            optname,
            SO_RCVTIMEO_OLD | SO_SNDTIMEO_OLD | SO_RCVTIMEO_NEW | SO_SNDTIMEO_NEW
        )
    {
        Some(socket_timeout_from_user(optval, optlen)?)
    } else {
        None
    };

    let process = current_process();
    let pid = process.getpid();
    let mut manager = SOCKET_MANAGER.lock();
    let Some(sock) = manager.get_socket_mut(fd, pid) else {
        drop(manager);
        let _ = unmanaged_socket_file(&process, fd)?;
        return match level {
            SOL_SOCKET => Ok(0),
            _ => Err(SysError::ENOPROTOOPT),
        };
    };
    if sock.is_closed() {
        return Err(SysError::ENOTSOCK);
    }

    match level {
        SOL_SOCKET => match optname {
            SO_RCVTIMEO_OLD | SO_RCVTIMEO_NEW => {
                sock.recv_timeout_us = timeout.unwrap_or(None);
                Ok(0)
            }
            SO_SNDTIMEO_OLD | SO_SNDTIMEO_NEW => {
                sock.send_timeout_us = timeout.unwrap_or(None);
                Ok(0)
            }
            SO_REUSEADDR | SO_KEEPALIVE | SO_BROADCAST | SO_LINGER | SO_SNDBUF | SO_RCVBUF => Ok(0),
            _ => Ok(0),
        },
        IPPROTO_TCP => match optname {
            TCP_NODELAY | 4 | 5 | 6 => Ok(0),
            _ => Ok(0),
        },
        IPPROTO_IP | IPPROTO_UDP => Ok(0),
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
    optlen: *mut u32,
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
    const SO_KEEPALIVE: i32 = 9;
    const SO_RCVTIMEO_OLD: i32 = 20;
    const SO_SNDTIMEO_OLD: i32 = 21;
    const SO_RCVTIMEO_NEW: i32 = 66;
    const SO_SNDTIMEO_NEW: i32 = 67;
    const IPPROTO_TCP: i32 = 6;
    const TCP_NODELAY: i32 = 1;

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
        drop(manager);
        let _ = unmanaged_socket_file(&process, fd)?;
        let value: i32 = match (level, optname) {
            (SOL_SOCKET, SO_ERROR) => 0,
            (SOL_SOCKET, SO_SNDBUF) => 212_992,
            (SOL_SOCKET, SO_RCVBUF) => 212_992,
            (SOL_SOCKET, SO_DOMAIN) => AF_UNIX,
            (SOL_SOCKET, SO_TYPE) => SOCK_STREAM,
            (SOL_SOCKET, SO_PROTOCOL) => 0,
            _ => return Err(SysError::ENOPROTOOPT),
        };
        let user_len = unsafe { *optlen as usize };
        if user_len == 0 {
            return Err(SysError::EINVAL);
        }
        let src = &value as *const i32 as *const u8;
        let copy_len = user_len.min(mem::size_of::<i32>());
        unsafe {
            ptr::copy_nonoverlapping(src, optval, copy_len);
            *optlen = copy_len as u32;
        }
        return Ok(0);
    };
    if sock.is_closed() {
        return Err(SysError::ENOTSOCK);
    }

    if level == SOL_SOCKET {
        match optname {
            SO_RCVTIMEO_OLD | SO_RCVTIMEO_NEW => {
                return write_socket_timeout(optval, optlen, sock.recv_timeout_us);
            }
            SO_SNDTIMEO_OLD | SO_SNDTIMEO_NEW => {
                return write_socket_timeout(optval, optlen, sock.send_timeout_us);
            }
            _ => {}
        }
    }

    let value: i32 = match (level, optname) {
        (SOL_SOCKET, SO_ERROR) => match &sock.inner {
            SocketInner::Tcp(tcp) => tcp.lock().take_error().map(|e| e as i32).unwrap_or(0),
            _ => 0,
        },
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
        (SOL_SOCKET, SO_KEEPALIVE) => 0,
        (IPPROTO_TCP, TCP_NODELAY) => 0,
        _ => return Err(SysError::ENOPROTOOPT),
    };

    let user_len = unsafe { *optlen as usize };
    if user_len == 0 {
        return Err(SysError::EINVAL);
    }

    let src = &value as *const i32 as *const u8;
    let copy_len = user_len.min(mem::size_of::<i32>());
    unsafe {
        ptr::copy_nonoverlapping(src, optval, copy_len);
        *optlen = copy_len as u32;
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

fn read_abstract_unix_name(addr_ptr: *const u8, addr_len: usize) -> SysResult<String> {
    const SUN_PATH_OFFSET: usize = 2;
    if addr_len <= SUN_PATH_OFFSET {
        return Err(SysError::EINVAL);
    }
    let path_len = addr_len - SUN_PATH_OFFSET;
    let path = unsafe { core::slice::from_raw_parts(addr_ptr.add(SUN_PATH_OFFSET), path_len) };
    if path.first() != Some(&0) {
        return Err(SysError::ENOENT);
    }
    let end = path[1..]
        .iter()
        .position(|byte| *byte == 0)
        .map(|pos| pos + 1)
        .unwrap_or(path_len);
    let name_bytes = &path[1..end];
    if name_bytes.is_empty() {
        return Err(SysError::EINVAL);
    }
    Ok(core::str::from_utf8(name_bytes)
        .map_err(|_| SysError::EINVAL)?
        .to_string())
}

fn read_unix_sockaddr(addr_ptr: *const u8, addr_len: usize) -> SysResult<UnixSockaddr> {
    const SUN_PATH_OFFSET: usize = 2;
    if addr_len <= SUN_PATH_OFFSET {
        return Err(SysError::EINVAL);
    }
    let path_len = addr_len - SUN_PATH_OFFSET;
    let path = unsafe { core::slice::from_raw_parts(addr_ptr.add(SUN_PATH_OFFSET), path_len) };
    if path.is_empty() {
        return Err(SysError::EINVAL);
    }
    if path[0] == 0 {
        let name = read_abstract_unix_name(addr_ptr, addr_len)?;
        return Ok(UnixSockaddr::Abstract(name));
    }

    let end = path.iter().position(|byte| *byte == 0).unwrap_or(path_len);
    let name_bytes = &path[..end];
    if name_bytes.is_empty() {
        return Err(SysError::EINVAL);
    }
    let path = core::str::from_utf8(name_bytes)
        .map_err(|_| SysError::EINVAL)?
        .to_string();
    Ok(UnixSockaddr::Pathname(path))
}

fn bind_pathname_unix_socket(path: &str) -> SyscallResult {
    if path.is_empty() {
        return Err(SysError::EINVAL);
    }

    let start = get_start_dentry(AT_FDCWD, path)?;
    let (parent_path, name) = split_parent_and_name(path);
    if name.is_empty() || name == "." || name == ".." {
        return Err(SysError::EINVAL);
    }

    let parent = if parent_path == "." || parent_path == "/" {
        start
    } else {
        resolve_path(start, &parent_path)?
    };
    let parent_inode = parent.get_inode().ok_or(SysError::ENOTDIR)?;
    if parent_inode.get_mode().get_type() != InodeMode::DIR {
        return Err(SysError::ENOTDIR);
    }
    if find_superblock_by_path(&parent.path()).is_some_and(|sb| sb.inner().is_readonly()) {
        return Err(SysError::EROFS);
    }
    match parent.find(&name) {
        Ok(_) => return Err(SysError::EADDRINUSE),
        Err(SysError::ENOENT) => {}
        Err(err) => return Err(err),
    }

    landlock_check_dentry(&parent, LANDLOCK_ACCESS_FS_MAKE_SOCK)?;

    let (mode, uid, gid) = {
        let process = current_process();
        let inner = process.inner_exclusive_access();
        let perm = 0o777 & !inner.umask;
        (
            InodeMode::SOCKET | InodeMode::from_bits_truncate(perm),
            inner.euid as usize,
            inner.egid as usize,
        )
    };
    parent.mknod(&name, mode, 0)?;
    if let Ok(dentry) = parent.find(&name) {
        if let Some(inode) = dentry.get_inode() {
            inode.set_uid(uid);
            inode.set_gid(gid);
        }
    }
    Ok(0)
}

fn register_abstract_unix_socket(name: String, pid: usize) {
    let mut sockets = ABSTRACT_UNIX_SOCKETS.lock();
    if let Some(entry) = sockets.iter_mut().find(|(n, _)| *n == name) {
        entry.1 = pid;
    } else {
        sockets.push((name, pid));
    }
}

fn lookup_abstract_unix_socket(name: &str) -> Option<usize> {
    ABSTRACT_UNIX_SOCKETS
        .lock()
        .iter()
        .find(|(n, _)| n.as_str() == name)
        .map(|(_, pid)| *pid)
}
