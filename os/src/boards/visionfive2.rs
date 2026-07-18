pub const _CLOCK_FREQ: usize = 4_000_000;

#[allow(unused)]
pub const MMIO: &[(usize, usize)] = &[
    (0x0200_0000, 0x01_0000), // ACLINT/CLINT area reported by OpenSBI
    (0x0c00_0000, 0x40_0000), // PLIC
    (0x1000_0000, 0x00_1000), // UART0
    (0x1601_0000, 0x00_1000), // sdio0
    (0x1602_0000, 0x00_1000), // sdio1
];

pub type BlockDeviceImpl = crate::drivers::block::VirtIOBlock;
