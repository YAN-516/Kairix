use crate::net::icmp::icmp_rcv;
use crate::net::route::{self, route_lookup};
use crate::net::skb::Skb;
use crate::net::tcp::tcp_rcv;
use crate::net::udp::udp_rcv;

/// IPv4头结构
#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
pub struct Ipv4Header {
    version_ihl: u8,
    tos: u8,
    total_len: u16,
    id: u16,
    flags_frag: u16,
    ttl: u8,
    protocol: u8,
    checksum: u16,
    src_addr: u32,
    dst_addr: u32,
}

impl Ipv4Header {
    pub fn version(&self) -> u8 {
        self.version_ihl >> 4
    }

    pub fn ihl(&self) -> u8 {
        (self.version_ihl & 0x0F) * 4
    }

    pub fn set_version_ihl(&mut self) {
        self.version_ihl = (4 << 4) | 5; // IPv4, 5个32位字
    }

    pub fn total_len(&self) -> u16 {
        u16::from_be(self.total_len)
    }

    pub fn set_total_len(&mut self, len: u16) {
        self.total_len = len.to_be();
    }
}

/// IP校验和计算
fn ip_fast_csum(header: &[u16]) -> u16 {
    let mut sum = 0u32;
    for &word in header {
        sum += word as u32;
        if sum >> 16 != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
    }
    !sum as u16
}

/// IP接收处理
pub fn ip_rcv(mut skb: Skb) -> Result<(), &'static str> {
    // 检查长度
    if skb.data.len() < core::mem::size_of::<Ipv4Header>() {
        return Err("IP packet too short");
    }

    // 解析IP头
    let ip_header = unsafe { &*(skb.data().as_ptr() as *const Ipv4Header) };

    // 检查版本
    if ip_header.version() != 4 {
        return Err("Not IPv4");
    }

    let ihl = ip_header.ihl() as usize;
    if skb.len < ihl {
        return Err("IP header truncated");
    }

    // 验证校验和
    let words = unsafe { core::slice::from_raw_parts(skb.data().as_ptr() as *const u16, ihl / 2) };
    if ip_fast_csum(words) != 0 {
        return Err("Invalid IP checksum");
    }

    let dst_addr = u32::from_be(ip_header.dst_addr);

    log::debug!(
        "IP: received packet from {} to {}",
        u32::from_be(ip_header.src_addr),
        dst_addr
    );

    // 检查是否是本地地址（127.0.0.0/8）
    let is_local = (dst_addr & 0xFF000000) == 0x7F000000;

    if is_local {
        // 移除IP头
        skb.pull(ihl);
        skb.transport_header = skb.head;

        // 根据协议分发
        match ip_header.protocol {
            1 => icmp_rcv(skb), // ICMP
            17 => udp_rcv(skb), // UDP
            6 => tcp_rcv(skb),  // TCP
            proto => {
                log::warn!("Unsupported IP protocol: {}", proto);
                Err("Unsupported protocol")
            }
        }
    } else {
        log::warn!("Packet not for local address: {}", dst_addr);
        Err("Not for local")
    }
}

/// IP发送
pub fn ip_queue_xmit(mut skb: Skb, src: u32, dst: u32, protocol: u8) -> Result<(), &'static str> {
    log::debug!(
        "IP: sending packet from {} to {} proto {}",
        src,
        dst,
        protocol
    );

    // 预留IP头空间
    let header_size = core::mem::size_of::<Ipv4Header>();
    skb.reserve(header_size);

    // 填充IP头
    let ip_header =
        unsafe { &mut *(skb.push(header_size).unwrap().as_mut_ptr() as *mut Ipv4Header) };
    ip_header.set_version_ihl();
    ip_header.tos = 0;
    ip_header.set_total_len((skb.len) as u16);
    ip_header.id = (fast_random() & 0xFFFF) as u16;
    ip_header.flags_frag = 0;
    ip_header.ttl = 64;
    ip_header.protocol = protocol;
    ip_header.src_addr = src.to_be();
    ip_header.dst_addr = dst.to_be();

    // 计算校验和
    let words = unsafe {
        core::slice::from_raw_parts(ip_header as *const _ as *const u16, header_size / 2)
    };
    ip_header.checksum = ip_fast_csum(words);

    // 路由查找获取设备
    let dev = route_lookup(dst)?;
    skb.dev = Some(dev.clone());

    // 发送
    if let Err(err) = dev.hard_start_xmit(skb) {
        let err_str: &'static str = err.into(); // 通过 Into 转换
        Err(err_str)
    } else {
        Ok(())
    }
}

/// 简单的随机数生成
fn fast_random() -> u32 {
    static mut STATE: u32 = 0x12345678;
    unsafe {
        STATE = STATE.wrapping_mul(1103515245).wrapping_add(12345);
        STATE
    }
}
