use crate::error::{SysError, SysResult};
use crate::mm::UserBuffer;
use crate::net::ethernet::EthernetHeader;
use crate::net::ip::Ipv4Header;
use crate::net::ip::ip_queue_xmit;
use crate::net::route::route_lookup;
use crate::net::skb::Skb;
use crate::net::udp::{UdpHeader, udp_checksum};
use alloc::collections::VecDeque;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU16, Ordering};
use spin::Mutex;

static NEXT_EPHEMERAL_PORT: AtomicU16 = AtomicU16::new(45000);
const DEFAULT_UDP_RCVBUF_LIMIT: usize = 4 * 1024 * 1024;

fn alloc_ephemeral_port() -> u16 {
    NEXT_EPHEMERAL_PORT.fetch_add(1, Ordering::Relaxed)
}

#[inline]
fn is_loopback_addr(addr: u32) -> bool {
    (addr & 0xFF00_0000) == 0x7F00_0000
}

#[allow(unused)]
/// UDP套接字
pub struct UdpSocket {
    local_addr: Option<(u32, u16)>,  // (IP地址, 端口) 主机字节序
    remote_addr: Option<(u32, u16)>, // 已 connect 的对端地址
    pub receive_queue: Mutex<VecDeque<(Skb, u32, u16)>>, // (数据包, 源IP, 源端口)
    rcvbuf_used: Mutex<usize>,       // 接收队列当前占用的字节数
    rcvbuf_limit: usize,             // 接收队列上限（默认64KB）
    waker: Mutex<Option<Weak<crate::task::TaskControlBlock>>>, // 等待 recvfrom 的任务
}
#[allow(unused)]
impl UdpSocket {
    pub fn new() -> Self {
        Self {
            local_addr: None,
            remote_addr: None,
            receive_queue: Mutex::new(VecDeque::new()),
            rcvbuf_used: Mutex::new(0),
            rcvbuf_limit: DEFAULT_UDP_RCVBUF_LIMIT,
            waker: Mutex::new(None),
        }
    }
    pub fn clear_queue(&mut self) {
        self.receive_queue.lock().clear();
        log::info!("RawSocket: cleared receive queue");
    }

    /// 绑定到本地地址和端口
    pub fn bind(&mut self, addr: u32, port: u16) -> SysResult<()> {
        if self.local_addr.is_some() {
            return Err(SysError::EADDRINUSE);
        }
        let chosen_port = if port == 0 {
            alloc_ephemeral_port()
        } else {
            port
        };
        self.local_addr = Some((addr, chosen_port));

        log::error!(
            "UDP: socket bound to {}.{}.{}.{}:{}",
            (addr >> 24) & 0xFF,
            (addr >> 16) & 0xFF,
            (addr >> 8) & 0xFF,
            addr & 0xFF,
            chosen_port
        );

        Ok(())
    }

    /// 发送数据到指定地址
    pub fn send_to(&self, data: &[u8], dst_addr: u32, dst_port: u16) -> SysResult<(Skb, u32, u16)> {
        let src = self.local_addr.ok_or(SysError::EINVAL)?;
        send_udp_packet(src, data, dst_addr, dst_port)
    }

    pub fn send_user_buffer_to(
        &self,
        buf: &UserBuffer,
        dst_addr: u32,
        dst_port: u16,
    ) -> SysResult<(Skb, u32, u16)> {
        let src = self.local_addr.ok_or(SysError::EINVAL)?;
        send_udp_user_buffer(src, buf, dst_addr, dst_port)
    }

    pub fn local_addr(&self) -> Option<(u32, u16)> {
        self.local_addr
    }

    pub fn remote_addr(&self) -> Option<(u32, u16)> {
        self.remote_addr
    }

    pub fn connect(&mut self, dst_addr: u32, dst_port: u16) -> Result<(), &'static str> {
        let (need_ip, need_port) = match self.local_addr {
            None => (true, true),
            Some((ip, port)) => (ip == 0, port == 0),
        };

        if need_ip || need_port {
            let local_ip = if (dst_addr & 0xFF00_0000) == 0x7F00_0000 {
                0x7F00_0001
            } else {
                let (dev, _) = route_lookup(dst_addr)?;
                let ip = dev.ip_addr();
                if ip == 0 {
                    return Err("Source IP not configured");
                }
                ip
            };
            let chosen_ip = if need_ip {
                local_ip
            } else {
                self.local_addr.unwrap().0
            };
            let chosen_port = if need_port {
                alloc_ephemeral_port()
            } else {
                self.local_addr.unwrap().1
            };
            self.local_addr = Some((chosen_ip, chosen_port));
        }
        self.remote_addr = Some((dst_addr, dst_port));
        Ok(())
    }

    pub fn ensure_local_for_dst(&mut self, dst_addr: u32) -> Result<(u32, u16), &'static str> {
        let (need_ip, need_port) = match self.local_addr {
            None => (true, true),
            Some((ip, port)) => (ip == 0, port == 0),
        };

        if need_ip || need_port {
            let local_ip = if (dst_addr & 0xFF00_0000) == 0x7F00_0000 {
                0x7F00_0001
            } else {
                let (dev, _) = route_lookup(dst_addr)?;
                let ip = dev.ip_addr();
                if ip == 0 {
                    return Err("Source IP not configured");
                }
                ip
            };
            let chosen_ip = if need_ip {
                local_ip
            } else {
                self.local_addr.unwrap().0
            };
            let chosen_port = if need_port {
                alloc_ephemeral_port()
            } else {
                self.local_addr.unwrap().1
            };
            self.local_addr = Some((chosen_ip, chosen_port));
        }

        self.local_addr.ok_or("Socket not bound")
    }
}

