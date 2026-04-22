use core::fmt;

use alloc::sync::Arc;
use log::info;

use super::arp::arp_rcv;
use super::device::NetDevice;
use super::ip::ip_rcv;
use super::skb::Skb;

/// 以太网帧头
#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
pub struct EthernetHeader {
    pub dest: [u8; 6],
    pub src: [u8; 6],
    pub ethertype: u16,
}

impl EthernetHeader {
    pub fn size() -> usize {
        core::mem::size_of::<EthernetHeader>()
    }
}

impl fmt::Display for EthernetHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Ethernet: dest={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, src={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, type=0x{:x}",
            self.dest[0],
            self.dest[1],
            self.dest[2],
            self.dest[3],
            self.dest[4],
            self.dest[5],
            self.src[0],
            self.src[1],
            self.src[2],
            self.src[3],
            self.src[4],
            self.src[5],
            u16::from_be(self.ethertype)
        )
    }
}

// 以太网类型
pub const ETH_P_IP: u16 = 0x0800;
pub const ETH_P_ARP: u16 = 0x0806;

#[allow(unused)]
/// 以太网接收入口：剥离二层头后分发到 ARP/IP。
pub fn ethernet_rcv(
    mut skb: Skb,
    dev: Arc<dyn NetDevice>,
) -> Result<(Skb, u32, u16), &'static str> {
    if skb.len() < EthernetHeader::size() {
        return Err("Ethernet frame too short");
    }

    let eth = unsafe { &*(skb.data().as_ptr() as *const EthernetHeader) };
    let ethertype = u16::from_be(eth.ethertype);
    info!(
        "Ethernet RX dev={} len={} type=0x{:04x} src={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} dst={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        dev.name(),
        skb.len(),
        ethertype,
        eth.src[0],
        eth.src[1],
        eth.src[2],
        eth.src[3],
        eth.src[4],
        eth.src[5],
        eth.dest[0],
        eth.dest[1],
        eth.dest[2],
        eth.dest[3],
        eth.dest[4],
        eth.dest[5],
    );

    skb.dev = Some(dev.clone());
    let _ = skb.pull(EthernetHeader::size());

    match ethertype {
        ETH_P_IP => ip_rcv(skb),
        ETH_P_ARP => {
            arp_rcv(skb, dev);
            Ok((Skb::new(0), 0, 0))
        }
        _ => Err("Unsupported ethertype"),
    }
}
