use crate::config::BLOCK_SIZE;
use crate::devices::BlockDevice;
use crate::mm::{PhysPageNum, frame_alloc_hal, frame_dealloc};
use crate::sync::SpinNoIrqLock;
use core::arch::asm;
use core::cmp::min;
use core::ptr::{read_volatile, write_volatile};
use log::{info, warn};
use polyhal::consts::PAGE_SIZE;

const AHCI_PHYS_BASE: usize = 0x400e_0000;
const LOONGARCH_UNCACHED_DMW_BASE: usize = 0x8000_0000_0000_0000;
const AHCI_MMIO_BASE: usize = LOONGARCH_UNCACHED_DMW_BASE | AHCI_PHYS_BASE;

const HBA_CAP: usize = 0x00;
const HBA_GHC: usize = 0x04;
const HBA_PI: usize = 0x0c;
const HBA_PORTS: usize = 0x100;
const HBA_PORT_STRIDE: usize = 0x80;

const PORT_CLB: usize = 0x00;
const PORT_CLBU: usize = 0x04;
const PORT_FB: usize = 0x08;
const PORT_FBU: usize = 0x0c;
const PORT_IS: usize = 0x10;
const PORT_IE: usize = 0x14;
const PORT_CMD: usize = 0x18;
const PORT_TFD: usize = 0x20;
const PORT_SIG: usize = 0x24;
const PORT_SSTS: usize = 0x28;
const PORT_SERR: usize = 0x30;
const PORT_SACT: usize = 0x34;
const PORT_CI: usize = 0x38;

const GHC_HR: u32 = 1 << 0;
const GHC_AE: u32 = 1 << 31;
const CAP_SCLO: u32 = 1 << 24;
const CAP_SSS: u32 = 1 << 27;
const CAP_SMPS: u32 = 1 << 28;
const PORT_CMD_ST: u32 = 1 << 0;
const PORT_CMD_SUD: u32 = 1 << 1;
const PORT_CMD_POD: u32 = 1 << 2;
const PORT_CMD_CLO: u32 = 1 << 3;
const PORT_CMD_FRE: u32 = 1 << 4;
const PORT_CMD_FR: u32 = 1 << 14;
const PORT_CMD_CR: u32 = 1 << 15;
const PORT_CMD_ICC_MASK: u32 = 0xf << 28;
const PORT_CMD_ICC_ACTIVE: u32 = 1 << 28;
const PORT_IS_TFES: u32 = 1 << 30;
const ATA_DEV_BUSY: u32 = 0x80;
const ATA_DEV_DRQ: u32 = 0x08;
const SATA_SIG_ATA: u32 = 0x0000_0101;

const FIS_TYPE_REG_H2D: u8 = 0x27;
const ATA_CMD_IDENTIFY: u8 = 0xec;
const ATA_CMD_READ_DMA_EXT: u8 = 0x25;
const ATA_CMD_WRITE_DMA_EXT: u8 = 0x35;

const COMMAND_TABLE_PRDT_OFFSET: usize = 0x80;
const MAX_SECTORS_PER_COMMAND: usize = 8;
const POLL_LIMIT: usize = 100_000_000;

#[derive(Clone, Copy)]
enum DmaDirection {
    Read,
    Write,
}

pub struct AhciBlock {
    inner: SpinNoIrqLock<AhciInner>,
}

struct AhciInner {
    port_base: usize,
    sectors: u64,
    command_list: DmaFrame,
    _received_fis: DmaFrame,
    command_table: DmaFrame,
    bounce: DmaFrame,
}

/// A page used by the non-coherent LS2K1000 AHCI controller.
///
/// The cached DMW alias must never be used for these pages: a barrier orders
/// memory accesses but does not clean or invalidate cache lines. Keeping all
/// CPU accesses on the uncached alias makes device and CPU views agree.
struct DmaFrame {
    ppn: PhysPageNum,
}

