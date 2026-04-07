use alloc::sync::Arc;

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
    if dev.name() == "loopback" {
        return dev.hard_start_xmit(skb);
    }

    // 查找 MAC 地址
    let mac = match arp_lookup(nexthop_ip) {
        Some(mac) => mac,
        None => {
            // 没有缓存，发送 ARP 请求
            log::debug!(
                "Neighbour: no ARP entry for {}.{}.{}.{}, sending ARP request",
                (nexthop_ip >> 24) & 0xFF,
                (nexthop_ip >> 16) & 0xFF,
                (nexthop_ip >> 8) & 0xFF,
                nexthop_ip & 0xFF
            );
            arp_request(nexthop_ip, dev.clone())?;
            return Err("ARP resolution pending");
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
