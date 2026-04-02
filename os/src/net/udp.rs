use crate::net::ip::ip_queue_xmit;
use crate::net::skb::Skb;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;
/// UDP头结构
#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
#[allow(unused)]
pub struct UdpHeader {
    src_port: u16, // 源端口（网络字节序）
    dst_port: u16, // 目标端口（网络字节序）
    len: u16,      // UDP包长度（网络字节序）
    checksum: u16, // 校验和（网络字节序）
}
#[allow(unused)]
impl UdpHeader {
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
#[allow(unused)]
/// UDP套接字
pub struct UdpSocket {
    local_addr: Option<(u32, u16)>, // (IP地址, 端口) 主机字节序
    pub receive_queue: Mutex<VecDeque<(Skb, u32, u16)>>, // (数据包, 源IP, 源端口)
}
#[allow(unused)]
impl UdpSocket {
    pub fn new() -> Self {
        Self {
            local_addr: None,
            receive_queue: Mutex::new(VecDeque::new()),
        }
    }
    pub fn clear_queue(&mut self) {
        self.receive_queue.lock().clear();
        log::debug!("RawSocket: cleared receive queue");
    }

    /// 绑定到本地地址和端口
    pub fn bind(&mut self, addr: u32, port: u16) -> Result<(), &'static str> {
        if self.local_addr.is_some() {
            return Err("Already bound");
        }
        self.local_addr = Some((addr, port));

        // 注册到全局UDP socket表
        register_udp_socket(port, Arc::new(Mutex::new(self.clone())));

        println!(
            "UDP: socket bound to {}.{}.{}.{}:{}",
            (addr >> 24) & 0xFF,
            (addr >> 16) & 0xFF,
            (addr >> 8) & 0xFF,
            addr & 0xFF,
            port
        );

        Ok(())
    }

    /// 发送数据到指定地址
    pub fn send_to(
        &self,
        data: &[u8],
        dst_addr: u32,
        dst_port: u16,
    ) -> Result<(Skb, u32, u16), &'static str> {
        let src = self.local_addr.ok_or("Socket not bound")?;

        // 分配 skb（UDP头 + 数据）
        let total_len = data.len() + UdpHeader::size();
        let mut skb = Skb::new(total_len);

        // 填充 UDP 头
        let udp_header =
            unsafe { &mut *(skb.put(UdpHeader::size()).unwrap().as_mut_ptr() as *mut UdpHeader) };
        udp_header.set_source_port(src.1); // 源端口（主机字节序）
        udp_header.set_dest_port(dst_port); // 目标端口（主机字节序）
        udp_header.set_length(total_len as u16);
        udp_header.checksum = 0; // 简化：跳过校验和计算

        // 拷贝数据
        skb.put(data.len()).unwrap().copy_from_slice(data);

        // 交给 IP 层发送
        ip_queue_xmit(skb, src.0, dst_addr, 17) // IPPROTO_UDP = 17
    }

    /// 接收数据
    /// 返回: (接收长度, 源IP地址, 源端口)
    pub fn recv_from(&self, buf: &mut [u8]) -> Result<(usize, u32, u16), &'static str> {
        let mut queue = self.receive_queue.lock();
        if let Some((skb, src_ip, src_port)) = queue.pop_front() {
            let copy_len = core::cmp::min(buf.len(), skb.len());
            buf[..copy_len].copy_from_slice(&skb.data()[..copy_len]);

            Ok((copy_len, src_ip, src_port))
        } else {
            Err("No data")
        }
    }

    /// 非阻塞接收
    pub fn try_recv_from(&self, buf: &mut [u8]) -> Option<(usize, u32, u16)> {
        let mut queue = self.receive_queue.lock();
        if let Some((skb, src_ip, src_port)) = queue.pop_front() {
            let copy_len = core::cmp::min(buf.len(), skb.len());
            buf[..copy_len].copy_from_slice(&skb.data()[..copy_len]);
            Some((copy_len, src_ip, src_port))
        } else {
            None
        }
    }
}

impl Clone for UdpSocket {
    fn clone(&self) -> Self {
        Self {
            local_addr: self.local_addr,
            receive_queue: Mutex::new(VecDeque::new()),
        }
    }
}

/// 全局UDP socket表（端口 -> socket）
static UDP_SOCKETS: Mutex<Vec<(u16, Arc<Mutex<UdpSocket>>)>> = Mutex::new(Vec::new());

fn register_udp_socket(port: u16, socket: Arc<Mutex<UdpSocket>>) {
    UDP_SOCKETS.lock().push((port, socket));
}

fn lookup_udp_socket(port: u16) -> Option<Arc<Mutex<UdpSocket>>> {
    UDP_SOCKETS
        .lock()
        .iter()
        .find(|(p, _)| *p == port)
        .map(|(_, s)| s.clone())
}

/// UDP接收处理（由IP层调用）
pub fn udp_rcv(mut skb: Skb, src_ip: u32, _dst_ip: u32) -> Result<(Skb, u32, u16), &'static str> {
    // 检查长度
    if skb.len() < UdpHeader::size() {
        return Err("UDP packet too short");
    }

    // 解析 UDP 头
    let udp_header = unsafe { &*(skb.data().as_ptr() as *const UdpHeader) };

    let dst_port = udp_header.dest_port(); // 主机字节序
    let src_port = udp_header.source_port(); // 主机字节序
    // println!("{:?} {:?}", src_ip, dst_port);
    // 查找对应的 socket
    if let Some(_socket) = lookup_udp_socket(dst_port) {
        // 移除 UDP 头
        skb.pull(UdpHeader::size());

        log::debug!("UDP: delivered packet to socket on port {}", dst_port);
        Ok((skb, src_ip, src_port))
    } else {
        log::warn!("UDP: no socket for port {}", dst_port);
        Err("No socket")
    }
}