impl DmaFrame {
    fn allocate() -> Result<Self, &'static str> {
        let ppn = frame_alloc_hal().ok_or("failed to allocate AHCI DMA frame")?;
        let paddr = (ppn.0 as u64) << 12;
        if paddr + PAGE_SIZE as u64 > (u32::MAX as u64) + 1 {
            frame_dealloc(ppn);
            return Err("AHCI DMA frame is above the 32-bit DMA limit");
        }
        let mut frame = Self { ppn };
        frame.bytes().fill(0);
        dma_barrier();
        Ok(frame)
    }

    fn paddr(&self) -> u64 {
        (self.ppn.0 as u64) << 12
    }

    fn bytes(&mut self) -> &mut [u8] {
        let addr = LOONGARCH_UNCACHED_DMW_BASE | self.paddr() as usize;
        unsafe { core::slice::from_raw_parts_mut(addr as *mut u8, PAGE_SIZE) }
    }
}

impl Drop for DmaFrame {
    fn drop(&mut self) {
        frame_dealloc(self.ppn);
    }
}

impl AhciBlock {
    pub fn try_new() -> Result<Self, &'static str> {
        let command_list = DmaFrame::allocate()?;
        let received_fis = DmaFrame::allocate()?;
        let command_table = DmaFrame::allocate()?;
        let bounce = DmaFrame::allocate()?;

        let cap = mmio_read(AHCI_MMIO_BASE + HBA_CAP);
        let ghc = mmio_read(AHCI_MMIO_BASE + HBA_GHC);
        let mut implemented = mmio_read(AHCI_MMIO_BASE + HBA_PI);
        info!(
            "[ahci] probe base={:#x}, cap={:#x}, ghc={:#x}, pi={:#x}",
            AHCI_PHYS_BASE, cap, ghc, implemented
        );

        // LS2K1000 leaves PI cleared after reset. Its U-Boot AHCI driver marks
        // port 0 implemented during `scsi scan`; do the same when booting the
        // kernel without probing SATA in U-Boot first.
        if implemented == 0 {
            warn!("[ahci] PI is zero; resetting the HBA and enabling port 0");
            reset_hba()?;
            let cap = mmio_read(AHCI_MMIO_BASE + HBA_CAP);
            mmio_write(AHCI_MMIO_BASE + HBA_CAP, cap | CAP_SMPS | CAP_SSS);
            mmio_write(AHCI_MMIO_BASE + HBA_PI, 0xf);
            let port_count = (cap & 0x1f) + 1;
            let port_mask = if port_count == 32 {
                u32::MAX
            } else {
                (1u32 << port_count) - 1
            };
            implemented = mmio_read(AHCI_MMIO_BASE + HBA_PI) & port_mask;
            info!(
                "[ahci] LS2K1000 setup: cap={:#x}, pi={:#x}",
                mmio_read(AHCI_MMIO_BASE + HBA_CAP),
                mmio_read(AHCI_MMIO_BASE + HBA_PI)
            );
            if implemented == 0 {
                warn!("[ahci] PI write did not latch; probing fixed port 0");
                implemented = 1;
            }
        }

        mmio_write(
            AHCI_MMIO_BASE + HBA_GHC,
            mmio_read(AHCI_MMIO_BASE + HBA_GHC) | GHC_AE,
        );

        let mut selected = None;
        for port in 0..32 {
            if implemented & (1 << port) == 0 {
                continue;
            }
            let base = AHCI_MMIO_BASE + HBA_PORTS + port * HBA_PORT_STRIDE;
            let mut ssts = mmio_read(base + PORT_SSTS);
            info!(
                "[ahci] port {} ssts={:#x}, sig={:#x}, tfd={:#x}, cmd={:#x}",
                port,
                ssts,
                mmio_read(base + PORT_SIG),
                mmio_read(base + PORT_TFD),
                mmio_read(base + PORT_CMD)
            );
            if !link_is_active(ssts) {
                warn!("[ahci] port {} link is down; starting PHY", port);
                if let Err(err) = bring_up_link(base) {
                    warn!("[ahci] port {} link startup failed: {}", port, err);
                    continue;
                }
                ssts = mmio_read(base + PORT_SSTS);
                info!(
                    "[ahci] port {} link up: ssts={:#x}, sig={:#x}, tfd={:#x}, cmd={:#x}",
                    port,
                    ssts,
                    mmio_read(base + PORT_SIG),
                    mmio_read(base + PORT_TFD),
                    mmio_read(base + PORT_CMD)
                );
            }
            let det = ssts & 0xf;
            let ipm = (ssts >> 8) & 0xf;
            if det == 3 && ipm == 1 {
                selected = Some((port, base));
                break;
            }
        }

