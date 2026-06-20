use crate::error::{SysError, SysResult};
use crate::net::ip::ip_queue_xmit;
use crate::net::route::route_lookup;
use crate::net::skb::Skb;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

/// 原始套接字（用于ICMP等协议）
#[allow(unused)]
pub struct RawSocket {
    pub protocol: u8,
    pub receive_queue: Mutex<VecDeque<(Skb, u32)>>,
}

#[allow(unused)]
impl RawSocket {
    /// 创建新的原始套接字
    pub fn new(protocol: u8) -> Self {
        Self {
            protocol,
            receive_queue: Mutex::new(VecDeque::new()),
        }
    }

    /// 发送原始IP数据包
    pub fn send_to(&self, data: &[u8], dst_addr: u32) -> SysResult<(Skb, u32, u16)> {
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
        send_raw_packet(src_addr, self.protocol, data, dst_addr)
    }

    pub fn protocol(&self) -> u8 {
        self.protocol
    }

    /// 接收数据
    pub fn recv_from(&self, buf: &mut [u8]) -> SysResult<(usize, u32)> {
        let mut queue = self.receive_queue.lock();
        if let Some((skb, src_ip)) = queue.pop_front() {
            let len = core::cmp::min(buf.len(), skb.len());
            buf[..len].copy_from_slice(&skb.data()[..len]);
            Ok((len, src_ip))
        } else {
            Err(SysError::EAGAIN)
        }
    }

    /// 非阻塞接收
    pub fn try_recv_from(&self, buf: &mut [u8]) -> SysResult<(usize, u32)> {
        let mut queue = self.receive_queue.lock();
        if let Some((skb, src_ip)) = queue.pop_front() {
            let len = core::cmp::min(buf.len(), skb.len());
            buf[..len].copy_from_slice(&skb.data()[..len]);
            Ok((len, src_ip))
        } else {
            Err(SysError::EAGAIN)
        }
    }

    /// 检查是否有待接收的数据
    pub fn has_data(&self) -> bool {
        !self.receive_queue.lock().is_empty()
    }

    /// 清空接收队列
    pub fn clear_queue(&self) {
        self.receive_queue.lock().clear();
        log::info!("RawSocket: cleared receive queue");
    }

    /// 将数据包放入接收队列（由协议栈调用）
    pub fn enqueue(&self, skb: Skb, src_ip: u32) {
        self.receive_queue.lock().push_back((skb, src_ip));
    }
}

pub fn send_raw_packet(
    src_addr: u32,
    protocol: u8,
    data: &[u8],
    dst_addr: u32,
) -> SysResult<(Skb, u32, u16)> {
    let mut skb = Skb::new(data.len());
    skb.put(data.len())
        .ok_or(SysError::ENOMEM)?
        .copy_from_slice(data);
    ip_queue_xmit(skb, src_addr, dst_addr, protocol).map_err(|_| SysError::ENETUNREACH)
}

/// 全局RAW socket表（协议号 -> socket）
static RAW_SOCKETS: Mutex<Vec<(u8, Arc<Mutex<RawSocket>>)>> = Mutex::new(Vec::new());

pub fn register_raw_socket(protocol: u8, socket: Arc<Mutex<RawSocket>>) {
    let mut table = RAW_SOCKETS.lock();
    if table
        .iter()
        .any(|(p, s)| *p == protocol && Arc::ptr_eq(s, &socket))
    {
        return;
    }
    table.push((protocol, socket));
}

pub fn unregister_raw_socket(protocol: u8, socket: Arc<Mutex<RawSocket>>) {
    let mut table = RAW_SOCKETS.lock();
    table.retain(|(p, s)| !(*p == protocol && Arc::ptr_eq(s, &socket)));
}

pub fn deliver_raw_packet(protocol: u8, skb: Skb, src_ip: u32) -> bool {
    let sockets = RAW_SOCKETS.lock();
    let mut delivered = false;
    for (p, sock) in sockets.iter() {
        if *p == protocol {
            sock.lock().enqueue(skb.clone(), src_ip);
            delivered = true;
        }
    }
    delivered
}
