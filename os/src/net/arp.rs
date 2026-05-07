use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use log::info;
use crate::sync::SpinNoIrqLock;

use super::device::NetDevice;
use super::ethernet::{ETH_P_ARP, EthernetHeader};
use super::skb::Skb;

const ARP_REQUEST: u16 = 1;
#[allow(unused)]
const ARP_REPLY: u16 = 2;
const HARD_TYPE_ETHERNET: u16 = 1;
const PROTO_IP: u16 = 0x0800;

/// ARP 包结构
#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
pub struct ArpPacket {
    pub hw_type: u16,    // 硬件类型
    pub proto_type: u16, // 协议类型
    pub hw_addr_len: u8,
    pub proto_addr_len: u8,
    pub op: u16, // 操作码
    pub sender_hw: [u8; 6],
    pub sender_proto: u32,
    pub target_hw: [u8; 6],
    pub target_proto: u32,
}

impl ArpPacket {
    pub fn size() -> usize {
        core::mem::size_of::<ArpPacket>()
    }
}

/// ARP 缓存条目
#[allow(unused)]
#[derive(Clone)]
struct ArpEntry {
    mac: [u8; 6],
    dev: Arc<dyn NetDevice>,
}

/// 全局 ARP 缓存
static ARP_CACHE: SpinNoIrqLock<BTreeMap<u32, ArpEntry>> = SpinNoIrqLock::new(BTreeMap::new());
#[allow(unused)]
/// 添加 ARP 缓存条目
pub fn arp_add_entry(ip: u32, mac: [u8; 6], dev: Arc<dyn NetDevice>) {
    ARP_CACHE.lock().insert(ip, ArpEntry { mac, dev });
    log::debug!(
        "ARP: added entry for {}.{}.{}.{} -> {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        (ip >> 24) & 0xFF,
        (ip >> 16) & 0xFF,
        (ip >> 8) & 0xFF,
        ip & 0xFF,
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5]
    );
}

/// 查找 ARP 缓存
pub fn arp_lookup(ip: u32) -> Option<[u8; 6]> {
    ARP_CACHE.lock().get(&ip).map(|entry| entry.mac)
}

/// 发送 ARP 请求
pub fn arp_request(ip: u32, dev: Arc<dyn NetDevice>) -> Result<(), &'static str> {
    let sender_ip = dev.ip_addr();

    info!(
        "ARP: sending request for {}.{}.{}.{}",
        (ip >> 24) & 0xFF,
        (ip >> 16) & 0xFF,
        (ip >> 8) & 0xFF,
        ip & 0xFF
    );

    let mut skb = Skb::new(64);

    // 以太网头
    let eth = unsafe {
        &mut *(skb.put(EthernetHeader::size()).unwrap().as_mut_ptr() as *mut EthernetHeader)
    };
    eth.dest = [0xFF; 6];
    eth.src = dev.mac_addr();
    eth.ethertype = ETH_P_ARP.to_be();

    // ARP 包
    let arp = unsafe { &mut *(skb.put(ArpPacket::size()).unwrap().as_mut_ptr() as *mut ArpPacket) };
    arp.hw_type = HARD_TYPE_ETHERNET.to_be();
    arp.proto_type = PROTO_IP.to_be();
    arp.hw_addr_len = 6;
    arp.proto_addr_len = 4;
    arp.op = ARP_REQUEST.to_be();
    arp.sender_hw = dev.mac_addr();
    arp.sender_proto = sender_ip.to_be();
    arp.target_hw = [0; 6];
    arp.target_proto = ip.to_be();

    info!(
        "ARP: request sender {}.{}.{}.{} ({:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}) asking for {}.{}.{}.{}",
        (sender_ip >> 24) & 0xFF,
        (sender_ip >> 16) & 0xFF,
        (sender_ip >> 8) & 0xFF,
        sender_ip & 0xFF,
        arp.sender_hw[0],
        arp.sender_hw[1],
        arp.sender_hw[2],
        arp.sender_hw[3],
        arp.sender_hw[4],
        arp.sender_hw[5],
        (ip >> 24) & 0xFF, // 取最高字节 = 0x0A = 10 ✅
        (ip >> 16) & 0xFF, // 0x00 = 0
        (ip >> 8) & 0xFF,  // 0x02 = 2
        ip & 0xFF          // 0x0F = 15
    );

    dev.hard_start_xmit(skb).map_err(|_| "ARP send failed")?;
    Ok(())
}
#[allow(unused)]
/// 处理接收到的 ARP 包
pub fn arp_rcv(skb: Skb, dev: Arc<dyn NetDevice>) {
    if skb.len() < ArpPacket::size() {
        log::warn!("ARP: packet too short");
        return;
    }

    let arp = unsafe { &*(skb.data().as_ptr() as *const ArpPacket) };

    let op = u16::from_be(arp.op);
    let sender_ip = u32::from_be(arp.sender_proto);
    let target_ip = u32::from_be(arp.target_proto);

    info!(
        "ARP: received op={}, sender_ip={}.{}.{}.{}, target_ip={}.{}.{}.{}",
        op,
        (sender_ip >> 24) & 0xFF,
        (sender_ip >> 16) & 0xFF,
        (sender_ip >> 8) & 0xFF,
        sender_ip & 0xFF,
        (target_ip >> 24) & 0xFF,
        (target_ip >> 16) & 0xFF,
        (target_ip >> 8) & 0xFF,
        target_ip & 0xFF
    );

    // 更新缓存
    arp_add_entry(sender_ip, arp.sender_hw, dev.clone());

    // 如果是请求且目标是自己，发送回复
    if op == ARP_REQUEST && target_ip == dev.ip_addr() {
        arp_reply(sender_ip, arp.sender_hw, dev);
    }
}

/// 发送 ARP 回复
fn arp_reply(target_ip: u32, target_mac: [u8; 6], dev: Arc<dyn NetDevice>) {
    log::debug!(
        "ARP: sending reply to {}.{}.{}.{}",
        (target_ip >> 24) & 0xFF,
        (target_ip >> 16) & 0xFF,
        (target_ip >> 8) & 0xFF,
        target_ip & 0xFF
    );

    let mut skb = Skb::new(64);

    // 以太网头
    let eth = unsafe {
        &mut *(skb.put(EthernetHeader::size()).unwrap().as_mut_ptr() as *mut EthernetHeader)
    };
    eth.dest = target_mac;
    eth.src = dev.mac_addr();
    eth.ethertype = ETH_P_ARP.to_be();

    // ARP 包
    let arp = unsafe { &mut *(skb.put(ArpPacket::size()).unwrap().as_mut_ptr() as *mut ArpPacket) };
    arp.hw_type = HARD_TYPE_ETHERNET.to_be();
    arp.proto_type = PROTO_IP.to_be();
    arp.hw_addr_len = 6;
    arp.proto_addr_len = 4;
    arp.op = ARP_REPLY.to_be();
    arp.sender_hw = dev.mac_addr();
    arp.sender_proto = dev.ip_addr().to_be();
    arp.target_hw = target_mac;
    arp.target_proto = target_ip.to_be();

    let _ = dev.hard_start_xmit(skb);
}
