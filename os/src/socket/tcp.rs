#![allow(missing_docs)]

use crate::net::route::route_lookup;
use crate::net::tcp::tcp_send_segment;
use crate::task::suspend_current_and_run_next;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU16, AtomicU32, Ordering};
use lazy_static::lazy_static;
use polyhal::println;
use spin::Mutex;
const TCP_FLAG_FIN: u8 = 0x01;
const TCP_FLAG_SYN: u8 = 0x02;
const TCP_FLAG_PSH: u8 = 0x08;
const TCP_FLAG_ACK: u8 = 0x10;

static NEXT_ISS: AtomicU32 = AtomicU32::new(0x4000_0000);
static NEXT_EPHEMERAL_PORT: AtomicU16 = AtomicU16::new(40000);

lazy_static! {
    static ref LISTENERS: Mutex<Vec<(u32, u16, Arc<Mutex<TcpSocket>>)>> = Mutex::new(Vec::new());
    static ref CONNECTIONS: Mutex<Vec<((u32, u16, u32, u16), Arc<Mutex<TcpSocket>>)>> =
        Mutex::new(Vec::new());
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TcpSocketState {
    Open,
    Bound,
    Listening,
    SynSent,
    SynReceived,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    LastAck,
    Closed,
}

pub struct TcpSocket {
    pub local_addr: Option<(u32, u16)>,
    pub remote_addr: Option<(u32, u16)>,
    pub state: TcpSocketState,
    pub send_seq: u32,
    pub recv_seq: u32,
    pub receive_queue: Mutex<VecDeque<(Vec<u8>, u32, u16)>>,
    pub accept_queue: Mutex<VecDeque<Arc<Mutex<TcpSocket>>>>,
}

impl TcpSocket {
    pub fn new() -> Self {
        Self {
            local_addr: None,
            remote_addr: None,
            state: TcpSocketState::Open,
            send_seq: NEXT_ISS.fetch_add(0x1000, Ordering::Relaxed),
            recv_seq: 0,
            receive_queue: Mutex::new(VecDeque::new()),
            accept_queue: Mutex::new(VecDeque::new()),
        }
    }

    pub fn bind(&mut self, addr: u32, port: u16) -> Result<(), &'static str> {
        if self.local_addr.is_some() {
            return Err("TCP socket already bound");
        }
        self.local_addr = Some((addr, port));
        self.state = TcpSocketState::Bound;
        Ok(())
    }

    pub fn listen(&mut self, backlog: usize) -> Result<(), &'static str> {
        if self.local_addr.is_none() {
            return Err("TCP socket not bound");
        }
        if backlog == 0 {
            return Err("backlog must be > 0");
        }
        self.state = TcpSocketState::Listening;
        Ok(())
    }

    pub fn recv_from(&self, buf: &mut [u8]) -> Result<(usize, u32, u16), &'static str> {
        let mut queue = self.receive_queue.lock();
        let Some((payload, src_ip, src_port)) = queue.pop_front() else {
            return Err("No data");
        };

        let copy_len = core::cmp::min(buf.len(), payload.len());
        buf[..copy_len].copy_from_slice(&payload[..copy_len]);
        Ok((copy_len, src_ip, src_port))
    }

    pub fn send_to(
        &self,
        data: &[u8],
        dst_addr: u32,
        dst_port: u16,
    ) -> Result<usize, &'static str> {
        let (local_ip, local_port, remote_ip, remote_port) =
            match (self.local_addr, self.remote_addr) {
                (Some(local), Some(remote)) => (local.0, local.1, remote.0, remote.1),
                _ => return Err("TCP socket not connected"),
            };

        if dst_addr != 0 && (dst_addr != remote_ip || (dst_port != 0 && dst_port != remote_port)) {
            return Err("TCP destination mismatch");
        }

        if data.is_empty() {
            return Ok(0);
        }

        let seq = self.send_seq;
        let ack = self.recv_seq;
        tcp_send_segment(
            local_ip,
            remote_ip,
            local_port,
            remote_port,
            seq,
            ack,
            TCP_FLAG_ACK | TCP_FLAG_PSH,
            data,
        )?;

        let mut next_seq = self.send_seq;
        next_seq = next_seq.wrapping_add(data.len() as u32);
        unsafe {
            let this = self as *const Self as *mut Self;
            (*this).send_seq = next_seq;
        }

        Ok(data.len())
    }

    pub fn close(&mut self) -> Result<(), &'static str> {
        if self.state == TcpSocketState::Closed {
            return Ok(());
        }

        if let (Some((local_ip, local_port)), Some((remote_ip, remote_port))) =
            (self.local_addr, self.remote_addr)
        {
            if matches!(
                self.state,
                TcpSocketState::Established | TcpSocketState::CloseWait | TcpSocketState::FinWait1
            ) {
                let _ = tcp_send_segment(
                    local_ip,
                    remote_ip,
                    local_port,
                    remote_port,
                    self.send_seq,
                    self.recv_seq,
                    TCP_FLAG_FIN | TCP_FLAG_ACK,
                    &[],
                );
                self.send_seq = self.send_seq.wrapping_add(1);
                self.state = TcpSocketState::LastAck;
            }
        }

        unregister_socket(self.local_addr, self.remote_addr);
        self.receive_queue.lock().clear();
        self.accept_queue.lock().clear();
        self.state = TcpSocketState::Closed;
        Ok(())
    }
}

