use super::BlockDevice;
use crate::config::BLOCK_SIZE;
use core::slice;

pub struct RamDisk {
    start: usize,
    len: usize,
}

impl RamDisk {
    pub const fn new(start: usize, len: usize) -> Self {
        Self { start, len }
    }

    fn bytes(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self.start as *const u8, self.len) }
    }

    fn bytes_mut(&self) -> &mut [u8] {
        unsafe { slice::from_raw_parts_mut(self.start as *mut u8, self.len) }
    }
}

impl BlockDevice for RamDisk {
    fn size(&self) -> u64 {
        self.len as u64
    }

    fn block_size(&self) -> usize {
        BLOCK_SIZE
    }

    fn read_block(&self, block_id: usize, buf: &mut [u8]) {
        assert_eq!(buf.len() % BLOCK_SIZE, 0);
        let offset = block_id
            .checked_mul(BLOCK_SIZE)
            .expect("ramdisk read offset overflow");
        let end = offset
            .checked_add(buf.len())
            .expect("ramdisk read end overflow");
        assert!(
            end <= self.len,
            "ramdisk read out of range: block_id={}, len={}, disk_len={}",
            block_id,
            buf.len(),
            self.len
        );
        buf.copy_from_slice(&self.bytes()[offset..end]);
    }

    fn write_block(&self, block_id: usize, buf: &[u8]) {
        assert_eq!(buf.len() % BLOCK_SIZE, 0);
        let offset = block_id
            .checked_mul(BLOCK_SIZE)
            .expect("ramdisk write offset overflow");
        let end = offset
            .checked_add(buf.len())
            .expect("ramdisk write end overflow");
        assert!(
            end <= self.len,
            "ramdisk write out of range: block_id={}, len={}, disk_len={}",
            block_id,
            buf.len(),
            self.len
        );
        self.bytes_mut()[offset..end].copy_from_slice(buf);
    }
}
