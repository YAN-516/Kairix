use crate::socket::raw::RawSocket;
use crate::socket::udp::UdpSocket;
use crate::socket::{Socket, SocketInner, socket_manager};
use alloc::sync::Arc;
use spin::Mutex;

/// socket() 系统调用
pub fn sys_socket(domain: i32, type_: i32, protocol: i32) -> Result<usize, &'static str> {
    if domain != 2 {
        // AF_INET
        return Err("Only AF_INET supported");
    }

    let socket = match type_ {
        1 => {
            // SOCK_STREAM
            Socket {
                inner: SocketInner::Tcp(crate::socket::tcp::TcpSocket::new()),
                fd: 0,
            }
        }
        2 => {
            // SOCK_DGRAM
            let udp = UdpSocket::new();
            Socket {
                inner: SocketInner::Udp(Arc::new(Mutex::new(udp))),
                fd: 0,
            }
        }
        3 => {
            // SOCK_RAW
            Socket {
                inner: SocketInner::Raw(RawSocket::new(protocol as u8)),
                fd: 0,
            }
        }
        _ => return Err("Unsupported socket type"),
    };

    let mut manager = socket_manager().lock();
    let manager_ref = manager.as_mut().ok_or("Socket manager not initialized")?;
    let fd = manager_ref.add_socket(socket);

    log::info!("Created socket: fd={}, type={}", fd, type_);

    Ok(fd)
}

/// bind() 系统调用
pub fn sys_bind(fd: usize, addr_ptr: *const u8, addr_len: usize) -> Result<(), &'static str> {
    let manager = socket_manager().lock();
    let manager_ref = manager.as_ref().ok_or("Socket manager not initialized")?;
    let socket = manager_ref.get_socket(fd).ok_or("Invalid fd")?;

    // 简化：假设sockaddr_in结构
    if addr_len != 16 {
        return Err("Invalid address length");
    }

    unsafe {
        let sin_family = *(addr_ptr as *const u16);
        let sin_port = *(addr_ptr.offset(2) as *const u16);
        let sin_addr = *(addr_ptr.offset(4) as *const u32);

        if sin_family != 2 {
            // AF_INET
            return Err("Only AF_INET supported");
        }

        match &socket.inner {
            SocketInner::Udp(udp) => {
                let mut udp = udp.lock();
                udp.bind(sin_addr, u16::from_be(sin_port))?;
            }
            _ => return Err("Bind not supported for this socket type"),
        }
    }

    log::debug!("Bound socket fd={}", fd);
    Ok(())
}

/// sendto() 系统调用
pub fn sys_sendto(
    fd: usize,
    buf_ptr: *const u8,
    len: usize,
    flags: i32,
    addr_ptr: *const u8,
    addr_len: usize,
) -> Result<usize, &'static str> {
    let manager = socket_manager().lock();
    let manager_ref = manager.as_ref().ok_or("Socket manager not initialized")?;
    let socket = manager_ref.get_socket(fd).ok_or("Invalid fd")?;

    // 读取数据
    let data = unsafe { core::slice::from_raw_parts(buf_ptr, len) };

    // 解析目标地址
    let (dst_addr, dst_port) = if addr_ptr.is_null() {
        (0x7F000001, 0) // 默认回环地址
    } else {
        unsafe {
            let sin_port = *(addr_ptr.offset(2) as *const u16);
            let sin_addr = *(addr_ptr.offset(4) as *const u32);
            (sin_addr, u16::from_be(sin_port))
        }
    };

    match &socket.inner {
        SocketInner::Udp(udp) => {
            udp.lock().send_to(data, dst_addr, dst_port)?;
            Ok(len)
        }
        SocketInner::Raw(raw) => {
            // 需要从全局获取mut引用，简化处理
            Err("Raw socket send not implemented")
        }
        _ => Err("Send not supported for this socket type"),
    }
}

/// recvfrom() 系统调用
pub fn sys_recvfrom(
    fd: usize,
    buf_ptr: *mut u8,
    len: usize,
    flags: i32,
    addr_ptr: *mut u8,
    addr_len: *mut usize,
) -> Result<usize, &'static str> {
    let manager = socket_manager().lock();
    let manager_ref = manager.as_ref().ok_or("Socket manager not initialized")?;
    let socket = manager_ref.get_socket(fd).ok_or("Invalid fd")?;

    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr, len) };

    match &socket.inner {
        SocketInner::Udp(udp) => {
            let (recv_len, src_addr, src_port) = udp.lock().recv_from(buf)?;

            // 填充源地址（如果需要）
            if !addr_ptr.is_null() && !addr_len.is_null() {
                unsafe {
                    let sin_family = addr_ptr as *mut u16;
                    let sin_port = addr_ptr.offset(2) as *mut u16;
                    let sin_addr = addr_ptr.offset(4) as *mut u32;

                    *sin_family = 2; // AF_INET
                    *sin_port = src_port.to_be();
                    *sin_addr = src_addr;
                    *addr_len = 16;
                }
            }

            Ok(recv_len)
        }
        _ => Err("Recv not supported for this socket type"),
    }
}
