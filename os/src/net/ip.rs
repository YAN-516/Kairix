use crate::net::device::XmitError;
use crate::net::icmp::icmp_rcv;
use crate::net::route::route_lookup;
use crate::net::skb::Skb;
use crate::net::udp::udp_rcv;

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
        self.version_ihl = (4 << 4) | 5; // IPv4, 5个32位字 = 20字节
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

/// IP 校验和计算（16位字数组）
#[allow(unused)]
fn ip_fast_csum(words: &[u16]) -> u16 {
    let mut sum = 0u32;
    for &word in words {
        sum += word as u32;
        // 处理进位
        if sum >> 16 != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
    }
    !sum as u16
}
#[allow(unused)]
/// IP 接收处理
pub fn ip_rcv(mut skb: Skb) -> Result<Skb, &'static str> {
    // 检查长度是否足够包含 IP 头
    if skb.len() < core::mem::size_of::<Ipv4Header>() {
        return Err("IP packet too short");
    }

    // 解析 IP 头（从数据开始处）
    let ip_header = unsafe { &*(skb.data().as_ptr() as *const Ipv4Header) };

    // 检查 IP 版本
    if ip_header.version() != 4 {
        return Err("Not IPv4");
    }

    // 获取 IP 头长度
    let ihl = ip_header.ihl() as usize;
    println!("{}", ihl);
    if skb.len() < ihl {
        return Err("IP header truncated");
    }
    // 验证 IP 头校验和
    let words = unsafe { core::slice::from_raw_parts(skb.data().as_ptr() as *const u16, ihl / 2) };
    // if ip_fast_csum(words) != 0 {
    //     return Err("Invalid IP checksum");
    // }
    //println!("enter ip rcv,protocol {:?}", ip_header.protocol);

    let src_addr = ip_header.src_addr();
    let dst_addr = ip_header.dst_addr();

    log::debug!(
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

    // 检查是否是本地地址（127.0.0.0/8）
    let is_local = (dst_addr & 0xFF000000) == 0x7F000000;

    if is_local {
        // 移除 IP 头，使 data() 指向传输层头
        skb.pull(ihl);

        // 根据协议分发到传输层
        match ip_header.protocol {
            1 => {
                log::debug!("IP: dispatching to ICMP");
                icmp_rcv(skb)
            }
            17 => {
                log::debug!("IP: dispatching to UDP");
                udp_rcv(skb, src_addr, dst_addr)
            }
            proto => {
                log::warn!("IP: unsupported protocol {}", proto);
                Err("Unsupported protocol")
            }
        }
    } else {
        log::warn!("IP: packet not for local address: {}", dst_addr);
        Err("Not for local")
    }
}

/// IP 发送
pub fn ip_queue_xmit(mut skb: Skb, src: u32, dst: u32, protocol: u8) -> Result<Skb, &'static str> {
    log::debug!(
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

    // 预留 IP 头空间
    let header_size = core::mem::size_of::<Ipv4Header>();
    skb.reserve_head(header_size);

    // 在头部添加 IP 头空间
    let ip_header_slice = match skb.push(header_size) {
        Some(slice) => slice,
        None => return Err("Failed to push IP header"),
    };

    // 填充 IP 头
    let ip_header = unsafe { &mut *(ip_header_slice.as_mut_ptr() as *mut Ipv4Header) };
    ip_header.set_version_ihl();
    ip_header.tos = 0;
    ip_header.set_total_len(skb.len() as u16);
    ip_header.id = (fast_random() & 0xFFFF) as u16;
    ip_header.flags_frag = 0;
    ip_header.ttl = 64;
    ip_header.protocol = protocol;
    ip_header.src_addr = src.to_be();
    ip_header.dst_addr = dst.to_be();

    // 计算 IP 头校验和
    let words = unsafe {
        core::slice::from_raw_parts(ip_header as *const _ as *const u16, header_size / 2)
    };
    ip_header.checksum = ip_fast_csum(words);

    // 路由查找获取输出设备
    let dev = route_lookup(dst).unwrap();

    skb.dev = Some(dev.clone());
    //print!("{:?}", skb.data);
    // 通过设备发送
    dev.hard_start_xmit(skb).map_err(|e| {
        let err_str: &'static str = e.into();
        log::error!("IP: send failed: {}", err_str);
        err_str
    })
}

/// 简单的随机数生成器（用于 IP 标识符）
fn fast_random() -> u32 {
    static mut STATE: u32 = 0x12345678;
    unsafe {
        STATE = STATE.wrapping_mul(1103515245).wrapping_add(12345);
        STATE
    }
}
