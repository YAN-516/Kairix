#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{bind, close, recvfrom, sendto, socket};

const AF_INET: i32 = 2;
const SOCK_DGRAM: i32 = 2;
const SOCK_RAW: i32 = 3;
const IPPROTO_UDP: i32 = 17;
const MSG_DONTWAIT: i32 = 0x40;
const EAGAIN_RET: isize = -11;
const LOOPBACK: u32 = 0x7F000001;
const UDP_PORT: u16 = 9301;
const RAW_SRC_PORT: u16 = 53117;

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
            sin_addr: ip.to_be(),
            sin_zero: [0; 8],
        }
    }
}

fn fold(mut sum: u32) -> u16 {
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

fn udp_checksum(src_ip: u32, dst_ip: u32, datagram: &[u8]) -> u16 {
    let mut sum = 0u32;
    sum += ((src_ip >> 16) & 0xFFFF) as u32;
    sum += (src_ip & 0xFFFF) as u32;
    sum += ((dst_ip >> 16) & 0xFFFF) as u32;
    sum += (dst_ip & 0xFFFF) as u32;
    sum += IPPROTO_UDP as u32;
    sum += datagram.len() as u32;

    let mut i = 0usize;
    while i + 1 < datagram.len() {
        sum += (((datagram[i] as u16) << 8) | datagram[i + 1] as u16) as u32;
        i += 2;
    }
    if i < datagram.len() {
        sum += (datagram[i] as u32) << 8;
    }

    let checksum = fold(sum);
    if checksum == 0 { 0xFFFF } else { checksum }
}

fn write_u16_be(buf: &mut [u8], off: usize, value: u16) {
    buf[off] = (value >> 8) as u8;
    buf[off + 1] = value as u8;
}

fn make_udp_datagram(payload: &[u8], checksum_good: bool, out: &mut [u8]) -> usize {
    let len = 8 + payload.len();
    for b in out[..len].iter_mut() {
        *b = 0;
    }
    write_u16_be(out, 0, RAW_SRC_PORT);
    write_u16_be(out, 2, UDP_PORT);
    write_u16_be(out, 4, len as u16);
    write_u16_be(out, 6, 0);
    out[8..8 + payload.len()].copy_from_slice(payload);

    let csum = udp_checksum(LOOPBACK, LOOPBACK, &out[..len]);
    let final_sum = if checksum_good {
        csum
    } else if csum == 0xFFFF {
        0x1234
    } else {
        csum ^ 0xFFFF
    };
    write_u16_be(out, 6, final_sum);
    len
}

fn expect(name: &str, ok: bool) -> bool {
    println!("[UDP-CSUM] {} {}", name, if ok { "ok" } else { "FAIL" });
    ok
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[UDP-CSUM] start");

    let udp_fd = socket(AF_INET, SOCK_DGRAM, 0);
    if udp_fd < 0 {
        let _ = expect("udp socket", false);
        return -1;
    }
    let raw_fd = socket(AF_INET, SOCK_RAW, IPPROTO_UDP);
    if raw_fd < 0 {
        let _ = close(udp_fd as usize);
        let _ = expect("raw socket", false);
        return -1;
    }

    let bind_addr = SockAddrIn::new(LOOPBACK, UDP_PORT);
    let bret = bind(
        udp_fd as usize,
        &bind_addr as *const SockAddrIn as *const u8,
        core::mem::size_of::<SockAddrIn>(),
    );
    if bret != 0 {
        let _ = close(raw_fd as usize);
        let _ = close(udp_fd as usize);
        let _ = expect("bind udp", false);
        return -1;
    }

    let dst = SockAddrIn::new(LOOPBACK, 0);
    let mut packet = [0u8; 64];
    let bad_payload = b"bad-sum";
    let bad_len = make_udp_datagram(bad_payload, false, &mut packet);
    let bad_send = sendto(
        raw_fd as usize,
        packet.as_ptr(),
        bad_len,
        0,
        &dst as *const SockAddrIn as *const u8,
        core::mem::size_of::<SockAddrIn>(),
    );

    let mut recv_buf = [0u8; 32];
    let mut src_addr = SockAddrIn::new(0, 0);
    let mut src_len = core::mem::size_of::<SockAddrIn>();
    let bad_recv = recvfrom(
        udp_fd as usize,
        recv_buf.as_mut_ptr(),
        recv_buf.len(),
        MSG_DONTWAIT,
        &mut src_addr as *mut SockAddrIn as *mut u8,
        &mut src_len as *mut usize,
    );

    let good_payload = b"good-sum";
    let good_len = make_udp_datagram(good_payload, true, &mut packet);
    let good_send = sendto(
        raw_fd as usize,
        packet.as_ptr(),
        good_len,
        0,
        &dst as *const SockAddrIn as *const u8,
        core::mem::size_of::<SockAddrIn>(),
    );
    let good_recv = recvfrom(
        udp_fd as usize,
        recv_buf.as_mut_ptr(),
        recv_buf.len(),
        MSG_DONTWAIT,
        &mut src_addr as *mut SockAddrIn as *mut u8,
        &mut src_len as *mut usize,
    );

    let mut ok = true;
    ok &= expect("bad checksum dropped", bad_send == bad_len as isize && bad_recv == EAGAIN_RET);
    ok &= expect(
        "good checksum delivered",
        good_send == good_len as isize
            && good_recv == good_payload.len() as isize
            && &recv_buf[..good_payload.len()] == good_payload,
    );

    let _ = close(raw_fd as usize);
    let _ = close(udp_fd as usize);
    println!("[UDP-CSUM] done ok={}", ok);
    if ok { 0 } else { -1 }
}
