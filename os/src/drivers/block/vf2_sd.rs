use crate::config::BLOCK_SIZE;
use crate::devices::BlockDevice;
use crate::println;
use crate::sync::SleepLock;
use log::{info, warn};
use polyhal::consts::VIRT_ADDR_START;

const SDIO1_BASE: usize = VIRT_ADDR_START + 0x1602_0000;

const CTRL: usize = 0x000;
const PWREN: usize = 0x004;
const CLKDIV: usize = 0x008;
const CLKSRC: usize = 0x00c;
const CLKENA: usize = 0x010;
const TMOUT: usize = 0x014;
const CTYPE: usize = 0x018;
const BLKSIZ: usize = 0x01c;
const BYTCNT: usize = 0x020;
const INTMASK: usize = 0x024;
const CMDARG: usize = 0x028;
const CMD: usize = 0x02c;
const RESP0: usize = 0x030;
const RINTSTS: usize = 0x044;
const STATUS: usize = 0x048;
const FIFOTH: usize = 0x04c;
const UHS_REG: usize = 0x074;
const BMOD: usize = 0x080;
const IDSTS: usize = 0x08c;
const IDINTEN: usize = 0x090;
const DATA: usize = 0x200;

const CTRL_RESET: u32 = 1 << 0;
const CTRL_FIFO_RESET: u32 = 1 << 1;
const CTRL_DMA_RESET: u32 = 1 << 2;
const CTRL_INT_ENABLE: u32 = 1 << 4;

const CMD_RESP_EXP: u32 = 1 << 6;
const CMD_RESP_LONG: u32 = 1 << 7;
const CMD_RESP_CRC: u32 = 1 << 8;
const CMD_DATA_EXP: u32 = 1 << 9;
const CMD_WRITE: u32 = 1 << 10;
const CMD_SEND_INIT: u32 = 1 << 15;
const CMD_UPD_CLK: u32 = 1 << 21;
const CMD_USE_HOLD: u32 = 1 << 29;
const CMD_START: u32 = 1 << 31;

const INT_CMD_DONE: u32 = 1 << 2;
const INT_DATA_OVER: u32 = 1 << 3;
const INT_TXDR: u32 = 1 << 4;
const INT_RXDR: u32 = 1 << 5;
const INT_ERR: u32 = (1 << 1)
    | (1 << 6)
    | (1 << 7)
    | (1 << 8)
    | (1 << 9)
    | (1 << 10)
    | (1 << 11)
    | (1 << 12)
    | (1 << 13)
    | (1 << 15);
const INT_ALL: u32 = 0xffff_ffff;

const STATUS_FIFO_FULL: u32 = 1 << 3;
const STATUS_DATA_BUSY: u32 = 1 << 9;
const STATUS_FIFO_COUNT_SHIFT: u32 = 17;
const STATUS_FIFO_COUNT_MASK: u32 = 0x1fff << STATUS_FIFO_COUNT_SHIFT;

const OCR_BUSY: u32 = 1 << 31;
const OCR_HCS: u32 = 1 << 30;
const OCR_33_34: u32 = 1 << 20;
const OCR_32_33: u32 = 1 << 19;
const OCR_31_32: u32 = 1 << 18;
const OCR_30_31: u32 = 1 << 17;
const OCR_29_30: u32 = 1 << 16;
const OCR_VOLTAGE: u32 = OCR_29_30 | OCR_30_31 | OCR_31_32 | OCR_32_33 | OCR_33_34;

const RCA_SHIFT: u32 = 16;
const CMD_WAIT_LIMIT: usize = 10_000_000;
const DATA_WAIT_LIMIT: usize = 50_000_000;

type SdResult<T> = Result<T, &'static str>;

#[derive(Clone, Copy)]
struct SdResponse {
    r: [u32; 4],
}

struct Vf2SdHost {
    base: usize,
    rca: u32,
    block_addressing: bool,
    capacity_bytes: u64,
}