fn alloc_ephemeral_port() -> u16 {
    NEXT_EPHEMERAL_PORT.fetch_add(1, Ordering::Relaxed)
}

fn register_listener(listener: Arc<Mutex<TcpSocket>>) -> Result<(), &'static str> {
    let addr = listener.lock().local_addr.ok_or("listener not bound")?;
    let mut table = LISTENERS.lock();
    if table
        .iter()
        .any(|(ip, port, sock)| *ip == addr.0 && *port == addr.1 && Arc::ptr_eq(sock, &listener))
    {
        return Ok(());
    }
    table.push((addr.0, addr.1, listener));
    Ok(())
}

fn unregister_socket(local_addr: Option<(u32, u16)>, remote_addr: Option<(u32, u16)>) {
    if let Some((ip, port)) = local_addr {
        LISTENERS
            .lock()
            .retain(|(lip, lport, _)| !(*lip == ip && *lport == port));
    }
    if let (Some((lip, lport)), Some((rip, rport))) = (local_addr, remote_addr) {
        CONNECTIONS.lock().retain(|(key, _)| {
            let (src_ip, src_port, dst_ip, dst_port) = *key;
            !(src_ip == rip && src_port == rport && dst_ip == lip && dst_port == lport)
        });
    }
}

fn register_connection(socket: Arc<Mutex<TcpSocket>>) -> Result<(), &'static str> {
    let sock = socket.lock();
    let (local_ip, local_port) = sock.local_addr.ok_or("tcp socket not bound")?;
    let (remote_ip, remote_port) = sock.remote_addr.ok_or("tcp socket not connected")?;
    drop(sock);

    let mut table = CONNECTIONS.lock();
    let key = (remote_ip, remote_port, local_ip, local_port);

    if let Some((_, existing)) = table.iter_mut().find(|(k, _)| *k == key) {
        *existing = socket;
        return Ok(());
    }

    table.push((key, socket));
    Ok(())
}

fn find_connection(
    src_ip: u32,
    src_port: u16,
    dst_ip: u32,
    dst_port: u16,
) -> Option<Arc<Mutex<TcpSocket>>> {
    CONNECTIONS
        .lock()
        .iter()
        .find(|(key, _)| {
            let (k_src_ip, k_src_port, k_dst_ip, k_dst_port) = *key;
            k_src_ip == src_ip
                && k_src_port == src_port
                && k_dst_ip == dst_ip
                && k_dst_port == dst_port
        })
        .map(|(_, sock)| sock.clone())
}

