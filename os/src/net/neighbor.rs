use alloc::sync::Arc;
use log::info;

use super::arp::{arp_lookup, arp_request};
use super::device::NetDevice;
use super::ethernet::{ETH_P_IP, EthernetHeader};
use super::skb::Skb;

/// 邻居输出（根据目的 IP 封装以太网头并发送）
pub fn neighbour_output(
    mut skb: Skb,
    nexthop_ip: u32,
    dev: Arc<dyn NetDevice>,
) -> Result<(Skb, u32, u16), &'static str> {
    // 回环设备直接发送
    info!(
        "Neighbour: output to {}.{}.{}.{} via device {}",
        (nexthop_ip >> 24) & 0xFF,
        (nexthop_ip >> 16) & 0xFF,
        (nexthop_ip >> 8) & 0xFF,
        nexthop_ip & 0xFF,
        dev.name()
    );
    if dev.name() == "loopback" {
        return dev.hard_start_xmit(skb);
    }

    // 先尝试消费设备接收队列，处理可能已到达的 ARP 响应。
    dev.poll_rx();

    // 查找 MAC 地址
    let mac = match arp_lookup(nexthop_ip) {
        Some(mac) => mac,
        None => {
            // 没有缓存，发送 ARP 请求
            info!(
                "Neighbour: no ARP entry for {}.{}.{}.{}, sending ARP request",
                (nexthop_ip >> 24) & 0xFF,
                (nexthop_ip >> 16) & 0xFF,
                (nexthop_ip >> 8) & 0xFF,
                nexthop_ip & 0xFF
            );
            arp_request(nexthop_ip, dev.clone())?;

            // ARP 请求发出后短轮询几次 RX，等待 ARP 响应入缓存。
            let mut resolved_mac = None;
            for _ in 0..16 {
                dev.poll_rx();
                if let Some(m) = arp_lookup(nexthop_ip) {
                    resolved_mac = Some(m);
                    break;
                }
            }
            match resolved_mac {
                Some(m) => m,
                None => return Err("ARP resolution pending"),
            }
        }
    };

    // 封装以太网头
    skb.reserve_head(EthernetHeader::size());
    let eth = unsafe {
        &mut *(skb.push(EthernetHeader::size()).unwrap().as_mut_ptr() as *mut EthernetHeader)
    };
    eth.dest = mac;
    eth.src = dev.mac_addr();
    eth.ethertype = ETH_P_IP.to_be();
    dev.hard_start_xmit(skb)
}