pub struct Vf2SdBlock {
    inner: SleepLock<Vf2SdHost>,
}

impl Vf2SdBlock {
    pub fn try_new() -> SdResult<Self> {
        let mut host = Vf2SdHost::new(SDIO1_BASE);
        host.init_card()?;
        Ok(Self {
            inner: SleepLock::new(host),
        })
    }

    pub fn new() -> Self {
        Self::try_new().expect("vf2-sd: init failed")
    }
}

#[allow(dead_code)]
pub fn smoke_test_read_headers() {
    println!("[vf2-sd] smoke test: init raw SD device");
    let dev = Vf2SdBlock::new();
    let mut block = [0u8; BLOCK_SIZE];
    dev.read_block(0, &mut block);
    println!("[vf2-sd] lba0 tail={:02x} {:02x}", block[510], block[511]);
    dev.read_block(1, &mut block);
    println!("[vf2-sd] lba1 first8={:02x?}", &block[..8]);
}

impl BlockDevice for Vf2SdBlock {
    fn size(&self) -> u64 {
        self.inner.lock().capacity_bytes
    }

    fn block_size(&self) -> usize {
        BLOCK_SIZE
    }

    fn read_block(&self, block_id: usize, buf: &mut [u8]) {
        assert_ne!(buf.len(), 0);
        assert_eq!(buf.len() % BLOCK_SIZE, 0);
        let mut host = self.inner.lock();
        for (i, chunk) in buf.chunks_mut(BLOCK_SIZE).enumerate() {
            host.read_single_block(block_id as u32 + i as u32, chunk)
                .expect("vf2-sd: read block failed");
        }
    }

    fn write_block(&self, block_id: usize, buf: &[u8]) {
        assert_ne!(buf.len(), 0);
        assert_eq!(buf.len() % BLOCK_SIZE, 0);
        let mut host = self.inner.lock();
        for (i, chunk) in buf.chunks(BLOCK_SIZE).enumerate() {
            host.write_single_block(block_id as u32 + i as u32, chunk)
                .expect("vf2-sd: write block failed");
        }
    }
}

impl Vf2SdHost {
    fn new(base: usize) -> Self {
        Self {
            base,
            rca: 0,
            block_addressing: false,
            capacity_bytes: 0,
        }
    }

    fn init_card(&mut self) -> SdResult<()> {
        println!("[vf2-sd] init controller");
        self.ctrl_reset()?;
        self.wr(PWREN, 1);
        self.wr(INTMASK, 0);
        self.wr(RINTSTS, INT_ALL);
        self.wr(IDSTS, INT_ALL);
        self.wr(IDINTEN, 0);
        self.wr(BMOD, 0);
        self.wr(TMOUT, 0xffff_ffff);
        self.wr(FIFOTH, (0x2 << 28) | (0x10 << 16) | 0x10);
        self.set_bus_width_1();
        self.set_clock_div(495)?;

        println!("[vf2-sd] reset card");
        let _ = self.cmd(0, 0, CMD_SEND_INIT, RespType::None)?;
        let r7 = self.cmd(8, 0x1aa, 0, RespType::ShortCrc)?.r[0];
        if (r7 & 0xfff) != 0x1aa {
            warn!("[vf2-sd] CMD8 unexpected response {:#x}", r7);
        }

        println!("[vf2-sd] wait card ready");
        let mut ocr = 0;
        for _ in 0..10_000 {
            let _ = self.cmd(55, 0, 0, RespType::ShortCrc)?;
            ocr = self.cmd(41, OCR_HCS | OCR_VOLTAGE, 0, RespType::Short)?.r[0];
            if ocr & OCR_BUSY != 0 {
                break;
            }
            spin_delay(10_000);
        }
        if ocr & OCR_BUSY == 0 {
            warn!("[vf2-sd] ACMD41 timeout, ocr={:#x}", ocr);
            return Err("ACMD41 timeout");
        }
        self.block_addressing = ocr & OCR_HCS != 0;

        println!("[vf2-sd] identify card");
        let cid = self.cmd(2, 0, 0, RespType::LongCrc)?;
        let rca_resp = self.cmd(3, 0, 0, RespType::ShortCrc)?.r[0];
        self.rca = rca_resp >> RCA_SHIFT;
        if self.rca == 0 {
            warn!("[vf2-sd] invalid RCA response {:#x}", rca_resp);
            return Err("invalid RCA");
        }
        let csd = self.cmd(9, self.rca << RCA_SHIFT, 0, RespType::LongCrc)?;
        self.capacity_bytes = parse_csd_capacity(csd.r).unwrap_or(0);
        info!(
            "[vf2-sd] card ready rca={} block_addressing={} capacity={} MiB cid0={:#x}",
            self.rca,
            self.block_addressing,
            self.capacity_bytes / 1024 / 1024,
            cid.r[0]
        );

        println!(
            "[vf2-sd] card ready rca={} capacity={} MiB",
            self.rca,
            self.capacity_bytes / 1024 / 1024
        );
        let _ = self.cmd(7, self.rca << RCA_SHIFT, 0, RespType::ShortCrc)?;
        let _ = self.cmd(16, BLOCK_SIZE as u32, 0, RespType::ShortCrc)?;
        self.set_clock_div(4)?;
        Ok(())
    }