pub fn send_udp_packet(
    src: (u32, u16),
    data: &[u8],
    dst_addr: u32,
    dst_port: u16,
) -> SysResult<(Skb, u32, u16)> {
    if is_loopback_addr(dst_addr) {
        return send_udp_loopback_slice(src, data, dst_port);
    }

    // 分配 skb（UDP头 + 数据）
    let total_len = data.len() + UdpHeader::size();
    let headroom = EthernetHeader::size() + core::mem::size_of::<Ipv4Header>();
    let mut skb = Skb::with_headroom(headroom, total_len);

    // 填充 UDP 头
    let udp_header = unsafe {
        &mut *(skb
            .put(UdpHeader::size())
            .ok_or(SysError::ENOMEM)?
            .as_mut_ptr() as *mut UdpHeader)
    };
    udp_header.set_source_port(src.1); // 源端口（主机字节序）
    udp_header.set_dest_port(dst_port); // 目标端口（主机字节序）
    udp_header.set_length(total_len as u16);
    udp_header.checksum = 0;

    // 拷贝数据
    skb.put(data.len())
        .ok_or(SysError::ENOMEM)?
        .copy_from_slice(data);

    let checksum = udp_checksum(src.0, dst_addr, skb.data());
    let udp_header = unsafe { &mut *(skb.data_mut().as_mut_ptr() as *mut UdpHeader) };
    udp_header.checksum = checksum.to_be();

    // 交给 IP 层发送
    ip_queue_xmit(skb, src.0, dst_addr, 17).map_err(|_| SysError::ENETUNREACH) // IPPROTO_UDP = 17
}

pub fn send_udp_user_buffer(
    src: (u32, u16),
    buf: &UserBuffer,
    dst_addr: u32,
    dst_port: u16,
) -> SysResult<(Skb, u32, u16)> {
    if is_loopback_addr(dst_addr) {
        return send_udp_loopback_user_buffer(src, buf, dst_port);
    }

    let total_len = buf.len() + UdpHeader::size();
    let headroom = EthernetHeader::size() + core::mem::size_of::<Ipv4Header>();
    let mut skb = Skb::with_headroom(headroom, total_len);

    let udp_header = unsafe {
        &mut *(skb
            .put(UdpHeader::size())
            .ok_or(SysError::ENOMEM)?
            .as_mut_ptr() as *mut UdpHeader)
    };
    udp_header.set_source_port(src.1);
    udp_header.set_dest_port(dst_port);
    udp_header.set_length(total_len as u16);
    udp_header.checksum = 0;

    for part in buf.buffers.iter() {
        skb.put(part.len())
            .ok_or(SysError::ENOMEM)?
            .copy_from_slice(&part[..]);
    }

    let checksum = udp_checksum(src.0, dst_addr, skb.data());
    let udp_header = unsafe { &mut *(skb.data_mut().as_mut_ptr() as *mut UdpHeader) };
    udp_header.checksum = checksum.to_be();

    ip_queue_xmit(skb, src.0, dst_addr, 17).map_err(|_| SysError::ENETUNREACH)
}

fn deliver_udp_loopback_payload(
    src: (u32, u16),
    dst_port: u16,
    skb: Skb,
) -> SysResult<(Skb, u32, u16)> {
    if let Some(socket) = lookup_udp_socket(dst_port, src.0, src.1) {
        let sock = socket.lock();
        let payload_len = skb.len();
        if sock.can_receive(payload_len) {
            sock.enqueue(skb, src.0, src.1);
            sock.wake();
        }
        Ok((Skb::new(0), src.0, src.1))
    } else {
        Ok((Skb::new(0), src.0, src.1))
    }
}

fn send_udp_loopback_slice(
    src: (u32, u16),
    data: &[u8],
    dst_port: u16,
) -> SysResult<(Skb, u32, u16)> {
    let mut skb = Skb::new(data.len());
    skb.put(data.len())
        .ok_or(SysError::ENOMEM)?
        .copy_from_slice(data);
    deliver_udp_loopback_payload(src, dst_port, skb)
}

fn send_udp_loopback_user_buffer(
    src: (u32, u16),
    buf: &UserBuffer,
    dst_port: u16,
) -> SysResult<(Skb, u32, u16)> {
    let mut skb = Skb::new(buf.len());
    for part in buf.buffers.iter() {
        skb.put(part.len())
            .ok_or(SysError::ENOMEM)?
            .copy_from_slice(&part[..]);
    }
    deliver_udp_loopback_payload(src, dst_port, skb)
}

