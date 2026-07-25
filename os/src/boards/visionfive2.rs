pub const _CLOCK_FREQ: usize = 4_000_000;

#[allow(unused)]
pub const MMIO: &[(usize, usize)] = &[
    (0x0200_0000, 0x01_0000), // ACLINT/CLINT area reported by OpenSBI
    (0x0201_0000, 0x00_4000), // SiFive L2 cache controller
    (0x0c00_0000, 0x40_0000), // PLIC
    (0x1000_0000, 0x00_1000), // UART0
    (0x1302_0000, 0x00_1000), // system clock/reset controller
    (0x1601_0000, 0x00_1000), // sdio0
    (0x1602_0000, 0x00_1000), // sdio1
    (0x1603_0000, 0x01_0000), // GMAC0 (DWMAC 5.10a)
    (0x1700_0000, 0x00_1000), // always-on clock/reset controller
    (0x1701_0000, 0x00_1000), // always-on syscon (GMAC0 PHY interface select)
];

pub type BlockDeviceImpl = crate::drivers::block::Vf2SdBlock;

#[cfg(vf2_root_part = "4")]
pub const ROOT_PARTITION: usize = 4;
#[cfg(vf2_root_part = "5")]
pub const ROOT_PARTITION: usize = 5;
#[cfg(vf2_root_part = "6")]
pub const ROOT_PARTITION: usize = 6;
#[cfg(vf2_root_part = "7")]
pub const ROOT_PARTITION: usize = 7;
#[cfg(vf2_root_part = "8")]
pub const ROOT_PARTITION: usize = 8;

#[cfg(not(any(
    vf2_root_part = "4",
    vf2_root_part = "5",
    vf2_root_part = "6",
    vf2_root_part = "7",
    vf2_root_part = "8"
)))]
pub const ROOT_PARTITION: usize = 5;
