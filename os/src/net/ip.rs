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
use log::{error, info};
use polyhal::println;
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

/// IP 校验和计算
#[allow(unused)]
fn ip_fast_csum(words: &[u16]) -> u16 {
    let mut sum = 0u32;
    for &word in words {
        sum += word as u32;
        if sum >> 16 != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
    }
    !sum as u16
}

// ========== 新增：本机 IP 地址管理 ==========
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
    info!("IP: received packet of {} bytes", skb.len());
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

    let words = unsafe { core::slice::from_raw_parts(skb.data().as_ptr() as *const u16, ihl / 2) };
    if ip_fast_csum(words) != 0 {
        return Err("Invalid IP checksum");
    }

    let src_addr = ip_header.src_addr();
    let dst_addr = ip_header.dst_addr();

    error!(
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

    // 修改：使用 is_local_ip 函数检查
    if is_local_ip(dst_addr) {
        skb.pull(ihl);

        match ip_header.protocol {
            1 => {
                info!("IP: dispatching to ICMP");
                let _ = deliver_raw_packet(1, skb.clone());
                icmp_rcv(skb, src_addr, dst_addr)
            }
            17 => {
                info!("IP: dispatching to UDP");
                let _ = deliver_raw_packet(17, skb.clone());
                udp_rcv(skb, src_addr, dst_addr)
            }
            6 => {
                info!("IP: dispatching to TCP");
                let _ = deliver_raw_packet(6, skb.clone());
                tcp_rcv(skb, src_addr, dst_addr)
            }
            proto => {
                if deliver_raw_packet(proto, skb.clone()) {
                    Ok((skb, src_addr, 0))
                } else {
                    log::warn!("IP: unsupported protocol {}", proto);
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
    println!(
        "IP: sending packet from {}.{}.{}.{} to {}.{}.{}.{} proto {}",
        (src >> 24) & 0xFF,
        (src >> 16) & 0xFF,
        (src >> 8) & 0xFF,
        src & 0xFF,
        (dst >> 24) & 0xFF,
        (dst >> 16) & 0xFF,
        (dst >> 8) & 0xFF,
        dst & 0xFF,
        protocol
    );

    let header_size = core::mem::size_of::<Ipv4Header>();
    skb.reserve_head(header_size);

    let ip_header_slice = match skb.push(header_size) {
        Some(slice) => slice,
        None => return Err("Failed to push IP header"),
    };

    let ip_header = unsafe { &mut *(ip_header_slice.as_mut_ptr() as *mut Ipv4Header) };
    ip_header.set_version_ihl();
    ip_header.tos = 0;
    ip_header.set_total_len(skb.len() as u16);
    ip_header.id = (fast_random() & 0xFFFF) as u16;
    ip_header.flags_frag = 0;
    ip_header.ttl = 64;
    ip_header.checksum = 0;
    ip_header.protocol = protocol;
    ip_header.src_addr = src.to_be();
    ip_header.dst_addr = dst.to_be();

    let words = unsafe {
        core::slice::from_raw_parts(ip_header as *const _ as *const u16, header_size / 2)
    };
    ip_header.checksum = ip_fast_csum(words);

    let (dev, nexthop) = match route_lookup(dst) {
        Ok(ret) => ret,
        Err(e) => {
            error!(
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

    // 修改：使用 neighbour_output 进行邻居解析和链路层封装
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