    fn read_single_block(&mut self, block_id: u32, buf: &mut [u8]) -> SdResult<()> {
        assert_eq!(buf.len(), BLOCK_SIZE);
        self.wait_not_busy()?;
        self.wr(BLKSIZ, BLOCK_SIZE as u32);
        self.wr(BYTCNT, BLOCK_SIZE as u32);
        self.wr(RINTSTS, INT_ALL);

        let addr = self.card_addr(block_id);
        let _ = self.cmd(17, addr, CMD_DATA_EXP | CMD_USE_HOLD, RespType::ShortCrc)?;

        let mut offset = 0usize;
        for _ in 0..DATA_WAIT_LIMIT {
            let pending = self.rd(RINTSTS);
            if pending & INT_ERR != 0 {
                warn!("[vf2-sd] read interrupt error {:#x}", pending);
                return Err("read interrupt error");
            }
            if self.fifo_count() == 0 {
                core::hint::spin_loop();
                continue;
            }
            let word = self.rd(DATA).to_le_bytes();
            let end = (offset + 4).min(BLOCK_SIZE);
            buf[offset..end].copy_from_slice(&word[..end - offset]);
            offset = end;
            if offset >= BLOCK_SIZE {
                self.wait_data_done()?;
                return Ok(());
            }
        }
        warn!(
            "[vf2-sd] read timeout block={} offset={} rint={:#x} status={:#x}",
            block_id,
            offset,
            self.rd(RINTSTS),
            self.rd(STATUS)
        );
        Err("read timeout")
    }

    fn write_single_block(&mut self, block_id: u32, buf: &[u8]) -> SdResult<()> {
        assert_eq!(buf.len(), BLOCK_SIZE);
        self.wait_not_busy()?;
        self.wr(BLKSIZ, BLOCK_SIZE as u32);
        self.wr(BYTCNT, BLOCK_SIZE as u32);
        self.wr(RINTSTS, INT_ALL);

        let addr = self.card_addr(block_id);
        let _ = self.cmd(
            24,
            addr,
            CMD_DATA_EXP | CMD_WRITE | CMD_USE_HOLD,
            RespType::ShortCrc,
        )?;

        let mut offset = 0usize;
        for _ in 0..DATA_WAIT_LIMIT {
            let pending = self.rd(RINTSTS);
            if pending & INT_ERR != 0 {
                warn!("[vf2-sd] write interrupt error {:#x}", pending);
                return Err("write interrupt error");
            }
            if self.rd(STATUS) & STATUS_FIFO_FULL != 0 {
                core::hint::spin_loop();
                continue;
            }
            let mut word = [0u8; 4];
            let end = (offset + 4).min(BLOCK_SIZE);
            word[..end - offset].copy_from_slice(&buf[offset..end]);
            self.wr(DATA, u32::from_le_bytes(word));
            offset = end;
            if offset >= BLOCK_SIZE {
                self.wait_data_done()?;
                return Ok(());
            }
        }
        warn!(
            "[vf2-sd] write timeout block={} offset={} rint={:#x} status={:#x}",
            block_id,
            offset,
            self.rd(RINTSTS),
            self.rd(STATUS)
        );
        Err("write timeout")
    }

