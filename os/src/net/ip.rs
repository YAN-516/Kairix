use crate::net::device::XmitError;
use crate::net::icmp::icmp_rcv;
use crate::net::neighbor::neighbour_output;
use crate::net::route::route_lookup;
use crate::net::skb::Skb;
use crate::net::tcp::tcp_rcv;
use crate::net::udp::udp_rcv;
use crate::socket::raw::deliver_raw_packet;
use alloc::sync::Arc;
use alloc::vec::Vec;
use log::debug;
use spin::Mutex;
/// IPv4头结构
#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
#[allow(unused)]
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

#[allow(unused)]
impl Ipv4Header {
    /// 获取版本号
    pub fn version(&self) -> u8 {
        self.version_ihl >> 4
    }

    /// 获取 IP 头长度（字节）
    pub fn ihl(&self) -> u8 {
        (self.version_ihl & 0x0F) * 4
    }

    /// 设置版本为 IPv4，头长度为 20 字节
    pub fn set_version_ihl(&mut self) {
        self.version_ihl = (4 << 4) | 5;
    }

    /// 获取总长度（主机字节序）
    pub fn total_len(&self) -> u16 {
        u16::from_be(self.total_len)
    }

    /// 设置总长度（网络字节序）
    pub fn set_total_len(&mut self, len: u16) {
        self.total_len = len.to_be();
    }

    /// 获取源地址（主机字节序）
    pub fn src_addr(&self) -> u32 {
        u32::from_be(self.src_addr)
    }

    /// 获取目标地址（主机字节序）
    pub fn dst_addr(&self) -> u32 {
        u32::from_be(self.dst_addr)
    }
}

/// Internet checksum over bytes in network order.
#[allow(unused)]
fn ip_checksum(data: &[u8]) -> u16 {
    let mut sum = 0u32;
    let mut chunks = data.chunks_exact(2);
    for chunk in &mut chunks {
        sum += ((chunk[0] as u32) << 8) | chunk[1] as u32;
        if sum >> 16 != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
    }
    if let Some(&last) = chunks.remainder().first() {
        sum += (last as u32) << 8;
        if sum >> 16 != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
    }
    !sum as u16
}

/// 全局本机 IP 地址列表
static LOCAL_IPS: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// 添加本机 IP 地址
#[allow(unused)]
pub fn add_local_ip(ip: u32) {
    LOCAL_IPS.lock().push(ip);
    log::info!(
        "Added local IP: {}.{}.{}.{}",
        (ip >> 24) & 0xFF,
        (ip >> 16) & 0xFF,
        (ip >> 8) & 0xFF,
        ip & 0xFF
    );
}

/// 检查是否是本机 IP
pub fn is_local_ip(ip: u32) -> bool {
    // 检查 127.0.0.0/8 回环
    if (ip & 0xFF000000) == 0x7F000000 {
        return true;
    }
    // 检查配置的本机 IP
    LOCAL_IPS.lock().contains(&ip)
}

