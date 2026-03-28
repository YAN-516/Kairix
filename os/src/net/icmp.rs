use crate::net::ip::ip_queue_xmit;
use crate::net::skb::Skb;

/// ICMP头结构
#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
pub struct IcmpHeader {
    type_: u8,
    code: u8,
    checksum: u16,

    pid: u16,
    seq: u16,
}

impl IcmpHeader {
    pub const ECHO_REPLY: u8 = 0;
    pub const ECHO_REQUEST: u8 = 8;

    pub fn size() -> usize {
        core::mem::size_of::<IcmpHeader>()
    }
}

/// ICMP校验和
fn icmp_csum(data: &[u8]) -> u16 {
    let mut sum = 0u32;
    let chunks = data.chunks_exact(2);
    for chunk in chunks {
        sum += ((chunk[0] as u32) << 8) | (chunk[1] as u32);
        if sum >> 16 != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
    }
    if data.len() % 2 == 1 {
        sum += (data[data.len() - 1] as u32) << 8;
        if sum >> 16 != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
    }
    !sum as u16
}

/// ICMP接收处理
pub fn icmp_rcv(skb: Skb) -> Result<(), &'static str> {
    if skb.data.len() < IcmpHeader::size() {
        return Err("ICMP packet too short");
    }

    let icmp = unsafe { &*(skb.data().as_ptr() as *const IcmpHeader) };

    log::debug!("ICMP: received type {} code {}", icmp.type_, icmp.code);

    match icmp.type_ {
        IcmpHeader::ECHO_REQUEST => {
            // 生成ECHO REPLY
            icmp_reply(skb)
        }
        _ => {
            log::warn!("Unsupported ICMP type: {}", icmp.type_);
            Err("Unsupported ICMP type")
        }
    }
}

/// 发送ICMP Echo Reply
fn icmp_reply(mut skb: Skb) -> Result<(), &'static str> {
    // 获取IP头信息（需要从skb中提取）
    // 简化：假设我们知道源和目标地址
    let src = 0x7F000001u32; // 127.0.0.1
    let dst = 0x7F000001u32;

    // 修改ICMP类型
    let icmp = unsafe { &mut *(skb.data_mut().as_mut_ptr() as *mut IcmpHeader) };
    icmp.type_ = IcmpHeader::ECHO_REPLY;

    // 重新计算校验和
    icmp.checksum = 0;
    let checksum = icmp_csum(skb.data());
    icmp.checksum = checksum;

    log::debug!("ICMP: sending echo reply");

    // 重新发送
    ip_queue_xmit(skb, src, dst, 1) // IPPROTO_ICMP = 1
}