    fn card_addr(&self, block_id: u32) -> u32 {
        if self.block_addressing {
            block_id
        } else {
            block_id * BLOCK_SIZE as u32
        }
    }

    fn cmd(&mut self, idx: u32, arg: u32, flags: u32, resp: RespType) -> SdResult<SdResponse> {
        self.wait_cmd_idle()?;
        self.wr(RINTSTS, INT_ALL);
        self.wr(CMDARG, arg);
        let cmd = CMD_START | flags | resp.cmd_flags() | idx;
        self.wr(CMD, cmd);

        for _ in 0..CMD_WAIT_LIMIT {
            let pending = self.rd(RINTSTS);
            if pending & INT_ERR != 0 {
                warn!(
                    "[vf2-sd] cmd{} error int={:#x} arg={:#x} status={:#x}",
                    idx,
                    pending,
                    arg,
                    self.rd(STATUS)
                );
                return Err("command interrupt error");
            }
            if pending & INT_CMD_DONE != 0 {
                self.wr(RINTSTS, INT_CMD_DONE);
                return Ok(SdResponse {
                    r: [
                        self.rd(RESP0),
                        self.rd(RESP0 + 4),
                        self.rd(RESP0 + 8),
                        self.rd(RESP0 + 12),
                    ],
                });
            }
            core::hint::spin_loop();
        }
        warn!(
            "[vf2-sd] cmd{} timeout arg={:#x} cmd={:#x} rint={:#x} status={:#x}",
            idx,
            arg,
            self.rd(CMD),
            self.rd(RINTSTS),
            self.rd(STATUS)
        );
        Err("command timeout")
    }

    fn ctrl_reset(&mut self) -> SdResult<()> {
        self.wr(CTRL, CTRL_RESET | CTRL_FIFO_RESET | CTRL_DMA_RESET);
        for _ in 0..100_000 {
            if self.rd(CTRL) & (CTRL_RESET | CTRL_FIFO_RESET | CTRL_DMA_RESET) == 0 {
                self.wr(CTRL, CTRL_INT_ENABLE);
                return Ok(());
            }
            core::hint::spin_loop();
        }
        warn!(
            "[vf2-sd] controller reset timeout ctrl={:#x}",
            self.rd(CTRL)
        );
        Err("controller reset timeout")
    }

    fn set_bus_width_1(&mut self) {
        self.wr(CTYPE, 0);
        self.wr(UHS_REG, 0);
    }

    fn set_clock_div(&mut self, div: u32) -> SdResult<()> {
        self.wr(CLKENA, 0);
        self.update_clock()?;
        self.wr(CLKSRC, 0);
        self.wr(CLKDIV, div);
        self.update_clock()?;
        self.wr(CLKENA, 1);
        self.update_clock()
    }

