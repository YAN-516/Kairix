#![no_std]
#![no_main]
#![allow(clippy::needless_range_loop)]

#[macro_use]
extern crate user_lib;
// 导入网络栈模块
// use crate::net::*;
// use crate::socket::*;
// use crate::syscall::*;
use user_lib::{socket,sendto,recvfrom};
#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("into sleep test!");
    test_icmp_loopback();
    //test_udp_loopback();
    0
}

/// ============================================
/// ICMP 校验和计算
/// ============================================
fn icmp_csum(data: &[u8]) -> u16 {
    let mut sum = 0u32;
    let chunks = data.chunks_exact(2);
    for chunk in chunks {
        sum += ((chunk[0] as u32) << 8) | (chunk[1] as u32);
        if sum >> 16 != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
    }
    if data.len() % 2 == 1 {
        sum += (data[data.len() - 1] as u32) << 8;
        if sum >> 16 != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
    }
    !sum as u16
}

/// ============================================
/// 测试1: ICMP (ping) 回环测试
/// ============================================
pub fn test_icmp_loopback() {
    println!("========================================================");
    println!("           ICMP Loopback Test (ping 127.0.0.1)            ");
    println!("========================================================");

    // 1. 初始化
    println!("[Step 1] Initializing network stack...");
    println!("✓ Network stack initialized\n");

    // 2. 创建原始套接字
    println!("[Step 2] Creating raw ICMP socket...");
    let fd = socket(2, 3, 1);
        if fd != -1{
            println!("✓ Created ICMP socket (fd={})", fd);
        }
        else {
            println!("✗ Failed to create socket");
            return;
        }

    // 3. 构造 ICMP Echo Request
    println!("[Step 3] Building ICMP Echo Request packet...");
    let mut icmp_packet = [0u8; 64];

    // ICMP 头
    icmp_packet[0] = 8; // type = Echo Request
    icmp_packet[1] = 0; // code = 0
    icmp_packet[2] = 0; // checksum (high)
    icmp_packet[3] = 0; // checksum (low)
    icmp_packet[4] = 0x12; // identifier (high)
    icmp_packet[5] = 0x34; // identifier (low)
    icmp_packet[6] = 0x00; // sequence (high)
    icmp_packet[7] = 0x01; // sequence (low)

    // 填充数据 (56 bytes)
    for i in 0..56 {
        icmp_packet[8 + i] = i as u8;
        //println!("{} ",i);
    }

    // 计算校验和
    let checksum = icmp_csum(&icmp_packet);
    icmp_packet[2] = (checksum >> 8) as u8;
    icmp_packet[3] = (checksum & 0xFF) as u8;

    println!("✓ ICMP Echo Request built:");
    println!("  - Identifier: 0x1234");
    println!("  - Sequence: 1");
    println!("  - Data size: 56 bytes");
    println!("  - Checksum: 0x{:04X}\n", checksum);

    // 4. 发送
    println!("[Step 4] Sending ICMP Echo Request to 127.0.0.1...");
    //let start_time = std::time::Instant::now();

    let send_ret = sendto(
        fd as usize,
        icmp_packet.as_ptr(),
        icmp_packet.len(),
        0,
        core::ptr::null(),
        0,
    ); 
        if send_ret != -1{
            println!("✓ Sent {} bytes", send_ret);
        }
        else {
            println!("✗ Failed to send");
            return;
        }

    // 5. 接收响应
    println!("[Step 5] Waiting for ICMP Echo Reply...");
    let mut reply = [0u8; 64];
    println!("-----------------------");
    let recv_ret = recvfrom(
        fd as usize,
        reply.as_mut_ptr(),
        reply.len(),
        0,
        core::ptr::null_mut(),
        core::ptr::null_mut(),
    ); 
        if recv_ret != -1 {
            println!("✓ Received {} bytes", recv_ret);

            // 6. 验证
            println!("\n[Step 6] Validating response...");
            let mut valid = true;

            // 检查类型
            if reply[0] == 0 {
                println!("✓ ICMP type: Echo Reply (0)");
            } else {
                println!("✗ Unexpected ICMP type: {} (expected 0)", reply[0]);
                valid = false;
            }

            // 检查标识符
            if reply[4] == 0x12 && reply[5] == 0x34 {
                println!("✓ Identifier matches: 0x1234");
            } else {
                println!("✗ Identifier mismatch: 0x{:02X}{:02X}", reply[4], reply[5]);
                valid = false;
            }

            // 检查序列号
            if reply[6] == 0x00 && reply[7] == 0x01 {
                println!("✓ Sequence matches: 1");
            } else {
                println!(
                    "✗ Sequence mismatch: {}",
                    (reply[6] as u16) << 8 | reply[7] as u16
                );
                valid = false;
            }

            // 验证数据完整性
            let mut data_valid = true;
            for i in 0..56 {
                if i < recv_ret - 8 && reply[(8 + i) as usize] != i as u8 {
                    data_valid = false;
                    println!("✗ Data mismatch at offset {}", i);
                    break;
                }
            }
            if data_valid {
                println!("✓ Data payload verified ({} bytes)", recv_ret - 8);
            } else {
                println!("✗ Data payload corrupted");
                valid = false;
            }

            // 最终结果
            if valid {
                println!("========================================================");
                println!("           ICMP LOOPBACK TEST SUCESS!                   ");
                println!("========================================================");
            } else {
                println!("========================================================");
                println!("           ICMP LOOPBACK TEST FAILED!                   ");
                println!("========================================================");
            }
        }
        else {
            println!("========================================================");
            println!("           CMP LOOPBACK TEST FAILED! (No response)      ");
            println!("========================================================");
    }
}
#[test]
/// ============================================
/// 测试2: UDP 回环测试
/// ============================================
fn test_udp_loopback() {
    // 初始化
    println!("========================================================");
    println!("           UDP Loopback Test (127.0.0.1:5000)            ");
    println!("========================================================");

    // 创建 UDP 套接字
    let fd = match socket(2, 2, 0) {
        // AF_INET, SOCK_DGRAM, 0
        Ok(fd) => {
            println!("✓ Created UDP socket (fd={})", fd);
            fd
        }
        Err(e) => {
            println!("✗ Failed to create socket: {}", e);
            return;
        }
    };

    // 绑定到本地地址
    let port = 5000u16;
    let addr = 0x7F000001; // 127.0.0.1

    let mut sockaddr = [0u8; 16];
    sockaddr[0] = 0x02; // AF_INET
    sockaddr[2] = (port >> 8) as u8;
    sockaddr[3] = (port & 0xFF) as u8;
    sockaddr[4] = (addr >> 24) as u8;
    sockaddr[5] = (addr >> 16) as u8;
    sockaddr[6] = (addr >> 8) as u8;
    sockaddr[7] = (addr & 0xFF) as u8;

    match bind(fd, sockaddr.as_ptr(), 16) {
        Ok(_) => println!("✓ Bound to 127.0.0.1:{}\n", port),
        Err(e) => {
            println!("✗ Failed to bind: {}\n", e);
            return;
        }
    }

    // 测试数据
    let test_messages = 
        b"Hello, UDP!"
        // b"Loopback test message",
        // b"The quick brown fox jumps over the lazy dog",
        // b"1234567890",
    ;

    let mut success_count = 0;

    for (i, msg) in test_messages.iter().enumerate() {
        println!("--- Test {} ---", i + 1);

        // 发送
        match sendto(fd, msg.as_ptr(), msg.len(), 0, sockaddr.as_ptr(), 16) {
            Ok(sent) => {
                println!(
                    "  Sent {} bytes: \"{}\"",
                    sent,
                    core::str::from_utf8(test_messages).unwrap()
                );
            }
            Err(e) => {
                println!("  ✗ Failed to send: {}", e);
                continue;
            }
        }

        // 接收
        let mut recv_buf = [0u8; 128];
        match sys_recvfrom(
            fd,
            recv_buf.as_mut_ptr(),
            recv_buf.len(),
            0,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        ) {
            Ok(recv_len) => {
                let received = &recv_buf[..recv_len];
                if received == msg {
                    println!("  ✓ Received {} bytes: Data matches!", recv_len);
                    success_count += 1;
                } else {
                    println!("  ✗ Data mismatch!");
                    println!("    Expected: {:?}", msg);
                    println!("    Received: {:?}", received);
                }
            }
            Err(e) => {
                println!("  ✗ Failed to receive: {}", e);
            }
        }
    }

    if success_count == test_messages.len() {
        println!("╔═══════════════════════════════════════════════════════════╗");
        println!("║  ✅ UDP LOOPBACK TEST PASSED!                             ║");
        println!("╚═══════════════════════════════════════════════════════════╝");
    } else {
        println!("╔═══════════════════════════════════════════════════════════╗");
        println!(
            "║  ⚠️ UDP LOOPBACK TEST PARTIALLY PASSED ({}/{})              ║",
            success_count,
            test_messages.len()
        );
        println!("╚═══════════════════════════════════════════════════════════╝");
    }
}