        let Some((port, port_base)) = selected else {
            let port0 = AHCI_MMIO_BASE + HBA_PORTS;
            polyhal::println!(
                "[ahci] SATA unavailable: cap={:#x} pi={:#x} ssts={:#x} sig={:#x} tfd={:#x} cmd={:#x}",
                mmio_read(AHCI_MMIO_BASE + HBA_CAP),
                mmio_read(AHCI_MMIO_BASE + HBA_PI),
                mmio_read(port0 + PORT_SSTS),
                mmio_read(port0 + PORT_SIG),
                mmio_read(port0 + PORT_TFD),
                mmio_read(port0 + PORT_CMD),
            );
            return Err("no active SATA port");
        };
        stop_port(port_base)?;

        let clb = command_list.paddr();
        let fb = received_fis.paddr();
        let ctba = command_table.paddr();
        let bounce_pa = bounce.paddr();
        info!(
            "[ahci] port {} DMA clb={:#x}, fb={:#x}, ctba={:#x}, bounce={:#x}",
            port, clb, fb, ctba, bounce_pa
        );
        mmio_write(port_base + PORT_CLB, clb as u32);
        mmio_write(port_base + PORT_CLBU, (clb >> 32) as u32);
        mmio_write(port_base + PORT_FB, fb as u32);
        mmio_write(port_base + PORT_FBU, (fb >> 32) as u32);
        mmio_write(port_base + PORT_IE, 0);
        mmio_write(port_base + PORT_IS, u32::MAX);
        mmio_write(port_base + PORT_SERR, u32::MAX);

        let signature = mmio_read(port_base + PORT_SIG);
        if signature != 0 && signature != SATA_SIG_ATA {
            return Err("active AHCI port is not a SATA disk");
        }

        start_port(port_base);
        let mut inner = AhciInner {
            port_base,
            sectors: 0,
            command_list,
            _received_fis: received_fis,
            command_table,
            bounce,
        };
        inner.identify()?;
        info!(
            "[ahci] Loongson SATA port {} ready, sectors={:#x}, size={} MiB",
            port,
            inner.sectors,
            inner.sectors / 2048
        );
        Ok(Self {
            inner: SpinNoIrqLock::new(inner),
        })
    }
}

impl BlockDevice for AhciBlock {
    fn size(&self) -> u64 {
        self.inner.lock().sectors * BLOCK_SIZE as u64
    }

    fn block_size(&self) -> usize {
        BLOCK_SIZE
    }

    fn read_block(&self, block_id: usize, buf: &mut [u8]) {
        assert_eq!(buf.len() % BLOCK_SIZE, 0);
        let mut inner = self.inner.lock();
        let sectors = buf.len() / BLOCK_SIZE;
        assert!((block_id as u64) + sectors as u64 <= inner.sectors);

        let mut done = 0;
        while done < sectors {
            let chunk = min(MAX_SECTORS_PER_COMMAND, sectors - done);
            inner
                .transfer(block_id as u64 + done as u64, chunk, DmaDirection::Read)
                .unwrap_or_else(|err| panic!("AHCI read failed: {}", err));
            let bytes = chunk * BLOCK_SIZE;
            let bounce = inner.bounce.bytes();
            buf[done * BLOCK_SIZE..done * BLOCK_SIZE + bytes].copy_from_slice(&bounce[..bytes]);
            done += chunk;
        }
    }

    fn write_block(&self, block_id: usize, buf: &[u8]) {
        assert_eq!(buf.len() % BLOCK_SIZE, 0);
        let mut inner = self.inner.lock();
        let sectors = buf.len() / BLOCK_SIZE;
        assert!((block_id as u64) + sectors as u64 <= inner.sectors);

        let mut done = 0;
        while done < sectors {
            let chunk = min(MAX_SECTORS_PER_COMMAND, sectors - done);
            let bytes = chunk * BLOCK_SIZE;
            inner.bounce.bytes()[..bytes]
                .copy_from_slice(&buf[done * BLOCK_SIZE..done * BLOCK_SIZE + bytes]);
            inner
                .transfer(block_id as u64 + done as u64, chunk, DmaDirection::Write)
                .unwrap_or_else(|err| panic!("AHCI write failed: {}", err));
            done += chunk;
        }
    }
}

