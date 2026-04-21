#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{close, recvfrom, sendto, sleep, socket};

const AF_INET: i32 = 2;
const SOCK_RAW: i32 = 3;
const IPPROTO_TCP: i32 = 6;

const LOOPBACK_IP: u32 = 0x7F000001;
const SERVER_PORT: u16 = 8080;
const CLIENT_PORT: u16 = 40000;

const TCP_FLAG_FIN: u8 = 0x01;
const TCP_FLAG_SYN: u8 = 0x02;
const TCP_FLAG_PSH: u8 = 0x08;
const TCP_FLAG_ACK: u8 = 0x10;

#[repr(C)]
#[derive(Clone, Copy)]
struct SockAddrIn {
    sin_family: u16,
    sin_port: u16,
    sin_addr: u32,
    sin_zero: [u8; 8],
}

impl SockAddrIn {
    fn new(ip: u32, port: u16) -> Self {
        Self {
            sin_family: AF_INET as u16,
            sin_port: port.to_be(),
            sin_addr: ip,
            sin_zero: [0; 8],
        }
    }
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct TcpHeader {
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    data_offset_reserved: u8,
    flags: u8,
    window: u16,
    checksum: u16,
    urgent_ptr: u16,
}

impl TcpHeader {
    fn size() -> usize {
        core::mem::size_of::<TcpHeader>()
    }

