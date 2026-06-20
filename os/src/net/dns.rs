#![allow(dead_code)]

use crate::error::{SysError, SysResult};
use crate::socket::udp::{UdpSocket, register_udp_socket, send_udp_packet, unregister_udp_socket};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

const DNS_PORT: u16 = 53;
const DNS_TYPE_A: u16 = 1;
const DNS_TYPE_CNAME: u16 = 5;
const DNS_CLASS_IN: u16 = 1;
const DNS_MAX_PACKET: usize = 512;
const DNS_TIMEOUT_US: usize = 3_000_000;
const DNS_MAX_RECURSION: usize = 4;

struct UdpRegistration {
    port: u16,
    socket: Arc<Mutex<UdpSocket>>,
}

impl Drop for UdpRegistration {
    fn drop(&mut self) {
        unregister_udp_socket(self.port, self.socket.clone());
    }
}

pub fn default_server() -> u32 {
    crate::net::QEMU_USER_DNS_SERVER
}

pub fn resolve_ipv4(name: &str) -> SysResult<u32> {
    resolve_ipv4_with_server(name, default_server())
}

pub fn resolve_ipv4_with_server(name: &str, server: u32) -> SysResult<u32> {
    resolve_ipv4_inner(name, server, 0)
}

fn resolve_ipv4_inner(name: &str, server: u32, depth: usize) -> SysResult<u32> {
    if depth >= DNS_MAX_RECURSION {
        return Err(SysError::ELOOP);
    }
    if let Some(ip) = parse_ipv4_literal(name) {
        return Ok(ip);
    }

    let txid = next_txid();
    let query = build_query(name, txid)?;
    let socket = Arc::new(Mutex::new(UdpSocket::new()));
    {
        socket
            .lock()
            .connect(server, DNS_PORT)
            .map_err(|_| SysError::ENETUNREACH)?;
    }
    let src = socket.lock().local_addr().ok_or(SysError::EINVAL)?;
    register_udp_socket(src.1, socket.clone());
    let _registration = UdpRegistration {
        port: src.1,
        socket: socket.clone(),
    };

    send_udp_packet(src, &query, server, DNS_PORT)?;

    let deadline = crate::timer::get_time_us().saturating_add(DNS_TIMEOUT_US);
    let mut buf = [0u8; DNS_MAX_PACKET];
    loop {
        crate::net::poll_rx_all();
        match socket.lock().try_recv_from(&mut buf) {
            Ok((len, src_ip, src_port)) => {
                if src_ip != server || src_port != DNS_PORT {
                    continue;
                }
                match parse_response(&buf[..len], txid)? {
                    DnsAnswer::A(ip) => return Ok(ip),
                    DnsAnswer::Cname(cname) => {
                        return resolve_ipv4_inner(&cname, server, depth + 1);
                    }
                    DnsAnswer::NoData => return Err(SysError::ENODATA),
                }
            }
            Err(SysError::EAGAIN) => {
                if crate::timer::get_time_us() >= deadline {
                    return Err(SysError::ETIMEDOUT);
                }
                crate::task::suspend_current_and_run_next();
            }
            Err(err) => return Err(err),
        }
    }
}

enum DnsAnswer {
    A(u32),
    Cname(String),
    NoData,
}

fn build_query(name: &str, txid: u16) -> SysResult<Vec<u8>> {
    let mut packet = Vec::new();
    push_u16(&mut packet, txid);
    push_u16(&mut packet, 0x0100);
    push_u16(&mut packet, 1);
    push_u16(&mut packet, 0);
    push_u16(&mut packet, 0);
    push_u16(&mut packet, 0);
    push_name(&mut packet, name)?;
    push_u16(&mut packet, DNS_TYPE_A);
    push_u16(&mut packet, DNS_CLASS_IN);
    Ok(packet)
}

