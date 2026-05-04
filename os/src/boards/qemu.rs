pub const _CLOCK_FREQ: usize = 12500000;
pub const MEMORY_END: usize = 0x9000_0000;

#[allow(unused)]
pub const MMIO: &[(usize, usize)] = &[
    (0x0010_0000, 0x00_2000), // VIRT_TEST/RTC  in virt machine
    (0x1000_1000, 0x00_1000), // Virtio Block in virt machine
    (0x3000_0000, 0x10_0000), // PCIe ECAM (bus 0 config space)
    (0x4000_0000, 0x4000_0000), // PCIe MMIO window
];

pub type BlockDeviceImpl = crate::drivers::block::VirtIOBlock;