#[allow(unused)]
impl UdpSocket {
    /// 接收数据
    /// 返回: (接收长度, 源IP地址, 源端口)
    pub fn recv_from(&self, buf: &mut [u8]) -> SysResult<(usize, u32, u16)> {
        let mut queue = self.receive_queue.lock();
        if let Some((skb, src_ip, src_port)) = queue.pop_front() {
            let copy_len = core::cmp::min(buf.len(), skb.len());
            buf[..copy_len].copy_from_slice(&skb.data()[..copy_len]);
            *self.rcvbuf_used.lock() -= skb.len();
            // 清除 waker，因为已经收到数据
            *self.waker.lock() = None;
            Ok((copy_len, src_ip, src_port))
        } else {
            Err(SysError::EAGAIN)
        }
    }

    pub fn recv_user_buffer(&self, buf: &mut UserBuffer) -> SysResult<(usize, u32, u16)> {
        let mut queue = self.receive_queue.lock();
        if let Some((skb, src_ip, src_port)) = queue.pop_front() {
            let copy_len = core::cmp::min(buf.len(), skb.len());
            let mut copied = 0usize;
            for slice in buf.buffers.iter_mut() {
                if copied >= copy_len {
                    break;
                }
                let take = core::cmp::min(slice.len(), copy_len - copied);
                slice[..take].copy_from_slice(&skb.data()[copied..copied + take]);
                copied += take;
            }
            *self.rcvbuf_used.lock() -= skb.len();
            *self.waker.lock() = None;
            Ok((copy_len, src_ip, src_port))
        } else {
            Err(SysError::EAGAIN)
        }
    }

    /// 非阻塞接收
    pub fn try_recv_from(&self, buf: &mut [u8]) -> SysResult<(usize, u32, u16)> {
        let mut queue = self.receive_queue.lock();
        if let Some((skb, src_ip, src_port)) = queue.pop_front() {
            let copy_len = core::cmp::min(buf.len(), skb.len());
            buf[..copy_len].copy_from_slice(&skb.data()[..copy_len]);
            *self.rcvbuf_used.lock() -= skb.len();
            Ok((copy_len, src_ip, src_port))
        } else {
            Err(SysError::EAGAIN)
        }
    }

    /// 检查接收队列是否有足够空间容纳新数据
    pub fn can_receive(&self, len: usize) -> bool {
        *self.rcvbuf_used.lock() + len <= self.rcvbuf_limit
    }

    /// 将数据包加入接收队列
    pub fn enqueue(&self, skb: Skb, src_ip: u32, src_port: u16) {
        let len = skb.len();
        self.receive_queue.lock().push_back((skb, src_ip, src_port));
        *self.rcvbuf_used.lock() += len;
    }

    /// 设置等待 recvfrom 的任务 waker
    pub fn set_waker(&self, task: Option<Arc<crate::task::TaskControlBlock>>) {
        *self.waker.lock() = task.map(|task| Arc::downgrade(&task));
    }

    /// 唤醒等待 recvfrom 的任务
    pub fn wake(&self) {
        if let Some(task) = self.waker.lock().take().and_then(|task| task.upgrade()) {
            crate::task::wakeup_task(task);
        }
    }
}

impl Clone for UdpSocket {
    fn clone(&self) -> Self {
        Self {
            local_addr: self.local_addr,
            remote_addr: self.remote_addr,
            receive_queue: Mutex::new(VecDeque::new()),
            rcvbuf_used: Mutex::new(0),
            rcvbuf_limit: self.rcvbuf_limit,
            waker: Mutex::new(None),
        }
    }
}

/// 全局UDP socket表（端口 -> socket）
static UDP_SOCKETS: Mutex<Vec<(u16, Arc<Mutex<UdpSocket>>)>> = Mutex::new(Vec::new());

pub fn register_udp_socket(port: u16, socket: Arc<Mutex<UdpSocket>>) {
    let mut table = UDP_SOCKETS.lock();
    if table
        .iter()
        .any(|(p, s)| *p == port && Arc::ptr_eq(s, &socket))
    {
        return;
    }
    table.push((port, socket));
}

pub fn unregister_udp_socket(port: u16, socket: Arc<Mutex<UdpSocket>>) {
    let mut table = UDP_SOCKETS.lock();
    table.retain(|(p, s)| !(*p == port && Arc::ptr_eq(s, &socket)));
}

pub fn lookup_udp_socket(
    dst_port: u16,
    src_ip: u32,
    src_port: u16,
) -> Option<Arc<Mutex<UdpSocket>>> {
    let table = UDP_SOCKETS.lock();

    for (port, socket) in table.iter().rev() {
        if *port != dst_port {
            continue;
        }
        if socket.lock().remote_addr() == Some((src_ip, src_port)) {
            return Some(socket.clone());
        }
    }

    table
        .iter()
        .rev()
        .find(|(port, socket)| *port == dst_port && socket.lock().remote_addr().is_none())
        .map(|(_, socket)| socket.clone())
}
