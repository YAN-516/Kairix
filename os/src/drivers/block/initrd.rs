use super::virtio_blk::VirtIOBlock;
use crate::devices::BlockDevice;
use alloc::sync::Arc;
use log::info;

const BLOCK_SIZE: usize = 512;
#[allow(unused)]
const INITRD_VADDR: usize = 0x9000_0000_9004_0000;
#[allow(unused)]
const INITRD_MAX_SIZE: usize = 0x0200_0000;
#[allow(unused)]
const EXT_SUPER_OFFSET: usize = 1024;
#[allow(unused)]
const EXT_MAGIC_OFFSET: usize = EXT_SUPER_OFFSET + 56;
#[allow(unused)]
const EXT_BLOCKS_COUNT_LO_OFFSET: usize = EXT_SUPER_OFFSET + 4;
#[allow(unused)]
const EXT_LOG_BLOCK_SIZE_OFFSET: usize = EXT_SUPER_OFFSET + 24;
#[allow(unused)]
const EXT_MAGIC: u16 = 0xef53;

pub struct BootBlock {
    inner: Arc<dyn BlockDevice>,
}

impl BootBlock {
    #[allow(unused)]
    pub fn new() -> Self {
        if let Some(initrd) = InitrdBlock::try_new() {
            info!(
                "Using initrd block device at {:#x}, size {:#x}",
                INITRD_VADDR, initrd.len
            );
            return Self {
                inner: Arc::new(initrd),
            };
        }

        Self {
            inner: Arc::new(VirtIOBlock::new()),
        }
    }
}

impl BlockDevice for BootBlock {
    fn size(&self) -> u64 {
        self.inner.size()
    }

    fn block_size(&self) -> usize {
        self.inner.block_size()
    }

    fn read_block(&self, block_id: usize, buf: &mut [u8]) {
        self.inner.read_block(block_id, buf)
    }

    fn write_block(&self, block_id: usize, buf: &[u8]) {
        self.inner.write_block(block_id, buf)
    }
}

pub struct InitrdBlock {
    start: usize,
    len: usize,
}

impl InitrdBlock {
    #[allow(unused)]
    fn try_new() -> Option<Self> {
        let magic = unsafe { read_le_u16(INITRD_VADDR + EXT_MAGIC_OFFSET) };
        if magic != EXT_MAGIC {
            let gzip_magic = unsafe { read_le_u16(INITRD_VADDR) };
            if gzip_magic == 0x8b1f {
                panic!(
                    "initrd at {:#x} is gzip-compressed; put decompressed raw ext2/ext4 image at /install/ramdisk.gz",
                    INITRD_VADDR
                );
            }
            return None;
        }

        let blocks = unsafe { read_le_u32(INITRD_VADDR + EXT_BLOCKS_COUNT_LO_OFFSET) as usize };
        let log_block_size =
            unsafe { read_le_u32(INITRD_VADDR + EXT_LOG_BLOCK_SIZE_OFFSET) as usize };
        let block_size = 1024usize.checked_shl(log_block_size as u32)?;
        let fs_size = blocks.checked_mul(block_size)?;
        let len = fs_size.min(INITRD_MAX_SIZE);

        Some(Self {
            start: INITRD_VADDR,
            len,
        })
    }
}

impl BlockDevice for InitrdBlock {
    fn size(&self) -> u64 {
        self.len as u64
    }

    fn block_size(&self) -> usize {
        BLOCK_SIZE
    }

    fn read_block(&self, block_id: usize, buf: &mut [u8]) {
        assert_eq!(buf.len() % BLOCK_SIZE, 0);
        let offset = block_id * BLOCK_SIZE;
        if offset >= self.len {
            buf.fill(0);
            return;
        }

        let count = buf.len().min(self.len - offset);
        unsafe {
            core::ptr::copy_nonoverlapping(
                (self.start + offset) as *const u8,
                buf.as_mut_ptr(),
                count,
            );
        }
        if count < buf.len() {
            buf[count..].fill(0);
        }
    }

    fn write_block(&self, block_id: usize, buf: &[u8]) {
        assert_eq!(buf.len() % BLOCK_SIZE, 0);
        let offset = block_id * BLOCK_SIZE;
        if offset >= self.len {
            return;
        }

        let count = buf.len().min(self.len - offset);
        unsafe {
            core::ptr::copy_nonoverlapping(buf.as_ptr(), (self.start + offset) as *mut u8, count);
        }
    }
}

#[allow(unused)]
unsafe fn read_le_u16(addr: usize) -> u16 {
    unsafe {
        u16::from_le_bytes([
            (addr as *const u8).read_volatile(),
            ((addr + 1) as *const u8).read_volatile(),
        ])
    }
}
#[allow(unused)]
unsafe fn read_le_u32(addr: usize) -> u32 {
    unsafe {
        u32::from_le_bytes([
            (addr as *const u8).read_volatile(),
            ((addr + 1) as *const u8).read_volatile(),
            ((addr + 2) as *const u8).read_volatile(),
            ((addr + 3) as *const u8).read_volatile(),
        ])
    }
}
