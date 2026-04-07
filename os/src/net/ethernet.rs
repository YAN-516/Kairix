use core::fmt;

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
