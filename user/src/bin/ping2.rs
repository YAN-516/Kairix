#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{bind, close, recvfrom, sendto, sleep, socket};

const AF_INET: i32 = 2;
const SOCK_DGRAM: i32 = 2;
const SOCK_RAW: i32 = 3;
const IPPROTO_ICMP: i32 = 1;

const LOOPBACK_IP: u32 = 0x7F000001;
const QEMU_GATEWAY_IP: u32 = 0x0A000202;
const QEMU_DNS_IP: u32 = 0x0A000203;
const PUBLIC_DNS_IP: u32 = 0x08080808;
const TEST_NET_UNREACHABLE_IP: u32 = 0xC0000201; // 192.0.2.1 (TEST-NET-1)

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

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("================ net_smoketest ================");

    let mut failed = 0;

    if !test_udp_loopback() {
        failed += 1;
    }
    if !test_icmp_loopback() {
        failed += 1;
    }
    if !test_icmp_gateway() {
        failed += 1;
    }
    if !test_icmp_extra_targets() {
        failed += 1;
    }

    if failed == 0 {
        println!("ALL TESTS PASSED");
        0
    } else {
        println!("FAILED TESTS: {}", failed);
        -1
    }
}

fn test_udp_loopback() -> bool {
    println!("[UDP] loopback send/recv start");

    let fd = socket(AF_INET, SOCK_DGRAM, 0);
    if fd < 0 {
        println!("[UDP] socket failed: {}", fd);
        return false;
    }

    let addr = SockAddrIn::new(LOOPBACK_IP, 5566);
    let bind_ret = bind(
        fd as usize,
        &addr as *const SockAddrIn as *const u8,
        core::mem::size_of::<SockAddrIn>(),
    );
    if bind_ret < 0 {
        println!("[UDP] bind failed: {}", bind_ret);
        let _ = close(fd as usize);
        return false;
    }

    let payload = b"udp-loopback-smoke";
    let send_ret = sendto(
        fd as usize,
        payload.as_ptr(),
        payload.len(),
        0,
        &addr as *const SockAddrIn as *const u8,
        core::mem::size_of::<SockAddrIn>(),
    );
    if send_ret < 0 {
        println!("[UDP] sendto failed: {}", send_ret);
        let _ = close(fd as usize);
        return false;
    }
    println!("[UDP] sent {} bytes to loopback", send_ret);
    sleep(20);

    let mut recv_buf = [0u8; 64];
    let mut src_addr = SockAddrIn::new(0, 0);
    let mut src_len = core::mem::size_of::<SockAddrIn>();
    let recv_ret = recvfrom(
        fd as usize,
        recv_buf.as_mut_ptr(),
        recv_buf.len(),
        0,
        &mut src_addr as *mut SockAddrIn as *mut u8,
        &mut src_len as *mut usize,
    );

    let _ = close(fd as usize);

    if recv_ret < 0 {
        println!("[UDP] recvfrom failed: {}", recv_ret);
        return false;
    }

    let got = &recv_buf[..recv_ret as usize];
    if got != payload {
        println!("[UDP] payload mismatch");
        return false;
    }

    println!("[UDP] loopback ok, recv {} bytes", recv_ret);
    true
}

fn test_icmp_loopback() -> bool {
    println!("[ICMP] loopback ping start");
    ping_once(LOOPBACK_IP, 0x1001, 1, "loopback")
}