fn find_listener(dst_ip: u32, dst_port: u16) -> Option<Arc<Mutex<TcpSocket>>> {
    LISTENERS
        .lock()
        .iter()
        .find(|(ip, port, _)| *ip == dst_ip && *port == dst_port)
        .map(|(_, _, sock)| sock.clone())
}

pub fn connect(
    socket: Arc<Mutex<TcpSocket>>,
    remote_ip: u32,
    remote_port: u16,
) -> Result<(), &'static str> {
    {
        let mut sock = socket.lock();
        if sock.remote_addr.is_some() {
            return Err("TCP socket already connected");
        }
        if sock.local_addr.is_none() {
            let local_ip = if (remote_ip & 0xFF00_0000) == 0x7F00_0000 {
                0x7F00_0001
            } else {
                let (dev, _) = route_lookup(remote_ip)?;
                let ip = dev.ip_addr();
                if ip == 0 {
                    return Err("Source IP not configured");
                }
                ip
            };
            let port = alloc_ephemeral_port();
            sock.local_addr = Some((local_ip, port));
        }
        sock.remote_addr = Some((remote_ip, remote_port));
        sock.state = TcpSocketState::SynSent;
    }

    register_connection(socket.clone())?;

    let (local_ip, local_port, remote_ip, remote_port, seq) = {
        let sock = socket.lock();
        let (local_ip, local_port) = sock.local_addr.ok_or("tcp local addr missing")?;
        let (remote_ip, remote_port) = sock.remote_addr.ok_or("tcp remote addr missing")?;
        (local_ip, local_port, remote_ip, remote_port, sock.send_seq)
    };

    {
        let mut sock = socket.lock();
        sock.send_seq = sock.send_seq.wrapping_add(1);
    }

    tcp_send_segment(
        local_ip,
        remote_ip,
        local_port,
        remote_port,
        seq,
        0,
        TCP_FLAG_SYN,
        &[],
    )?;

    for _ in 0..500 {
        if socket.lock().state == TcpSocketState::Established {
            return Ok(());
        }
        suspend_current_and_run_next();
    }

    Err("TCP connect timeout")
}

pub fn listen(socket: Arc<Mutex<TcpSocket>>, backlog: usize) -> Result<(), &'static str> {
    {
        let mut sock = socket.lock();
        sock.listen(backlog)?;
    }
    register_listener(socket)
}

pub fn accept(socket: Arc<Mutex<TcpSocket>>) -> Option<Arc<Mutex<TcpSocket>>> {
    let child = {
        let sock = socket.lock();
        sock.accept_queue.lock().front().cloned()
    };

    if let Some(child) = child {
        println!("TCP accept pop child ptr={:p}", Arc::as_ptr(&child));
        socket.lock().accept_queue.lock().pop_front();
        return Some(child);
    }

    None
}

