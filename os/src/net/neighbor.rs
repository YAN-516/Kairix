//! IPv4 邻居输出层。
//!
//! 发送 IPv4 包前需要知道下一跳 MAC。这里先查 ARP 缓存，命中则封装
//! 以太网头；未命中则暂存数据包并发送 ARP 请求。

use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

use super::arp::{arp_lookup, arp_request};
use super::device::NetDevice;
use super::ethernet::{EthernetHeader, ETH_P_IP};
use super::skb::Skb;

/// 等待 ARP 解析的数据包最长保留时间。
const ARP_PENDING_TIMEOUT_US: usize = 5_000_000;
/// 全局最多暂存的数据包数量，防止无界增长。
const ARP_PENDING_MAX_TOTAL: usize = 32;
/// 单个下一跳最多暂存的数据包数量。
const ARP_PENDING_MAX_PER_IP: usize = 8;

/// 因缺少下一跳 MAC 而暂存的发送包。
struct PendingNeighbourPacket {
    /// 待解析的下一跳 IPv4 地址。
    nexthop_ip: u32,
    /// 最终发送设备。
    dev: Arc<dyn NetDevice>,
    /// 已经包含 IP 头的待发送包。
    skb: Skb,
    /// 入队时间，用于超时清理。
    queued_at_us: usize,
}

/// 全局等待 ARP 解析的数据包队列。
static PENDING_PACKETS: Mutex<Vec<PendingNeighbourPacket>> = Mutex::new(Vec::new());

/// 把 IPv4 地址拆成日志友好的四段。
fn ip4(ip: u32) -> (u32, u32, u32, u32) {
    (
        (ip >> 24) & 0xFF,
        (ip >> 16) & 0xFF,
        (ip >> 8) & 0xFF,
        ip & 0xFF,
    )
}

/// 封装以太网头并交给设备发送。
fn xmit_ip_packet(
    mut skb: Skb,
    mac: [u8; 6],
    dev: Arc<dyn NetDevice>,
) -> Result<(Skb, u32, u16), &'static str> {
    skb.reserve_head(EthernetHeader::size());
    let eth = unsafe {
        &mut *(skb.push(EthernetHeader::size()).unwrap().as_mut_ptr() as *mut EthernetHeader)
    };
    eth.dest = mac;
    eth.src = dev.mac_addr();
    eth.ethertype = ETH_P_IP.to_be();
    dev.hard_start_xmit(skb)
}

/// 暂存等待 ARP 响应的数据包。
///
/// 入队前会清理超时包，并按全局和单 IP 上限淘汰最旧数据包。
fn queue_pending_packet(nexthop_ip: u32, dev: Arc<dyn NetDevice>, skb: Skb) {
    let now = crate::timer::get_time_us();
    let mut pending = PENDING_PACKETS.lock();

    pending.retain(|pkt| now.saturating_sub(pkt.queued_at_us) <= ARP_PENDING_TIMEOUT_US);

    let mut per_ip = pending
        .iter()
        .filter(|pkt| pkt.nexthop_ip == nexthop_ip)
        .count();
    while per_ip >= ARP_PENDING_MAX_PER_IP {
        if let Some(pos) = pending.iter().position(|pkt| pkt.nexthop_ip == nexthop_ip) {
            pending.remove(pos);
            per_ip -= 1;
        } else {
            break;
        }
    }

    while pending.len() >= ARP_PENDING_MAX_TOTAL {
        pending.remove(0);
    }

    let (a, b, c, d) = ip4(nexthop_ip);
    log::debug!(
        "Neighbour: queued packet waiting for ARP {}.{}.{}.{} (pending={})",
        a,
        b,
        c,
        d,
        pending.len() + 1
    );
    pending.push(PendingNeighbourPacket {
        nexthop_ip,
        dev,
        skb,
        queued_at_us: now,
    });
}

/// 发送所有等待指定下一跳 MAC 的数据包。
///
/// ARP 层学习到地址后调用该函数。
pub fn flush_pending_for(nexthop_ip: u32, mac: [u8; 6]) {
    let packets = {
        let mut pending = PENDING_PACKETS.lock();
        let mut out = Vec::new();
        let mut idx = 0;
        while idx < pending.len() {
            if pending[idx].nexthop_ip == nexthop_ip {
                out.push(pending.remove(idx));
            } else {
                idx += 1;
            }
        }
        out
    };

    if packets.is_empty() {
        return;
    }

    let (a, b, c, d) = ip4(nexthop_ip);
    log::debug!(
        "Neighbour: flushing {} packet(s) for {}.{}.{}.{}",
        packets.len(),
        a,
        b,
        c,
        d
    );

    for pkt in packets {
        if let Err(err) = xmit_ip_packet(pkt.skb, mac, pkt.dev) {
            log::debug!("Neighbour: pending packet transmit failed: {}", err);
        }
    }
}

/// 邻居输出（根据目的 IP 封装以太网头并发送）
///
/// `nexthop_ip` 是路由层给出的下一跳地址，不一定等于最终目的地址。
pub fn neighbour_output(
    skb: Skb,
    nexthop_ip: u32,
    dev: Arc<dyn NetDevice>,
) -> Result<(Skb, u32, u16), &'static str> {
    let (a, b, c, d) = ip4(nexthop_ip);
    log::debug!(
        "Neighbour: output to {}.{}.{}.{} via device {}",
        a,
        b,
        c,
        d,
        dev.name()
    );
    if dev.name() == "loopback" {
        return dev.hard_start_xmit(skb);
    }

    // 先尝试消费设备接收队列，处理可能已到达的 ARP 响应。
    dev.poll_rx();

    if let Some(mac) = arp_lookup(nexthop_ip) {
        return xmit_ip_packet(skb, mac, dev);
    }

    log::debug!(
        "Neighbour: no ARP entry for {}.{}.{}.{}, queueing packet and sending ARP request",
        a,
        b,
        c,
        d
    );
    queue_pending_packet(nexthop_ip, dev.clone(), skb);
    arp_request(nexthop_ip, dev.clone())?;

    for _ in 0..16 {
        dev.poll_rx();
        if let Some(mac) = arp_lookup(nexthop_ip) {
            flush_pending_for(nexthop_ip, mac);
            break;
        }
    }

    Ok((Skb::new(0), 0, 0))
}