/// ============================================
/// 测试3: 多包性能测试
/// ============================================
#[test]
fn test_performance() {
    println!("\n╔═══════════════════════════════════════════════════════════╗");
    println!("║           Performance Test (100 packets)                  ║");
    println!("╚═══════════════════════════════════════════════════════════╝\n");

    crate::test_net::init();
    socket::init();

    let fd = sys_socket(2, 2, 0).expect("Failed to create socket");

    // 绑定
    let mut sockaddr = [0u8; 16];
    sockaddr[0] = 0x02;
    sockaddr[2] = (6000 >> 8) as u8;
    sockaddr[3] = (6000 & 0xFF) as u8;
    sockaddr[4] = 0x7F;
    sockaddr[5] = 0x00;
    sockaddr[6] = 0x00;
    sockaddr[7] = 0x01;
    sys_bind(fd, sockaddr.as_ptr(), 16).expect("Failed to bind");

    let packet_count = 100;
    let test_data = [0xAAu8; 1024]; // 1KB 数据

    println!(
        "Sending {} packets of {} bytes...",
        packet_count,
        test_data.len()
    );
    print!("Progress: ");

    let start = std::time::Instant::now();
    let mut success = 0;

    for i in 0..packet_count {
        // 发送
        if sys_sendto(
            fd,
            test_data.as_ptr(),
            test_data.len(),
            0,
            sockaddr.as_ptr(),
            16,
        )
        .is_ok()
        {
            // 接收
            let mut recv_buf = [0u8; 2048];
            if let Ok(recv_len) = sys_recvfrom(
                fd,
                recv_buf.as_mut_ptr(),
                recv_buf.len(),
                0,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            ) {
                if recv_len == test_data.len() && &recv_buf[..recv_len] == &test_data[..] {
                    success += 1;
                }
            }
        }

        if (i + 1) % 10 == 0 {
            print!(".");
        }
    }

    let duration = start.elapsed();
    let total_bytes = packet_count * test_data.len();
    let throughput = total_bytes as f64 / duration.as_secs_f64();

    println!("\n\n📊 Results:");
    println!(
        "  ✓ Success rate: {}/{} ({:.1}%)",
        success,
        packet_count,
        (success as f64 / packet_count as f64) * 100.0
    );
    println!(
        "  ✓ Total data: {} bytes ({:.2} KB)",
        total_bytes,
        total_bytes as f64 / 1024.0
    );
    println!("  ✓ Time: {:?}", duration);
    println!("  ✓ Throughput: {:.2} MB/s", throughput / 1024.0 / 1024.0);

    if success == packet_count {
        println!("\n✅ PERFORMANCE TEST PASSED!");
    } else {
        println!(
            "\n⚠️ PERFORMANCE TEST COMPLETED WITH {} LOSS",
            packet_count - success
        );
    }
}

