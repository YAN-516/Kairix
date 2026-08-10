//! ICMP Echo 和 IPv4 Path MTU Discovery 的最小实现。
//!
//! 支持 ping 所需的 Echo Request / Echo Reply，并处理 Destination Unreachable
//! 中的 Fragmentation Needed 反馈。

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
    /// Destination Unreachable 类型值。
    pub const DESTINATION_UNREACHABLE: u8 = 3;
    /// Destination Unreachable 中的 Fragmentation Needed code。
    pub const FRAGMENTATION_NEEDED: u8 = 4;

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

    if icmp_csum(skb.data()) != 0 {
        return Err("Invalid ICMP checksum");
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
        IcmpHeader::DESTINATION_UNREACHABLE if icmp.code == IcmpHeader::FRAGMENTATION_NEEDED => {
            handle_fragmentation_needed(&skb, dst_ip)?;
            Ok((skb, src_ip, 0))
        }
        _ => {
            log::debug!("Unsupported ICMP type: {}", icmp.type_);
            Err("Unsupported ICMP type")
        }
    }
}

/// RFC 1191 为未填写 next-hop MTU 的旧路由器定义的常见 MTU plateau。
const MTU_PLATEAUS: [u16; 11] = [
    65535, 32000, 17914, 8166, 4352, 2002, 1492, 1006, 508, 296, 68,
];

fn legacy_next_hop_mtu(original_len: u16) -> u16 {
    MTU_PLATEAUS
        .iter()
        .copied()
        .find(|mtu| *mtu < original_len)
        .unwrap_or(crate::net::tcp::IPV4_MIN_PATH_MTU)
}

/// 解析 ICMP type 3/code 4 引用的原始 IPv4/TCP 头并更新对应连接的 PMTU。
fn handle_fragmentation_needed(skb: &Skb, local_ip: u32) -> Result<(), &'static str> {
    const ICMP_ERROR_HEADER_LEN: usize = 8;
    const IPV4_MIN_HEADER_LEN: usize = 20;
    const TCP_QUOTED_LEN: usize = 8;

    let data = skb.data();
    if data.len() < ICMP_ERROR_HEADER_LEN + IPV4_MIN_HEADER_LEN + TCP_QUOTED_LEN {
        return Err("ICMP fragmentation-needed packet too short");
    }

    let quoted = &data[ICMP_ERROR_HEADER_LEN..];
    if quoted[0] >> 4 != 4 {
        return Err("ICMP quote is not IPv4");
    }
    let ihl = ((quoted[0] & 0x0F) as usize) * 4;
    if ihl < IPV4_MIN_HEADER_LEN || quoted.len() < ihl + TCP_QUOTED_LEN {
        return Err("ICMP quoted IPv4 header truncated");
    }
    if icmp_csum(&quoted[..ihl]) != 0 {
        return Err("ICMP quoted IPv4 checksum invalid");
    }

    let original_len = u16::from_be_bytes([quoted[2], quoted[3]]);
    if (original_len as usize) < ihl + TCP_QUOTED_LEN {
        return Err("ICMP quoted IPv4 length invalid");
    }
    if quoted[9] != 6 {
        return Ok(());
    }

    let original_src = u32::from_be_bytes([quoted[12], quoted[13], quoted[14], quoted[15]]);
    let original_dst = u32::from_be_bytes([quoted[16], quoted[17], quoted[18], quoted[19]]);
    if original_src != local_ip && !crate::net::ip::is_local_ip(original_src) {
        return Err("ICMP quote does not reference a local packet");
    }

    let tcp = &quoted[ihl..ihl + TCP_QUOTED_LEN];
    let local_port = u16::from_be_bytes([tcp[0], tcp[1]]);
    let remote_port = u16::from_be_bytes([tcp[2], tcp[3]]);
    let quoted_seq = u32::from_be_bytes([tcp[4], tcp[5], tcp[6], tcp[7]]);

    let reported_mtu = u16::from_be_bytes([data[6], data[7]]);
    let next_hop_mtu = if reported_mtu == 0 {
        legacy_next_hop_mtu(original_len)
    } else {
        reported_mtu
    };
    if next_hop_mtu < crate::net::tcp::IPV4_MIN_PATH_MTU || next_hop_mtu >= original_len {
        return Err("ICMP next-hop MTU invalid");
    }

    if let Some(new_mss) = crate::net::tcp::update_path_mtu(original_dst, next_hop_mtu) {
        crate::socket::tcp::handle_pmtu_update(
            original_src,
            local_port,
            original_dst,
            remote_port,
            quoted_seq,
            new_mss,
        );
    }
    Ok(())
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