pub fn dispatch_tcp_segment(
    src_ip: u32,
    dst_ip: u32,
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    payload: &[u8],
) -> bool {
    if let Some(socket) = find_connection(src_ip, src_port, dst_ip, dst_port) {
        println!(
            "TCP dispatch hit conn ptr={:p} {}:{} -> {}:{}",
            Arc::as_ptr(&socket),
            src_ip,
            src_port,
            dst_ip,
            dst_port
        );
        let mut sock = socket.lock();
        match sock.state {
            TcpSocketState::SynSent => {
                if (flags & (TCP_FLAG_SYN | TCP_FLAG_ACK)) == (TCP_FLAG_SYN | TCP_FLAG_ACK)
                    && ack == sock.send_seq
                {
                    sock.recv_seq = seq.wrapping_add(1);
                    sock.state = TcpSocketState::Established;
                    let (local_ip, local_port) = sock.local_addr.unwrap();
                    let (remote_ip, remote_port) = sock.remote_addr.unwrap();
                    let send_seq = sock.send_seq;
                    let recv_seq = sock.recv_seq;
                    drop(sock);
                    let _ = tcp_send_segment(
                        local_ip,
                        remote_ip,
                        local_port,
                        remote_port,
                        send_seq,
                        recv_seq,
                        TCP_FLAG_ACK,
                        &[],
                    );
                    return true;
                }
            }
            TcpSocketState::SynReceived => {
                if (flags & TCP_FLAG_ACK) != 0 && ack == sock.send_seq {
                    sock.state = TcpSocketState::Established;
                    return true;
                }
            }
            TcpSocketState::Established => {
                if (flags & TCP_FLAG_FIN) != 0 {
                    sock.recv_seq = seq.wrapping_add(1);
                    let (local_ip, local_port) = sock.local_addr.unwrap();
                    let (remote_ip, remote_port) = sock.remote_addr.unwrap();
                    let send_seq = sock.send_seq;
                    let recv_seq = sock.recv_seq;
                    sock.state = TcpSocketState::CloseWait;
                    drop(sock);
                    let _ = tcp_send_segment(
                        local_ip,
                        remote_ip,
                        local_port,
                        remote_port,
                        send_seq,
                        recv_seq,
                        TCP_FLAG_ACK,
                        &[],
                    );
                    return true;
                }

                if !payload.is_empty() {
                    println!(
                        "TCP socket rx payload: {}:{} <- {}:{} len={} state={:?}",
                        dst_ip,
                        dst_port,
                        src_ip,
                        src_port,
                        payload.len(),
                        sock.state
                    );
                    sock.recv_seq = seq.wrapping_add(payload.len() as u32);
                    sock.receive_queue
                        .lock()
                        .push_back((payload.to_vec(), src_ip, src_port));
                    println!(
                        "TCP socket queue depth after push: {}",
                        sock.receive_queue.lock().len()
                    );
                    let (local_ip, local_port) = sock.local_addr.unwrap();
                    let (remote_ip, remote_port) = sock.remote_addr.unwrap();
                    let send_seq = sock.send_seq;
                    let recv_seq = sock.recv_seq;
                    drop(sock);
                    let _ = tcp_send_segment(
                        local_ip,
                        remote_ip,
                        local_port,
                        remote_port,
                        send_seq,
                        recv_seq,
                        TCP_FLAG_ACK,
                        &[],
                    );
                    return true;
                }
            }
            TcpSocketState::CloseWait
            | TcpSocketState::FinWait1
            | TcpSocketState::FinWait2
            | TcpSocketState::LastAck => {
                if (flags & TCP_FLAG_ACK) != 0 && ack == sock.send_seq {
                    sock.state = TcpSocketState::Closed;
                    return true;
                }
            }
            _ => {}
        }
        return true;
    }

    if (flags & TCP_FLAG_SYN) != 0 && (flags & TCP_FLAG_ACK) == 0 {
        if let Some(listener) = find_listener(dst_ip, dst_port) {
            println!(
                "TCP listener hit: {}:{} <- {}:{} (enqueue child)",
                dst_ip, dst_port, src_ip, src_port
            );
            let iss = NEXT_ISS.fetch_add(0x1000, Ordering::Relaxed);
            let child = Arc::new(Mutex::new(TcpSocket {
                local_addr: Some((dst_ip, dst_port)),
                remote_addr: Some((src_ip, src_port)),
                state: TcpSocketState::SynReceived,
                send_seq: iss.wrapping_add(1),
                recv_seq: seq.wrapping_add(1),
                receive_queue: Mutex::new(VecDeque::new()),
                accept_queue: Mutex::new(VecDeque::new()),
            }));

            let _ = register_connection(child.clone());
            let listener_guard = listener.lock();
            listener_guard.accept_queue.lock().push_back(child.clone());
            println!(
                "TCP listener queue depth after enqueue: {}",
                listener_guard.accept_queue.lock().len()
            );

            let _ = tcp_send_segment(
                dst_ip,
                src_ip,
                dst_port,
                src_port,
                iss,
                seq.wrapping_add(1),
                TCP_FLAG_SYN | TCP_FLAG_ACK,
                &[],
            );
            return true;
        }
    }

    false
}
