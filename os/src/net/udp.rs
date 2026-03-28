use crate::net::ip::ip_queue_xmit;
use crate::net::skb::Skb;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;
/// UDP头结构
#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
pub struct UdpHeader {
    src_port: u16,
    dst_port: u16,
    len: u16,
    checksum: u16,
}

impl UdpHeader {
    pub fn size() -> usize {
        core::mem::size_of::<UdpHeader>()
    }
}

/// UDP套接字
pub struct UdpSocket {
    local_addr: Option<(u32, u16)>,
    receive_queue: Mutex<VecDeque<Skb>>,
}

impl UdpSocket {
    pub fn new() -> Self {
        Self {
            local_addr: None,
            receive_queue: Mutex::new(VecDeque::new()),
        }
    }

    pub fn bind(&mut self, addr: u32, port: u16) -> Result<(), &'static str> {
        if self.local_addr.is_some() {
            return Err("Already bound");
        }
        self.local_addr = Some((addr, port));

        // 注册到全局UDP socket表
        register_udp_socket(port, Arc::new(Mutex::new(self.clone())));

        Ok(())
    }

    pub fn send_to(&self, data: &[u8], dst_addr: u32, dst_port: u16) -> Result<(), &'static str> {
        let src = self.local_addr.ok_or("Socket not bound")?;

        // 分配skb
        let total_len = data.len() + UdpHeader::size();
        let mut skb = Skb::new(total_len);

        // 填充UDP头
        let udp_header =
            unsafe { &mut *(skb.put(UdpHeader::size()).unwrap().as_mut_ptr() as *mut UdpHeader) };
        udp_header.src_port = src.1.to_be();
        udp_header.dst_port = dst_port.to_be();
        udp_header.len = (total_len as u16).to_be();
        udp_header.checksum = 0; // 简化：跳过校验和

        // 拷贝数据
        skb.put(data.len()).unwrap().copy_from_slice(data);

        log::debug!(
            "UDP: sending {} bytes to {}:{}",
            data.len(),
            dst_addr,
            dst_port
        );

        // 交给IP层发送
        ip_queue_xmit(skb, src.0, dst_addr, 17) // IPPROTO_UDP = 17
    }

    pub fn recv_from(&self, buf: &mut [u8]) -> Result<(usize, u32, u16), &'static str> {
        let mut queue = self.receive_queue.lock();
        if let Some(skb) = queue.pop_front() {
            let udp_header = unsafe { &*(skb.data().as_ptr() as *const UdpHeader) };
            let data_len = skb.len - UdpHeader::size();
            let copy_len = core::cmp::min(buf.len(), data_len);
            let data_start = skb.data().as_ptr() as usize + UdpHeader::size();
            unsafe {
                core::ptr::copy_nonoverlapping(data_start as *const u8, buf.as_mut_ptr(), copy_len);
            }
            Ok((
                copy_len,
                u32::from_be(udp_header.src_port as u32),
                u16::from_be(udp_header.src_port),
            ))
        } else {
            Err("No data")
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

/// 全局UDP socket表
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

/// UDP接收处理
pub fn udp_rcv(mut skb: Skb) -> Result<(), &'static str> {
    if skb.data.len() < UdpHeader::size() {
        return Err("UDP packet too short");
    }

    let udp_header = unsafe { &*(skb.data().as_ptr() as *const UdpHeader) };

    let dst_port = u16::from_be(udp_header.dst_port);

    log::debug!("UDP: received packet for port {}", dst_port);

    if let Some(socket) = lookup_udp_socket(dst_port) {
        // 移除UDP头
        skb.pull(UdpHeader::size());

        // 放入socket接收队列
        socket.lock().receive_queue.lock().push_back(skb);

        Ok(())
    } else {
        log::warn!("UDP: no socket for port {}", dst_port);
        Err("No socket")
    }
}
