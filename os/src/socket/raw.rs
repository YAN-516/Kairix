use crate::net::ip::ip_queue_xmit;
use crate::net::skb::Skb;
use alloc::collections::VecDeque;

/// 原始套接字（用于ICMP）
pub struct RawSocket {
    pub protocol: u8,
    pub receive_queue: VecDeque<Skb>,
}

impl RawSocket {
    pub fn new(protocol: u8) -> Self {
        Self {
            protocol,
            receive_queue: VecDeque::new(),
        }
    }

    pub fn send_to(&mut self, data: &[u8], dst: u32) -> Result<(), &'static str> {
        let mut skb = Skb::new(data.len());
        skb.put(data.len()).unwrap().copy_from_slice(data);

        // 原始套接字直接交给IP层（不需要传输层头）
        ip_queue_xmit(skb, 0x7F000001, dst, self.protocol)
    }

    pub fn recv_from(&mut self, buf: &mut [u8]) -> Result<usize, &'static str> {
        if let Some(skb) = self.receive_queue.pop_front() {
            let len = core::cmp::min(buf.len(), skb.len);
            buf[..len].copy_from_slice(&skb.data()[..len]);
            Ok(len)
        } else {
            Err("No data")
        }
    }
}
