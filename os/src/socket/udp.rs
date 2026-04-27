use crate::error::{SysError, SysResult};
use crate::net::ip::ip_queue_xmit;
use crate::net::skb::Skb;
use crate::net::udp::UdpHeader;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use polyhal::println;
use spin::Mutex;

#[allow(unused)]
/// UDP套接字
pub struct UdpSocket {
    local_addr: Option<(u32, u16)>, // (IP地址, 端口) 主机字节序
    pub receive_queue: Mutex<VecDeque<(Skb, u32, u16)>>, // (数据包, 源IP, 源端口)
}
#[allow(unused)]
impl UdpSocket {
    pub fn new() -> Self {
        Self {
            local_addr: None,
            receive_queue: Mutex::new(VecDeque::new()),
        }
    }
    pub fn clear_queue(&mut self) {
        self.receive_queue.lock().clear();
        log::debug!("RawSocket: cleared receive queue");
    }

    /// 绑定到本地地址和端口
    pub fn bind(&mut self, addr: u32, port: u16) -> SysResult<()> {
        if self.local_addr.is_some() {
            return Err(SysError::EADDRINUSE);
        }
        self.local_addr = Some((addr, port));

        println!(
            "UDP: socket bound to {}.{}.{}.{}:{}",
            (addr >> 24) & 0xFF,
            (addr >> 16) & 0xFF,
            (addr >> 8) & 0xFF,
            addr & 0xFF,
            port
        );

        Ok(())
    }

    /// 发送数据到指定地址
    pub fn send_to(
        &self,
        data: &[u8],
        dst_addr: u32,
        dst_port: u16,
    ) -> SysResult<(Skb, u32, u16)> {
        let src = self.local_addr.ok_or(SysError::EINVAL)?;
        send_udp_packet(src, data, dst_addr, dst_port)
    }

    pub fn local_addr(&self) -> Option<(u32, u16)> {
        self.local_addr
    }
}

pub fn send_udp_packet(
    src: (u32, u16),
    data: &[u8],
    dst_addr: u32,
    dst_port: u16,
) -> SysResult<(Skb, u32, u16)> {
    // 分配 skb（UDP头 + 数据）
    let total_len = data.len() + UdpHeader::size();
    let mut skb = Skb::new(total_len);

    // 填充 UDP 头
    let udp_header = unsafe {
        &mut *(skb.put(UdpHeader::size())
            .ok_or(SysError::ENOMEM)?
            .as_mut_ptr() as *mut UdpHeader)
    };
    udp_header.set_source_port(src.1); // 源端口（主机字节序）
    udp_header.set_dest_port(dst_port); // 目标端口（主机字节序）
    udp_header.set_length(total_len as u16);
    udp_header.checksum = 0; // 简化：跳过校验和计算

    // 拷贝数据
    skb.put(data.len())
        .ok_or(SysError::ENOMEM)?
        .copy_from_slice(data);

    // 交给 IP 层发送
    ip_queue_xmit(skb, src.0, dst_addr, 17)
        .map_err(|_| SysError::ENETUNREACH) // IPPROTO_UDP = 17
}

#[allow(unused)]
impl UdpSocket {
    /// 接收数据
    /// 返回: (接收长度, 源IP地址, 源端口)
    pub fn recv_from(&self, buf: &mut [u8]) -> SysResult<(usize, u32, u16)> {
        let mut queue = self.receive_queue.lock();
        if let Some((skb, src_ip, src_port)) = queue.pop_front() {
            let copy_len = core::cmp::min(buf.len(), skb.len());
            buf[..copy_len].copy_from_slice(&skb.data()[..copy_len]);

            Ok((copy_len, src_ip, src_port))
        } else {
            Err(SysError::EAGAIN)
        }
    }

    /// 非阻塞接收
    pub fn try_recv_from(&self, buf: &mut [u8]) -> SysResult<(usize, u32, u16)> {
        let mut queue = self.receive_queue.lock();
        if let Some((skb, src_ip, src_port)) = queue.pop_front() {
            let copy_len = core::cmp::min(buf.len(), skb.len());
            buf[..copy_len].copy_from_slice(&skb.data()[..copy_len]);
            Ok((copy_len, src_ip, src_port))
        } else {
            Err(SysError::EAGAIN)
        }
    }
}

impl Clone for UdpSocket {
    fn clone(&self) -> Self {
        Self {
            local_addr: self.local_addr,
            receive_queue: Mutex::new(VecDeque::new()),
        }
    }
}

/// 全局UDP socket表（端口 -> socket）
static UDP_SOCKETS: Mutex<Vec<(u16, Arc<Mutex<UdpSocket>>)>> = Mutex::new(Vec::new());

pub fn register_udp_socket(port: u16, socket: Arc<Mutex<UdpSocket>>) {
    let mut table = UDP_SOCKETS.lock();
    if table
        .iter()
        .any(|(p, s)| *p == port && Arc::ptr_eq(s, &socket))
    {
        return;
    }
    table.push((port, socket));
}

pub fn unregister_udp_socket(port: u16, socket: Arc<Mutex<UdpSocket>>) {
    let mut table = UDP_SOCKETS.lock();
    table.retain(|(p, s)| !(*p == port && Arc::ptr_eq(s, &socket)));
}

pub fn lookup_udp_socket(port: u16) -> Option<Arc<Mutex<UdpSocket>>> {
    UDP_SOCKETS
        .lock()
        .iter()
        .find(|(p, _)| *p == port)
        .map(|(_, s)| s.clone())
}
