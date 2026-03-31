use crate::net::ip::ip_queue_xmit;
use crate::net::skb::Skb;
use crate::socket::SocketManager;
use crate::task::process::{self, ProcessControlBlockInner};
use alloc::collections::VecDeque;
use spin::{Mutex, MutexGuard};
/// 原始套接字（用于ICMP等协议）
#[allow(unused)]
pub struct RawSocket {
    pub protocol: u8,
    pub receive_queue: Mutex<VecDeque<Skb>>,
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

    /// 发送数据
    pub fn send_to(&mut self, data: &[u8], dst: u32) -> Result<Skb, &'static str> {
        let mut skb = Skb::new(data.len());

        skb.put(data.len()).unwrap().copy_from_slice(data);
        println!("enter raw sending");
        log::debug!(
            "RawSocket: sending {} bytes to {}.{}.{}.{} protocol {}",
            data.len(),
            (dst >> 24) & 0xFF,
            (dst >> 16) & 0xFF,
            (dst >> 8) & 0xFF,
            dst & 0xFF,
            self.protocol
        );
        println!("Rawsocket sending...");
        // 原始套接字直接交给IP层（不需要传输层头）
        // 使用 127.0.0.1 作为源地址
        ip_queue_xmit(skb, 0x7F000001, dst, self.protocol)
    }

    /// 接收数据
    pub fn recv_from(&mut self, buf: &mut [u8]) -> Result<usize, &'static str> {
        let mut queue = self.receive_queue.lock();
        if let Some(skb) = queue.pop_front() {
            let len = core::cmp::min(buf.len(), skb.len());
            buf[..len].copy_from_slice(&skb.data()[..len]);

            log::debug!("RawSocket: received {} bytes", len);
            Ok(len)
        } else {
            Err("No data")
        }
    }

    /// 非阻塞接收
    pub fn try_recv_from(&mut self, buf: &mut [u8]) -> Option<usize> {
        let mut queue = self.receive_queue.lock();
        if let Some(skb) = queue.pop_front() {
            let len = core::cmp::min(buf.len(), skb.len());
            buf[..len].copy_from_slice(&skb.data()[..len]);
            Some(len)
        } else {
            None
        }
    }

    /// 检查是否有待接收的数据
    pub fn has_data(&self) -> bool {
        let mut queue = self.receive_queue.lock();
        !queue.is_empty()
    }

    /// 清空接收队列
    pub fn clear_queue(&mut self) {
        let mut queue = self.receive_queue.lock();
        queue.clear();
        log::debug!("RawSocket: cleared receive queue");
    }

    /// 将数据包放入接收队列（由协议栈调用）
    pub fn enqueue(&mut self, skb: Skb) {
        let mut queue = self.receive_queue.lock();
        queue.push_back(skb);
    }
}