    fn header_len(&self) -> usize {
        ((self.data_offset_reserved >> 4) as usize) * 4
    }
}

fn checksum_fold(mut sum: u32) -> u16 {
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

fn tcp_checksum(src_ip: u32, dst_ip: u32, segment: &[u8]) -> u16 {
    let mut sum: u32 = 0;

    sum += ((src_ip >> 16) & 0xFFFF) as u32;
    sum += (src_ip & 0xFFFF) as u32;
    sum += ((dst_ip >> 16) & 0xFFFF) as u32;
    sum += (dst_ip & 0xFFFF) as u32;
    sum += 6u32;
    sum += segment.len() as u32;

    let mut i = 0usize;
    while i + 1 < segment.len() {
        let word = ((segment[i] as u16) << 8) | (segment[i + 1] as u16);
        sum += word as u32;
        i += 2;
    }
    if i < segment.len() {
        sum += (segment[i] as u32) << 8;
    }

    checksum_fold(sum)
}

fn send_tcp(
    fd: usize,
    src_ip: u32,
    dst_ip: u32,
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    payload: &[u8],
) -> bool {
    let mut packet = [0u8; 256];
    let total = TcpHeader::size() + payload.len();
    if total > packet.len() {
        return false;
    }

    for b in packet[..total].iter_mut() {
        *b = 0;
    }

    let hdr = unsafe { &mut *(packet.as_mut_ptr() as *mut TcpHeader) };
    hdr.src_port = src_port.to_be();
    hdr.dst_port = dst_port.to_be();
    hdr.seq = seq.to_be();
    hdr.ack = ack.to_be();
    hdr.data_offset_reserved = 5 << 4;
    hdr.flags = flags;
    hdr.window = 4096u16.to_be();
    hdr.checksum = 0;
    hdr.urgent_ptr = 0;

    if !payload.is_empty() {
        packet[TcpHeader::size()..total].copy_from_slice(payload);
    }

    let csum = tcp_checksum(src_ip, dst_ip, &packet[..total]);
    let hdr = unsafe { &mut *(packet.as_mut_ptr() as *mut TcpHeader) };
    hdr.checksum = csum.to_be();

    let dst = SockAddrIn::new(dst_ip, 0);
    let ret = sendto(
        fd,
        packet.as_ptr(),
        total,
        0,
        &dst as *const SockAddrIn as *const u8,
        core::mem::size_of::<SockAddrIn>(),
    );
    ret >= 0
}

fn recv_tcp(fd: usize, out: &mut [u8], max_try: usize) -> Option<usize> {
    let mut src = SockAddrIn::new(0, 0);
    let mut src_len: usize;

    for _ in 0..max_try {
        src_len = core::mem::size_of::<SockAddrIn>();
        let n = recvfrom(
            fd,
            out.as_mut_ptr(),
            out.len(),
            0,
            &mut src as *mut SockAddrIn as *mut u8,
            &mut src_len as *mut usize,
        );
        if n > 0 {
            return Some(n as usize);
        }
        sleep(1);
    }
    None
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[TCP-TEST] start");

    let fd = socket(AF_INET, SOCK_RAW, IPPROTO_TCP);
    if fd < 0 {
        println!("[TCP-TEST] socket failed: {}", fd);
        return -1;
    }
    let fd = fd as usize;

    let mut client_seq = 0x1122_3344u32;
    let mut server_seq: u32;

    if !send_tcp(
        fd,
        LOOPBACK_IP,
        LOOPBACK_IP,
        CLIENT_PORT,
        SERVER_PORT,
        client_seq,
        0,
        TCP_FLAG_SYN,
        &[],
    ) {
        println!("[TCP-TEST] send SYN failed");
        let _ = close(fd);
        return -1;
    }
    println!("[TCP-TEST] SYN sent");

    let mut buf = [0u8; 512];
    let syn_ack_len = loop {
        let Some(n) = recv_tcp(fd, &mut buf, 200) else {
            println!("[TCP-TEST] timeout waiting SYN-ACK");
            let _ = close(fd);
            return -1;
        };
        if n < TcpHeader::size() {
            continue;
        }
        let hdr = unsafe { &*(buf.as_ptr() as *const TcpHeader) };
        let src_port = u16::from_be(hdr.src_port);
        let dst_port = u16::from_be(hdr.dst_port);
        if src_port == SERVER_PORT
            && dst_port == CLIENT_PORT
            && (hdr.flags & (TCP_FLAG_SYN | TCP_FLAG_ACK)) == (TCP_FLAG_SYN | TCP_FLAG_ACK)
        {
            break n;
        }
    };

    let syn_ack_hdr = unsafe { &*(buf.as_ptr() as *const TcpHeader) };
    if syn_ack_len < syn_ack_hdr.header_len() {
        println!("[TCP-TEST] invalid SYN-ACK length");
        let _ = close(fd);
        return -1;
    }
    server_seq = u32::from_be(syn_ack_hdr.seq).wrapping_add(1);
    client_seq = client_seq.wrapping_add(1);
    println!("[TCP-TEST] SYN-ACK received");

    if !send_tcp(
        fd,
        LOOPBACK_IP,
        LOOPBACK_IP,
        CLIENT_PORT,
        SERVER_PORT,
        client_seq,
        server_seq,
        TCP_FLAG_ACK,
        &[],
    ) {
        println!("[TCP-TEST] send ACK failed");
        let _ = close(fd);
        return -1;
    }
    println!("[TCP-TEST] handshake done");

    let payload = b"hello-tcp";
    if !send_tcp(
        fd,
        LOOPBACK_IP,
        LOOPBACK_IP,
        CLIENT_PORT,
        SERVER_PORT,
        client_seq,
        server_seq,
        TCP_FLAG_ACK | TCP_FLAG_PSH,
        payload,
    ) {
        println!("[TCP-TEST] send data failed");
        let _ = close(fd);
        return -1;
    }
    client_seq = client_seq.wrapping_add(payload.len() as u32);

    let mut got_echo = false;
    for _ in 0..400 {
        let Some(n) = recv_tcp(fd, &mut buf, 10) else {
            continue;
        };
        if n < TcpHeader::size() {
            continue;
        }
        let hdr = unsafe { &*(buf.as_ptr() as *const TcpHeader) };
        let src_port = u16::from_be(hdr.src_port);
        let dst_port = u16::from_be(hdr.dst_port);
        if src_port != SERVER_PORT || dst_port != CLIENT_PORT {
            continue;
        }

        let hdr_len = hdr.header_len();
        if n < hdr_len {
            continue;
        }
        let data = &buf[hdr_len..n];

        if (hdr.flags & TCP_FLAG_ACK) != 0 && u32::from_be(hdr.ack) == client_seq {
            // ACK-only or ACK with data, keep scanning for echo data.
        }

        if !data.is_empty() {
            if data == payload {
                let seq_start = u32::from_be(hdr.seq);
                server_seq = seq_start.wrapping_add(data.len() as u32);
                got_echo = true;
                break;
            }
        }
    }

    if !got_echo {
        println!("[TCP-TEST] echo payload not received");
        let _ = close(fd);
        return -1;
    }

    if !send_tcp(
        fd,
        LOOPBACK_IP,
        LOOPBACK_IP,
        CLIENT_PORT,
        SERVER_PORT,
        client_seq,
        server_seq,
        TCP_FLAG_ACK,
        &[],
    ) {
        println!("[TCP-TEST] send echo ACK failed");
        let _ = close(fd);
        return -1;
    }
    println!("[TCP-TEST] data exchange done");

    if !send_tcp(
        fd,
        LOOPBACK_IP,
        LOOPBACK_IP,
        CLIENT_PORT,
        SERVER_PORT,
        client_seq,
        server_seq,
        TCP_FLAG_FIN | TCP_FLAG_ACK,
        &[],
    ) {
        println!("[TCP-TEST] send FIN failed");
        let _ = close(fd);
        return -1;
    }
    client_seq = client_seq.wrapping_add(1);

    let mut got_server_fin = false;
    for _ in 0..600 {
        let Some(n) = recv_tcp(fd, &mut buf, 10) else {
            continue;
        };
        if n < TcpHeader::size() {
            continue;
        }
        let hdr = unsafe { &*(buf.as_ptr() as *const TcpHeader) };
        let src_port = u16::from_be(hdr.src_port);
        let dst_port = u16::from_be(hdr.dst_port);
        if src_port != SERVER_PORT || dst_port != CLIENT_PORT {
            continue;
        }

        if (hdr.flags & TCP_FLAG_FIN) != 0 {
            let fin_seq = u32::from_be(hdr.seq);
            server_seq = fin_seq.wrapping_add(1);
            got_server_fin = true;
            break;
        }
    }

    if !got_server_fin {
        println!("[TCP-TEST] timeout waiting server FIN");
        let _ = close(fd);
        return -1;
    }

    if !send_tcp(
        fd,
        LOOPBACK_IP,
        LOOPBACK_IP,
        CLIENT_PORT,
        SERVER_PORT,
        client_seq,
        server_seq,
        TCP_FLAG_ACK,
        &[],
    ) {
        println!("[TCP-TEST] send final ACK failed");
        let _ = close(fd);
        return -1;
    }

    println!("[TCP-TEST] close handshake done");
    let _ = close(fd);
    0
}
