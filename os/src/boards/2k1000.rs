pub const _CLOCK_FREQ: usize = 100_000_000;

#[allow(unused)]
pub const MMIO: &[(usize, usize)] = &[
    (0x0010_0000, 0x00_2000), // Legacy platform registers used during early boot.
    (0x1000_1000, 0x00_8000), // LS7A-compatible low MMIO window.
    (0x100d_0000, 0x00_1000), // LS7A RTC.
    (0x1fe0_0000, 0x00_4000), // Pinmux, interrupt controller, and UART window.
    (0x3000_0000, 0x10_0000), // PCIe ECAM aperture.
    (0x4000_0000, 0x4000_0000), // SoC devices, including AHCI at 0x400e0000.
];
