use crate::net::skb::Skb;

/// TCP头结构（简化版）
#[repr(C, packed)]
pub struct TcpHeader {
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack_seq: u32,
    flags: u16,
    window: u16,
    checksum: u16,
    urgent: u16,
}

/// TCP接收处理（简化）
pub fn tcp_rcv(_skb: Skb) -> Result<(), &'static str> {
    log::warn!("TCP not fully implemented yet");
    Err("TCP not implemented")
}
