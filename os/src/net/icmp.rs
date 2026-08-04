//! ICMP Echo 的最小实现。
//!
//! 当前主要支持 ping 所需的 Echo Request / Echo Reply。

use crate::net::ip::ip_queue_xmit;
use crate::net::skb::Skb;
/// ICMP Echo 头结构。
#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
#[allow(unused)]
pub struct IcmpHeader {
    /// ICMP 类型，如 Echo Reply=0、Echo Request=8。
    type_: u8,
    /// ICMP code，Echo 报文中通常为 0。
    code: u8,
    /// 校验和。
    checksum: u16,
    /// Echo 标识符，常由用户态 ping 填入进程 id。
    pid: u16,
    /// Echo 序列号。
    seq: u16,
}
#[allow(unused)]
impl IcmpHeader {
    /// ICMP Echo Reply 类型值。
    pub const ECHO_REPLY: u8 = 0;
    /// ICMP Echo Request 类型值。
    pub const ECHO_REQUEST: u8 = 8;

    /// 返回 ICMP Echo 头部长度。
    pub fn size() -> usize {
        core::mem::size_of::<IcmpHeader>()
    }
}
#[allow(unused)]
/// 计算 ICMP 校验和。
///
/// ICMP checksum 覆盖整个 ICMP 报文，包括头部和数据。
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
#[allow(unused)]
/// ICMP 接收处理。
///
/// Echo Request 会被原样改写成 Echo Reply 后重新从 IP 层发送。
pub fn icmp_rcv(skb: Skb, src_ip: u32, dst_ip: u32) -> Result<(Skb, u32, u16), &'static str> {
    //println!("enter icmp recv");
    if skb.len() < IcmpHeader::size() {
        return Err("ICMP packet too short");
    }

    let icmp = unsafe { &*(skb.data().as_ptr() as *const IcmpHeader) };

    log::debug!("ICMP: received type {} code {}", icmp.type_, icmp.code);

    match icmp.type_ {
        IcmpHeader::ECHO_REQUEST => {
            // 生成ECHO REPLY
            icmp_reply(skb, src_ip, dst_ip)
        }
        IcmpHeader::ECHO_REPLY => {
            //println!("{:?}", skb.data);
            Ok((skb, 0, 0))
        }
        _ => {
            log::debug!("Unsupported ICMP type: {}", icmp.type_);
            Err("Unsupported ICMP type")
        }
    }
}
#[allow(unused)]
/// 发送 ICMP Echo Reply。
fn icmp_reply(mut skb: Skb, src_ip: u32, dst_ip: u32) -> Result<(Skb, u32, u16), &'static str> {
    // Echo Reply 应交换源/目的地址。
    let src = dst_ip;
    let dst = src_ip;

    // Modify the wire bytes directly so checksum byte order is explicit and
    // no packed mutable reference aliases the immutable checksum input.
    {
        let data = skb.data_mut();
        data[0] = IcmpHeader::ECHO_REPLY;
        data[2] = 0;
        data[3] = 0;
    }
    let checksum = icmp_csum(skb.data());
    skb.data_mut()[2..4].copy_from_slice(&checksum.to_be_bytes());
    let verify = icmp_csum(skb.data());

    log::debug!(
        "ICMP: sending echo reply checksum={:#06x} verify={:#06x}",
        checksum,
        verify,
    );
    if verify != 0 {
        return Err("ICMP checksum self-check failed");
    }
    // 重新发送
    ip_queue_xmit(skb, src, dst, 1) // IPPROTO_ICMP = 1
}
