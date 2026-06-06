use crate::net::ip::ip_queue_xmit;
use crate::net::skb::Skb;
/// ICMP头结构
#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
#[allow(unused)]
pub struct IcmpHeader {
    type_: u8,
    code: u8,
    ///校验和
    checksum: u16,
    ///进程id
    pid: u16,
    ///序列号
    seq: u16,
}
#[allow(unused)]
impl IcmpHeader {
    ///两种ICMP报文
    pub const ECHO_REPLY: u8 = 0;
    pub const ECHO_REQUEST: u8 = 8;

    pub fn size() -> usize {
        core::mem::size_of::<IcmpHeader>()
    }
}
#[allow(unused)]
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
#[allow(unused)]
/// ICMP接收处理
pub fn icmp_rcv(skb: Skb, src_ip: u32, dst_ip: u32) -> Result<(Skb, u32, u16), &'static str> {
    //println!("enter icmp recv");
    if skb.len() < IcmpHeader::size() {
        return Err("ICMP packet too short");
    }

    let icmp = unsafe { &*(skb.data().as_ptr() as *const IcmpHeader) };

    log::info!("ICMP: received type {} code {}", icmp.type_, icmp.code);

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
            log::info!("Unsupported ICMP type: {}", icmp.type_);
            Err("Unsupported ICMP type")
        }
    }
}
#[allow(unused)]
/// 发送ICMP Echo Reply
fn icmp_reply(mut skb: Skb, src_ip: u32, dst_ip: u32) -> Result<(Skb, u32, u16), &'static str> {
    // Echo Reply 应交换源/目的地址。
    let src = dst_ip;
    let dst = src_ip;

    // 修改ICMP类型
    let icmp = unsafe { &mut *(skb.data_mut().as_mut_ptr() as *mut IcmpHeader) };
    icmp.type_ = IcmpHeader::ECHO_REPLY;

    // 重新计算校验和
    icmp.checksum = 0;
    let checksum = icmp_csum(skb.data());
    icmp.checksum = checksum;

    log::info!("ICMP: sending echo reply");
    // 重新发送
    ip_queue_xmit(skb, src, dst, 1) // IPPROTO_ICMP = 1
}
