use crate::net::ip::ip_queue_xmit;
use crate::net::skb::Skb;
use crate::socket::udp::lookup_udp_socket;
use crate::trap::_set_sum_bit;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use log::error;
use spin::Mutex;

/// UDP头结构
#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
#[allow(unused)]
pub struct UdpHeader {
    pub src_port: u16, // 源端口（网络字节序）
    pub dst_port: u16, // 目标端口（网络字节序）
    pub len: u16,      // UDP包长度（网络字节序）
    pub checksum: u16, // 校验和（网络字节序）
}
#[allow(unused)]
impl UdpHeader {
    pub fn size() -> usize {
        core::mem::size_of::<UdpHeader>()
    }

    /// 获取源端口（主机字节序）
    pub fn source_port(&self) -> u16 {
        u16::from_be(self.src_port)
    }

    /// 获取目标端口（主机字节序）
    pub fn dest_port(&self) -> u16 {
        u16::from_be(self.dst_port)
    }

    /// 设置源端口（主机字节序转网络字节序）
    pub fn set_source_port(&mut self, port: u16) {
        self.src_port = port.to_be();
    }

    /// 设置目标端口
    pub fn set_dest_port(&mut self, port: u16) {
        self.dst_port = port.to_be();
    }

    /// 获取UDP长度（主机字节序）
    pub fn length(&self) -> u16 {
        u16::from_be(self.len)
    }

    /// 设置UDP长度
    pub fn set_length(&mut self, len: u16) {
        self.len = len.to_be();
    }
}

fn checksum_fold(mut sum: u32) -> u16 {
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

pub fn udp_checksum(src_ip: u32, dst_ip: u32, datagram: &[u8]) -> u16 {
    let mut sum: u32 = 0;

    sum += ((src_ip >> 16) & 0xFFFF) as u32;
    sum += (src_ip & 0xFFFF) as u32;
    sum += ((dst_ip >> 16) & 0xFFFF) as u32;
    sum += (dst_ip & 0xFFFF) as u32;
    sum += 17u32;
    sum += datagram.len() as u32;

    let mut i = 0usize;
    while i + 1 < datagram.len() {
        let word = ((datagram[i] as u16) << 8) | datagram[i + 1] as u16;
        sum += word as u32;
        i += 2;
    }
    if i < datagram.len() {
        sum += (datagram[i] as u32) << 8;
    }

    let checksum = checksum_fold(sum);
    if checksum == 0 { 0xFFFF } else { checksum }
}

/// UDP接收处理（由IP层调用）
pub fn udp_rcv(mut skb: Skb, src_ip: u32, dst_ip: u32) -> Result<(Skb, u32, u16), &'static str> {
    _set_sum_bit();
    // 检查长度
    if skb.len() < UdpHeader::size() {
        return Err("UDP packet too short");
    }
    // println!(
    //     "UDP: received packet of {} bytes from {}.{}.{}.{}",
    //     skb.len(),
    //     (src_ip >> 24) & 0xFF,
    //     (src_ip >> 16) & 0xFF,
    //     (src_ip >> 8) & 0xFF,
    //     src_ip & 0xFF,
    // );
    // 解析 UDP 头
    let udp_header = unsafe { &*(skb.data().as_ptr() as *const UdpHeader) };

    let dst_port = udp_header.dest_port(); // 主机字节序
    let src_port = udp_header.source_port(); // 主机字节序
    let udp_len = udp_header.length() as usize;
    let checksum = u16::from_be(udp_header.checksum);
    if udp_len < UdpHeader::size() || udp_len > skb.len() {
        return Err("UDP length invalid");
    }
    if checksum != 0 && udp_checksum(src_ip, dst_ip, &skb.data()[..udp_len]) != 0xFFFF {
        return Err("UDP checksum invalid");
    }
    if udp_len < skb.len() {
        let _ = skb.trim(skb.len() - udp_len);
    }
    //println!("{:?} {:?}", src_ip, dst_port);
    // 查找对应的 socket
    if let Some(socket) = lookup_udp_socket(dst_port, src_ip, src_port) {
        // 移除 UDP 头
        skb.pull(UdpHeader::size());

        let sock = socket.lock();
        let payload_len = skb.len();
        if sock.can_receive(payload_len) {
            sock.enqueue(skb, src_ip, src_port);
            // 唤醒可能阻塞在 recvfrom 上的任务
            sock.wake();
        } else {
            // 接收缓冲区已满，丢弃数据包
            // println!("UDP: dropping packet, receive buffer full for port {}", dst_port);
        }

        // println!("UDP: delivered packet to socket on port {}", dst_port);
        error!(
            "UDP: delivered packet dst_port={} src={}:{} len={}",
            dst_port, src_ip, src_port, payload_len
        );
        Ok((Skb::new(0), src_ip, src_port))
    } else {
        // println!("UDP: no socket for port {}", dst_port);
        error!("UDP: no socket for dst_port={}", dst_port);
        Err("No socket")
    }
}