fn parse_response(packet: &[u8], txid: u16) -> SysResult<DnsAnswer> {
    if packet.len() < 12 {
        return Err(SysError::EINVAL);
    }
    if read_u16(packet, 0)? != txid {
        return Err(SysError::EINVAL);
    }
    let flags = read_u16(packet, 2)?;
    if flags & 0x8000 == 0 {
        return Err(SysError::EINVAL);
    }
    if flags & 0x000f != 0 {
        return Err(SysError::ENOENT);
    }

    let qdcount = read_u16(packet, 4)? as usize;
    let ancount = read_u16(packet, 6)? as usize;
    let mut offset = 12usize;
    for _ in 0..qdcount {
        offset = skip_name(packet, offset)?;
        offset = offset.checked_add(4).ok_or(SysError::EINVAL)?;
        if offset > packet.len() {
            return Err(SysError::EINVAL);
        }
    }

    let mut cname = None;
    for _ in 0..ancount {
        offset = skip_name(packet, offset)?;
        if offset.checked_add(10).ok_or(SysError::EINVAL)? > packet.len() {
            return Err(SysError::EINVAL);
        }
        let typ = read_u16(packet, offset)?;
        let class = read_u16(packet, offset + 2)?;
        let rdlen = read_u16(packet, offset + 8)? as usize;
        offset += 10;
        let rdata_end = offset.checked_add(rdlen).ok_or(SysError::EINVAL)?;
        if rdata_end > packet.len() {
            return Err(SysError::EINVAL);
        }

        if typ == DNS_TYPE_A && class == DNS_CLASS_IN && rdlen == 4 {
            return Ok(DnsAnswer::A(
                ((packet[offset] as u32) << 24)
                    | ((packet[offset + 1] as u32) << 16)
                    | ((packet[offset + 2] as u32) << 8)
                    | packet[offset + 3] as u32,
            ));
        }
        if typ == DNS_TYPE_CNAME && class == DNS_CLASS_IN {
            if let Ok((name, _)) = read_name(packet, offset) {
                cname = Some(name);
            }
        }
        offset = rdata_end;
    }

    if let Some(name) = cname {
        Ok(DnsAnswer::Cname(name))
    } else {
        Ok(DnsAnswer::NoData)
    }
}

fn push_name(packet: &mut Vec<u8>, name: &str) -> SysResult<()> {
    let trimmed = name.trim_end_matches('.');
    if trimmed.is_empty() || trimmed.len() > 253 {
        return Err(SysError::EINVAL);
    }
    for label in trimmed.split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err(SysError::EINVAL);
        }
        packet.push(label.len() as u8);
        packet.extend_from_slice(label.as_bytes());
    }
    packet.push(0);
    Ok(())
}

fn skip_name(packet: &[u8], offset: usize) -> SysResult<usize> {
    read_name(packet, offset).map(|(_, next)| next)
}

fn read_name(packet: &[u8], mut offset: usize) -> SysResult<(String, usize)> {
    let mut labels = Vec::new();
    let mut jumped = false;
    let mut next = offset;
    let mut guard = 0usize;

    loop {
        if offset >= packet.len() {
            return Err(SysError::EINVAL);
        }
        guard += 1;
        if guard > packet.len() {
            return Err(SysError::ELOOP);
        }
        let len = packet[offset];
        if len & 0xc0 == 0xc0 {
            if offset + 1 >= packet.len() {
                return Err(SysError::EINVAL);
            }
            let ptr = (((len & 0x3f) as usize) << 8) | packet[offset + 1] as usize;
            if !jumped {
                next = offset + 2;
            }
            offset = ptr;
            jumped = true;
            continue;
        }
        if len & 0xc0 != 0 {
            return Err(SysError::EINVAL);
        }
        offset += 1;
        if len == 0 {
            if !jumped {
                next = offset;
            }
            break;
        }
        let end = offset.checked_add(len as usize).ok_or(SysError::EINVAL)?;
        if end > packet.len() {
            return Err(SysError::EINVAL);
        }
        let label = core::str::from_utf8(&packet[offset..end]).map_err(|_| SysError::EINVAL)?;
        labels.push(label.to_string());
        offset = end;
    }

    Ok((labels.join("."), next))
}

fn read_u16(packet: &[u8], offset: usize) -> SysResult<u16> {
    if offset + 2 > packet.len() {
        return Err(SysError::EINVAL);
    }
    Ok(u16::from_be_bytes([packet[offset], packet[offset + 1]]))
}

fn push_u16(packet: &mut Vec<u8>, value: u16) {
    packet.extend_from_slice(&value.to_be_bytes());
}

fn next_txid() -> u16 {
    static TXID: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(0x4b58);
    TXID.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
}

fn parse_ipv4_literal(name: &str) -> Option<u32> {
    let mut ip = 0u32;
    let mut count = 0usize;
    for part in name.split('.') {
        if part.is_empty() || part.len() > 3 {
            return None;
        }
        let mut value = 0u32;
        for byte in part.bytes() {
            if !byte.is_ascii_digit() {
                return None;
            }
            value = value * 10 + (byte - b'0') as u32;
            if value > 255 {
                return None;
            }
        }
        ip = (ip << 8) | value;
        count += 1;
    }
    if count == 4 { Some(ip) } else { None }
}