fn test_icmp_gateway() -> bool {
    println!("[ICMP] gateway ping start (10.0.2.2)");
    if ping_once(QEMU_GATEWAY_IP, 0x1002, 1, "gateway") {
        return true;
    }

    // 首次探测可能因 ARP 建邻导致 echo 报文未真正发出，补发一次。
    println!("[ICMP] gateway first try missed, retry once after ARP warmup");
    sleep(20);
    ping_once(QEMU_GATEWAY_IP, 0x1002, 2, "gateway-retry")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PingExpect {
    MustReply,
    MustFail,
}

#[derive(Clone, Copy)]
struct PingCase {
    ip: u32,
    tag: &'static str,
    expect: PingExpect,
}

fn test_icmp_extra_targets() -> bool {
    println!("[ICMP] extra target matrix start");

    let cases = [
        // PingCase {
        //     ip: QEMU_GATEWAY_IP,
        //     tag: "gateway-recheck(10.0.2.2)",
        //     expect: PingExpect::MustReply,
        // },
        // PingCase {
        //     ip: QEMU_DNS_IP,
        //     tag: "qemu-dns(10.0.2.3)",
        //     expect: PingExpect::OptionalReply,
        // },
        PingCase {
            ip: PUBLIC_DNS_IP,
            tag: "public-dns(8.8.8.8)",
            expect: PingExpect::MustReply,
        },
        PingCase {
            ip: TEST_NET_UNREACHABLE_IP,
            tag: "test-net-unreachable(192.0.2.1)",
            expect: PingExpect::MustFail,
        },
    ];

    let mut hard_failed = false;

    for (idx, case) in cases.iter().enumerate() {
        let ok = ping_once(case.ip, 0x2000 + idx as u16, 1, case.tag);
        match case.expect {
            PingExpect::MustReply => {
                if !ok {
                    println!("[ICMP:{}] expected reply but got timeout", case.tag);
                    hard_failed = true;
                }
            }
            PingExpect::MustFail => {
                if ok {
                    println!("[ICMP:{}] unexpected reply", case.tag);
                    hard_failed = true;
                } else {
                    println!("[ICMP:{}] no reply as expected", case.tag);
                }
            }
        }
    }

    !hard_failed
}

fn ping_once(dst_ip: u32, ident: u16, seq: u16, tag: &str) -> bool {
    let fd = socket(AF_INET, SOCK_RAW, IPPROTO_ICMP);
    if fd < 0 {
        println!("[ICMP:{}] socket failed: {}", tag, fd);
        return false;
    }

    let mut packet = [0u8; 64];
    packet[0] = 8;
    packet[1] = 0;
    packet[4] = (ident >> 8) as u8;
    packet[5] = (ident & 0xFF) as u8;
    packet[6] = (seq >> 8) as u8;
    packet[7] = (seq & 0xFF) as u8;
    let mut i = 8;
    while i < packet.len() {
        packet[i] = (i - 8) as u8;
        i += 1;
    }

    let csum = icmp_csum(&packet);
    packet[2] = (csum >> 8) as u8;
    packet[3] = (csum & 0xFF) as u8;

    let dst = SockAddrIn::new(dst_ip, 0);
    let mut attempt = 0usize;
    loop {
        let send_ret = sendto(
            fd as usize,
            packet.as_ptr(),
            packet.len(),
            0,
            &dst as *const SockAddrIn as *const u8,
            core::mem::size_of::<SockAddrIn>(),
        );
        if send_ret >= 0 {
            if attempt > 0 {
                println!(
                    "[ICMP:{}] sendto succeeded after {} retries (ARP resolved)",
                    tag, attempt
                );
            }
            break;
        }

        if attempt == 0 {
            println!("[ICMP:{}] sendto pending, waiting for ARP resolution", tag);
        } else if attempt % 50 == 0 {
            println!(
                "[ICMP:{}] still waiting ARP for {}.{}.{}.{} (retry={})",
                tag,
                (dst_ip >> 24) & 0xFF,
                (dst_ip >> 16) & 0xFF,
                (dst_ip >> 8) & 0xFF,
                dst_ip & 0xFF,
                attempt
            );
        }
        attempt += 1;
        sleep(1);
    }

    let mut reply = [0u8; 128];
    let mut src_addr = SockAddrIn::new(0, 0);
    let mut src_len: usize;
    let mut wait_loops = 0usize;
    println!("[ICMP:{}] request sent, waiting reply (no timeout)", tag);
    loop {
        src_len = core::mem::size_of::<SockAddrIn>();
        let recv_ret = recvfrom(
            fd as usize,
            reply.as_mut_ptr(),
            reply.len(),
            0,
            &mut src_addr as *mut SockAddrIn as *mut u8,
            &mut src_len as *mut usize,
        );

        if recv_ret < 0 {
            wait_loops += 1;
            if wait_loops % 200 == 0 {
                println!(
                    "[ICMP:{}] still waiting echo reply from {}.{}.{}.{}",
                    tag,
                    (dst_ip >> 24) & 0xFF,
                    (dst_ip >> 16) & 0xFF,
                    (dst_ip >> 8) & 0xFF,
                    dst_ip & 0xFF
                );
            }
            sleep(1);
            continue;
        }

        let n = recv_ret as usize;
        if n < 8 {
            continue;
        }

        // RAW socket 可能先读到 echo request 或其他同协议报文，继续等待匹配的 echo reply。
        if reply[0] != 0 {
            continue;
        }

        if reply[4] == packet[4]
            && reply[5] == packet[5]
            && reply[6] == packet[6]
            && reply[7] == packet[7]
        {
            let _ = close(fd as usize);
            println!("[ICMP:{}] reply ok, {} bytes", tag, recv_ret);
            return true;
        }
    }
}

fn icmp_csum(data: &[u8]) -> u16 {
    let mut sum = 0u32;
    let mut i = 0;

    while i + 1 < data.len() {
        let word = ((data[i] as u16) << 8) | data[i + 1] as u16;
        sum += word as u32;
        if sum >> 16 != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
        i += 2;
    }

    if i < data.len() {
        sum += (data[i] as u32) << 8;
        if sum >> 16 != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
    }

    !(sum as u16)
}