#[allow(unused)]
/// IP 接收处理
pub fn ip_rcv(mut skb: Skb) -> Result<(Skb, u32, u16), &'static str> {
    debug!("IP: received packet of {} bytes", skb.len());
    if skb.len() < core::mem::size_of::<Ipv4Header>() {
        return Err("IP packet too short");
    }

    let ip_header = unsafe { &*(skb.data().as_ptr() as *const Ipv4Header) };

    if ip_header.version() != 4 {
        return Err("Not IPv4");
    }

    let ihl = ip_header.ihl() as usize;
    if skb.len() < ihl {
        return Err("IP header truncated");
    }

    if ip_checksum(&skb.data()[..ihl]) != 0 {
        return Err("Invalid IP checksum");
    }

    let total_len = ip_header.total_len() as usize;
    if total_len < ihl || total_len > skb.len() {
        return Err("Invalid IP total length");
    }

    let src_addr = ip_header.src_addr();
    let dst_addr = ip_header.dst_addr();
    let protocol = ip_header.protocol;

    debug!(
        "IP: received packet from {}.{}.{}.{} to {}.{}.{}.{}",
        (src_addr >> 24) & 0xFF,
        (src_addr >> 16) & 0xFF,
        (src_addr >> 8) & 0xFF,
        src_addr & 0xFF,
        (dst_addr >> 24) & 0xFF,
        (dst_addr >> 16) & 0xFF,
        (dst_addr >> 8) & 0xFF,
        dst_addr & 0xFF
    );

    if is_local_ip(dst_addr) {
        let padding = skb.len() - total_len;
        if padding != 0 {
            let _ = skb.trim(padding);
        }
        skb.pull(ihl);

        match protocol {
            1 => {
                debug!("IP: dispatching to ICMP");
                let _ = deliver_raw_packet(1, skb.clone(), src_addr);
                icmp_rcv(skb, src_addr, dst_addr)
            }
            17 => {
                debug!("IP: dispatching to UDP");
                let _ = deliver_raw_packet(17, skb.clone(), src_addr);
                udp_rcv(skb, src_addr, dst_addr)
            }
            6 => {
                debug!("IP: dispatching to TCP");
                let _ = deliver_raw_packet(6, skb.clone(), src_addr);
                tcp_rcv(skb, src_addr, dst_addr)
            }
            proto => {
                if deliver_raw_packet(proto, skb.clone(), src_addr) {
                    Ok((skb, src_addr, 0))
                } else {
                    log::debug!("IP: unsupported protocol {}", proto);
                    Err("Unsupported protocol")
                }
            }
        }
    } else {
        log::debug!(
            "IP: packet for {}.{}.{}.{} is not local",
            (dst_addr >> 24) & 0xFF,
            (dst_addr >> 16) & 0xFF,
            (dst_addr >> 8) & 0xFF,
            dst_addr & 0xFF
        );
        Err("Not for local")
    }
}

/// IP 发送
pub fn ip_queue_xmit(
    mut skb: Skb,
    src: u32,
    dst: u32,
    protocol: u8,
) -> Result<(Skb, u32, u16), &'static str> {
    let header_size = core::mem::size_of::<Ipv4Header>();
    skb.reserve_head(header_size);

    let ip_header_slice = match skb.push(header_size) {
        Some(slice) => slice,
        None => return Err("Failed to push IP header"),
    };

    {
        let ip_header = unsafe { &mut *(ip_header_slice.as_mut_ptr() as *mut Ipv4Header) };
        ip_header.set_version_ihl();
        ip_header.tos = 0;
        ip_header.set_total_len(skb.len() as u16);
        ip_header.id = ((fast_random() & 0xFFFF) as u16).to_be();
        ip_header.flags_frag = 0;
        ip_header.ttl = 64;
        ip_header.checksum = 0;
        ip_header.protocol = protocol;
        ip_header.src_addr = src.to_be();
        ip_header.dst_addr = dst.to_be();
    }

    let checksum = ip_checksum(&skb.data()[..header_size]);
    skb.data_mut()[10..12].copy_from_slice(&checksum.to_be_bytes());
    let verify = ip_checksum(&skb.data()[..header_size]);
    debug!(
        "IP TX: total_len={} protocol={} checksum={:#06x} verify={:#06x}",
        skb.len(),
        protocol,
        checksum,
        verify,
    );
    if verify != 0 {
        return Err("IP checksum self-check failed");
    }

    let (dev, nexthop) = match route_lookup(dst) {
        Ok(ret) => ret,
        Err(e) => {
            debug!(
                "IP: route lookup failed for {}.{}.{}.{}: {}",
                (dst >> 24) & 0xFF,
                (dst >> 16) & 0xFF,
                (dst >> 8) & 0xFF,
                dst & 0xFF,
                e
            );
            return Err(e);
        }
    };
    skb.dev = Some(dev.clone());

    neighbour_output(skb, nexthop, dev)
}

/// 简单的随机数生成器
fn fast_random() -> u32 {
    static mut STATE: u32 = 0x12345678;
    unsafe {
        STATE = STATE.wrapping_mul(1103515245).wrapping_add(12345);
        STATE
    }
}
