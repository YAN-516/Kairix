//! UDP 收发辅助。
//!
//! 收包路径负责校验 UDP 长度和校验和，并把 payload 投递到 socket 层。

use crate::net::ip::ip_queue_xmit;
use crate::net::skb::Skb;
use crate::socket::udp::lookup_udp_socket;
use crate::trap::_set_sum_bit;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use log::{error, info};
use spin::Mutex;

/// UDP 头结构。
#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
#[allow(unused)]
pub struct UdpHeader {
    /// 源端口（网络字节序）。
    pub src_port: u16,
    /// 目标端口（网络字节序）。
    pub dst_port: u16,
    /// UDP 包长度，包含 UDP 头和 payload（网络字节序）。
    pub len: u16,
    /// UDP 校验和（网络字节序）。
    pub checksum: u16,
}
#[allow(unused)]
impl UdpHeader {
    /// 返回 UDP 头部长度。
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

/// 将 32 位累加和折叠成 16 位 Internet checksum。
fn checksum_fold(mut sum: u32) -> u16 {
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

/// 计算 UDP 校验和。
///
/// UDP 校验和覆盖 IPv4 伪首部、UDP 头和 payload。返回值为 0 时按规范
/// 以 `0xffff` 发送，避免和“未启用校验和”的 0 混淆。
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
    if checksum == 0 {
        0xFFFF
    } else {
        checksum
    }
}

/// UDP 接收处理（由 IP 层调用）。
///
/// 校验通过后剥离 UDP 头，按目标端口和远端地址查找 socket 并入队 payload。
pub fn udp_rcv(mut skb: Skb, src_ip: u32, dst_ip: u32) -> Result<(Skb, u32, u16), &'static str> {
    _set_sum_bit();
    // 检查长度
    if skb.len() < UdpHeader::size() {
        return Err("UDP packet too short");
    }
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
        }

        info!(
            "UDP: delivered packet dst_port={} src={}:{} len={}",
            dst_port, src_ip, src_port, payload_len
        );
        Ok((Skb::new(0), src_ip, src_port))
    } else {
        info!("UDP: no socket for dst_port={}", dst_port);
        Err("No socket")
    }
}