impl AhciInner {
    fn identify(&mut self) -> Result<(), &'static str> {
        self.issue(ATA_CMD_IDENTIFY, 0, 1, DmaDirection::Read)?;
        let data = self.bounce.bytes();
        let word83 = identify_word(data, 83);
        let sectors = if word83 & (1 << 10) != 0 {
            (identify_word(data, 100) as u64)
                | ((identify_word(data, 101) as u64) << 16)
                | ((identify_word(data, 102) as u64) << 32)
                | ((identify_word(data, 103) as u64) << 48)
        } else {
            (identify_word(data, 60) as u64) | ((identify_word(data, 61) as u64) << 16)
        };
        if sectors == 0 {
            return Err("SATA IDENTIFY returned zero capacity");
        }
        self.sectors = sectors;
        Ok(())
    }

    fn transfer(
        &mut self,
        lba: u64,
        sectors: usize,
        direction: DmaDirection,
    ) -> Result<(), &'static str> {
        let command = match direction {
            DmaDirection::Read => ATA_CMD_READ_DMA_EXT,
            DmaDirection::Write => ATA_CMD_WRITE_DMA_EXT,
        };
        self.issue(command, lba, sectors, direction)
    }

    fn issue(
        &mut self,
        command: u8,
        lba: u64,
        sectors: usize,
        direction: DmaDirection,
    ) -> Result<(), &'static str> {
        if sectors == 0 || sectors > MAX_SECTORS_PER_COMMAND {
            return Err("invalid AHCI sector count");
        }
        wait_until(POLL_LIMIT, || {
            mmio_read(self.port_base + PORT_TFD) & (ATA_DEV_BUSY | ATA_DEV_DRQ) == 0
        })
        .ok_or("SATA device stayed busy")?;

        let table_pa = self.command_table.paddr();
        let bounce_pa = self.bounce.paddr();
        let list = self.command_list.bytes();
        let table = self.command_table.bytes();
        list.fill(0);
        table.fill(0);

        let write_flag = match direction {
            DmaDirection::Read => 0,
            DmaDirection::Write => 1 << 6,
        };
        write_u32(list, 0, 5 | write_flag | (1 << 16));
        write_u32(list, 8, table_pa as u32);
        write_u32(list, 12, (table_pa >> 32) as u32);

        table[0] = FIS_TYPE_REG_H2D;
        table[1] = 1 << 7;
        table[2] = command;
        table[4] = lba as u8;
        table[5] = (lba >> 8) as u8;
        table[6] = (lba >> 16) as u8;
        table[7] = 1 << 6;
        table[8] = (lba >> 24) as u8;
        table[9] = (lba >> 32) as u8;
        table[10] = (lba >> 40) as u8;
        table[12] = sectors as u8;
        table[13] = (sectors >> 8) as u8;

        let bytes = sectors * BLOCK_SIZE;
        write_u32(table, COMMAND_TABLE_PRDT_OFFSET, bounce_pa as u32);
        write_u32(
            table,
            COMMAND_TABLE_PRDT_OFFSET + 4,
            (bounce_pa >> 32) as u32,
        );
        write_u32(
            table,
            COMMAND_TABLE_PRDT_OFFSET + 12,
            (bytes as u32 - 1) | (1 << 31),
        );

        dma_barrier();
        mmio_write(self.port_base + PORT_IS, u32::MAX);
        mmio_write(self.port_base + PORT_SERR, u32::MAX);
        if mmio_read(self.port_base + PORT_SACT) & 1 != 0 {
            return Err("AHCI command slot 0 is active");
        }
        mmio_write(self.port_base + PORT_CI, 1);

        wait_until(POLL_LIMIT, || {
            let is = mmio_read(self.port_base + PORT_IS);
            is & PORT_IS_TFES != 0 || mmio_read(self.port_base + PORT_CI) & 1 == 0
        })
        .ok_or("AHCI command timeout")?;
        dma_barrier();

        if mmio_read(self.port_base + PORT_IS) & PORT_IS_TFES != 0
            || mmio_read(self.port_base + PORT_TFD) & 1 != 0
        {
            return Err("SATA task-file error");
        }
        Ok(())
    }
}