/// ============================================
/// 测试4: 多次 ping 测试
/// ============================================
#[test]
fn test_multiple_ping() {
    println!("\n╔═══════════════════════════════════════════════════════════╗");
    println!("║           Multiple Ping Test (5 packets)                  ║");
    println!("╚═══════════════════════════════════════════════════════════╝\n");

    crate::test_net::init();
    socket::init();

    let fd = sys_socket(2, 3, 1).expect("Failed to create ICMP socket");

    let packet_count = 5;
    let mut success = 0;
    let mut rtts = Vec::new();

    println!("PING 127.0.0.1 (127.0.0.1):\n");

    for seq in 1..=packet_count {
        let mut packet = [0u8; 64];
        packet[0] = 8; // Echo Request
        packet[1] = 0;
        packet[4] = 0x12;
        packet[5] = 0x34;
        packet[6] = ((seq >> 8) & 0xFF) as u8;
        packet[7] = (seq & 0xFF) as u8;

        for i in 0..56 {
            packet[8 + i] = i as u8;
        }

        let checksum = icmp_csum(&packet);
        packet[2] = (checksum >> 8) as u8;
        packet[3] = (checksum & 0xFF) as u8;

        let start = std::time::Instant::now();

        if sys_sendto(fd, packet.as_ptr(), packet.len(), 0, core::ptr::null(), 0).is_ok() {
            let mut reply = [0u8; 64];
            if let Ok(recv_len) = sys_recvfrom(
                fd,
                reply.as_mut_ptr(),
                reply.len(),
                0,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            ) {
                let rtt = start.elapsed();
                rtts.push(rtt);
                success += 1;

                let recv_seq = ((reply[6] as u16) << 8) | (reply[7] as u16);
                println!(
                    "  {} bytes from 127.0.0.1: icmp_seq={} time={:?}",
                    recv_len, recv_seq, rtt
                );
            }
        }
    }

    println!("\n--- 127.0.0.1 ping statistics ---");
    println!(
        "{} packets transmitted, {} received, {:.1}% packet loss",
        packet_count,
        success,
        (packet_count - success) as f64 / packet_count as f64 * 100.0
    );

    if !rtts.is_empty() {
        let min = *rtts.iter().min().unwrap();
        let max = *rtts.iter().max().unwrap();
        let avg = rtts.iter().sum::<Duration>() / rtts.len() as u32;
        println!("round-trip min/avg/max = {:?}/{:?}/{:?}", min, avg, max);
    }

    if success == packet_count {
        println!("\n✅ MULTIPLE PING TEST PASSED!");
    } else {
        println!("\n⚠️ MULTIPLE PING TEST COMPLETED WITH LOSS");
    }
}

/// ============================================
/// 运行所有测试
/// ============================================
#[test]
fn run_all_tests() {
    println!("\n");
    println!("╔═══════════════════════════════════════════════════════════╗");
    println!("║           NETWORK LOOPBACK TEST SUITE                     ║");
    println!("╚═══════════════════════════════════════════════════════════╝");

    test_icmp_loopback();
    // println!("\n" + "-".repeat(60));

    // test_udp_loopback();
    // println!("\n" + "-".repeat(60));

    // test_multiple_ping();
    // println!("\n" + "-".repeat(60));

    test_performance();

    println!("\n");
    println!("╔═══════════════════════════════════════════════════════════╗");
    println!("║           ALL TESTS COMPLETED!                            ║");
    println!("╚═══════════════════════════════════════════════════════════╝");
}