    fn update_clock(&mut self) -> SdResult<()> {
        self.wr(CMD, CMD_START | CMD_UPD_CLK | CMD_WAIT_PRVDATA);
        for _ in 0..100_000 {
            if self.rd(CMD) & CMD_START == 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        warn!("[vf2-sd] update clock timeout cmd={:#x}", self.rd(CMD));
        Err("update clock timeout")
    }

    fn wait_cmd_idle(&self) -> SdResult<()> {
        for _ in 0..100_000 {
            if self.rd(CMD) & CMD_START == 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        warn!("[vf2-sd] command engine busy cmd={:#x}", self.rd(CMD));
        Err("command engine busy")
    }

    fn wait_not_busy(&self) -> SdResult<()> {
        for _ in 0..1_000_000 {
            if self.rd(STATUS) & STATUS_DATA_BUSY == 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        warn!("[vf2-sd] data busy timeout status={:#x}", self.rd(STATUS));
        Err("data busy timeout")
    }

    fn wait_data_done(&mut self) -> SdResult<()> {
        for _ in 0..DATA_WAIT_LIMIT {
            let pending = self.rd(RINTSTS);
            if pending & INT_ERR != 0 {
                warn!("[vf2-sd] data interrupt error {:#x}", pending);
                return Err("data interrupt error");
            }
            if pending & INT_DATA_OVER != 0 {
                self.wr(RINTSTS, INT_DATA_OVER | INT_RXDR | INT_TXDR);
                return Ok(());
            }
            core::hint::spin_loop();
        }
        warn!(
            "[vf2-sd] data done timeout rint={:#x} status={:#x}",
            self.rd(RINTSTS),
            self.rd(STATUS)
        );
        Err("data done timeout")
    }

    fn fifo_count(&self) -> u32 {
        (self.rd(STATUS) & STATUS_FIFO_COUNT_MASK) >> STATUS_FIFO_COUNT_SHIFT
    }

    #[inline]
    fn rd(&self, offset: usize) -> u32 {
        unsafe { ((self.base + offset) as *const u32).read_volatile() }
    }

    #[inline]
    fn wr(&self, offset: usize, value: u32) {
        unsafe { ((self.base + offset) as *mut u32).write_volatile(value) }
    }
}

#[derive(Clone, Copy)]
enum RespType {
    None,
    Short,
    ShortCrc,
    LongCrc,
}

impl RespType {
    fn cmd_flags(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Short => CMD_RESP_EXP,
            Self::ShortCrc => CMD_RESP_EXP | CMD_RESP_CRC,
            Self::LongCrc => CMD_RESP_EXP | CMD_RESP_LONG | CMD_RESP_CRC,
        }
    }
}

const CMD_WAIT_PRVDATA: u32 = 1 << 13;

fn parse_csd_capacity(resp: [u32; 4]) -> Option<u64> {
    let mut csd = [0u8; 16];
    csd[0..4].copy_from_slice(&resp[3].to_be_bytes());
    csd[4..8].copy_from_slice(&resp[2].to_be_bytes());
    csd[8..12].copy_from_slice(&resp[1].to_be_bytes());
    csd[12..16].copy_from_slice(&resp[0].to_be_bytes());

    let csd_structure = get_bits(&csd, 127, 126);
    match csd_structure {
        1 => {
            let c_size = get_bits(&csd, 69, 48) as u64;
            Some((c_size + 1) * 512 * 1024)
        }
        0 => {
            let read_bl_len = get_bits(&csd, 83, 80) as u64;
            let c_size = get_bits(&csd, 73, 62) as u64;
            let c_size_mult = get_bits(&csd, 49, 47) as u64;
            let block_len = 1u64 << read_bl_len;
            let mult = 1u64 << (c_size_mult + 2);
            Some((c_size + 1) * mult * block_len)
        }
        _ => None,
    }
}

fn get_bits(bytes: &[u8; 16], msb: u32, lsb: u32) -> u32 {
    let mut value = 0u32;
    for bit in (lsb..=msb).rev() {
        let byte_index = (15 - bit / 8) as usize;
        let bit_index = bit % 8;
        value = (value << 1) | (((bytes[byte_index] >> bit_index) & 1) as u32);
    }
    value
}

fn spin_delay(iterations: usize) {
    for _ in 0..iterations {
        core::hint::spin_loop();
    }
}