fn reset_hba() -> Result<(), &'static str> {
    let ghc = mmio_read(AHCI_MMIO_BASE + HBA_GHC);
    mmio_write(AHCI_MMIO_BASE + HBA_GHC, ghc | GHC_AE | GHC_HR);
    wait_until(POLL_LIMIT, || {
        mmio_read(AHCI_MMIO_BASE + HBA_GHC) & GHC_HR == 0
    })
    .ok_or("AHCI controller reset timed out")?;
    mmio_write(
        AHCI_MMIO_BASE + HBA_GHC,
        mmio_read(AHCI_MMIO_BASE + HBA_GHC) | GHC_AE,
    );
    delay_us(1_000);
    Ok(())
}

fn link_is_active(ssts: u32) -> bool {
    ssts & 0xf == 3 && (ssts >> 8) & 0xf == 1
}

fn bring_up_link(base: usize) -> Result<(), &'static str> {
    stop_port(base)?;

    let tfd = mmio_read(base + PORT_TFD);
    if tfd & (ATA_DEV_BUSY | ATA_DEV_DRQ) != 0 {
        if mmio_read(AHCI_MMIO_BASE + HBA_CAP) & CAP_SCLO == 0 {
            return Err("SATA device is busy and controller has no CLO support");
        }
        mmio_write(base + PORT_CMD, mmio_read(base + PORT_CMD) | PORT_CMD_CLO);
        wait_until(POLL_LIMIT, || {
            mmio_read(base + PORT_CMD) & PORT_CMD_CLO == 0
        })
        .ok_or("AHCI command-list override timed out")?;
    }

    mmio_write(base + PORT_CMD, mmio_read(base + PORT_CMD) | PORT_CMD_SUD);
    wait_until(POLL_LIMIT, || {
        mmio_read(base + PORT_CMD) & PORT_CMD_SUD != 0
    })
    .ok_or("AHCI spin-up bit did not latch")?;

    wait_until(POLL_LIMIT, || {
        matches!(mmio_read(base + PORT_SSTS) & 0xf, 1 | 3)
    })
    .ok_or("SATA device presence detection timed out")?;
    mmio_write(base + PORT_SERR, u32::MAX);
    mmio_write(base + PORT_IS, u32::MAX);

    wait_until(POLL_LIMIT, || mmio_read(base + PORT_SSTS) & 0xf == 3)
        .ok_or("SATA link did not become active")
}

fn stop_port(base: usize) -> Result<(), &'static str> {
    let mut cmd = mmio_read(base + PORT_CMD);
    cmd &= !PORT_CMD_ST;
    mmio_write(base + PORT_CMD, cmd);
    wait_until(POLL_LIMIT, || mmio_read(base + PORT_CMD) & PORT_CMD_CR == 0)
        .ok_or("AHCI command engine did not stop")?;
    cmd = mmio_read(base + PORT_CMD) & !PORT_CMD_FRE;
    mmio_write(base + PORT_CMD, cmd);
    wait_until(POLL_LIMIT, || mmio_read(base + PORT_CMD) & PORT_CMD_FR == 0)
        .ok_or("AHCI FIS engine did not stop")?;
    Ok(())
}

fn start_port(base: usize) {
    let mut cmd = mmio_read(base + PORT_CMD) & !PORT_CMD_ICC_MASK;
    cmd |= PORT_CMD_FRE | PORT_CMD_ST | PORT_CMD_POD | PORT_CMD_SUD | PORT_CMD_ICC_ACTIVE;
    mmio_write(base + PORT_CMD, cmd);
}

fn identify_word(data: &[u8], word: usize) -> u16 {
    let offset = word * 2;
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn wait_until(mut spins: usize, mut condition: impl FnMut() -> bool) -> Option<()> {
    while spins != 0 {
        if condition() {
            return Some(());
        }
        core::hint::spin_loop();
        spins -= 1;
    }
    None
}

fn delay_us(microseconds: u64) {
    let frequency = polyhal::timer::get_freq();
    let ticks = frequency
        .saturating_mul(microseconds)
        .div_ceil(1_000_000)
        .max(1);
    let start = polyhal::timer::get_ticks();
    while polyhal::timer::get_ticks().wrapping_sub(start) < ticks {
        core::hint::spin_loop();
    }
}

fn dma_barrier() {
    unsafe { asm!("dbar 0", options(nostack, preserves_flags)) };
}

fn mmio_read(addr: usize) -> u32 {
    unsafe { read_volatile(addr as *const u32) }
}

fn mmio_write(addr: usize, value: u32) {
    unsafe { write_volatile(addr as *mut u32, value) }
    dma_barrier();
}
