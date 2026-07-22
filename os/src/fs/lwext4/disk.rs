//! necessary device implementation for ext4 filesystem
//参考chronix实现
use alloc::sync::Arc;
use lwext4_rust::KernelDevOp;

use virtio_drivers::device::blk::VirtIOBlk;
use virtio_drivers::transport::mmio::{MmioTransport, VirtIOHeader};
use virtio_drivers::transport::{DeviceType, Transport};

use crate::logging;
use log::*;

use crate::devices::BlockDevice;

const BLOCK_SIZE: usize = 512;

/// A disk device with a cursor.
pub struct Disk {
    block_id: usize,
    offset: usize,
    dev: Arc<dyn BlockDevice>,
}

impl Disk {
    /// Create a new disk.
    pub fn new(dev: Arc<dyn BlockDevice>) -> Self {
        Self {
            block_id: 0,
            offset: 0,
            dev,
        }
    }

    /// Get the size of the disk.
    /// capacity() 以512 byte为单位
    pub fn size(&self) -> u64 {
        self.dev.size()
    }

    /// Get the current position of the disk cursor in bytes.
    pub fn position(&self) -> u64 {
        (self.block_id * BLOCK_SIZE + self.offset) as u64
    }

    /// Set the position of the disk cursor in bytes.
    pub fn set_position(&mut self, pos: u64) {
        self.block_id = pos as usize / BLOCK_SIZE;
        self.offset = pos as usize % BLOCK_SIZE;
    }

    ///  Read within one block, returns the number of bytes read.
    pub fn read_one(&mut self, buf: &mut [u8]) -> Result<usize, i32> {
        // info!("block id: {}", self.block_id);
        let read_size = if self.offset == 0 && buf.len() >= BLOCK_SIZE {
            // whole contiguous blocks
            let blocks = buf.len() / BLOCK_SIZE;
            let count = blocks * BLOCK_SIZE;
            self.dev.read_block(self.block_id, &mut buf[..count]);
            self.block_id += blocks;
            count
        } else {
            // partial block
            let mut data = [0u8; BLOCK_SIZE];
            let start = self.offset;
            let count = buf.len().min(BLOCK_SIZE - self.offset);
            if start > BLOCK_SIZE {
                info!("block size: {} start {}", BLOCK_SIZE, start);
            }

            self.dev.read_block(self.block_id, &mut data);
            buf[..count].copy_from_slice(&data[start..start + count]);

            self.offset += count;
            if self.offset >= BLOCK_SIZE {
                self.block_id += 1;
                self.offset -= BLOCK_SIZE;
            }
            count
        };
        Ok(read_size)
    }

    /// Write within one block, returns the number of bytes written.
    pub fn write_one(&mut self, buf: &[u8]) -> Result<usize, i32> {
        let write_size = if self.offset == 0 && buf.len() >= BLOCK_SIZE {
            // whole contiguous blocks
            let blocks = buf.len() / BLOCK_SIZE;
            let count = blocks * BLOCK_SIZE;
            self.dev.write_block(self.block_id, &buf[..count]);
            self.block_id += blocks;
            count
        } else {
            // partial block
            let mut data = [0u8; BLOCK_SIZE];
            let start = self.offset;
            let count = buf.len().min(BLOCK_SIZE - self.offset);

            self.dev.read_block(self.block_id, &mut data);
            data[start..start + count].copy_from_slice(&buf[..count]);
            self.dev.write_block(self.block_id, &data);

            self.offset += count;
            if self.offset >= BLOCK_SIZE {
                self.block_id += 1;
                self.offset -= BLOCK_SIZE;
            }
            count
        };
        Ok(write_size)
    }
}

impl KernelDevOp for Disk {
    //type DevType = Box<Disk>;
    type DevType = Disk;

    fn read_at(dev: &Self, offset: u64, buf: &mut [u8]) -> Result<usize, i32> {
        debug!("READ block device buf={}", buf.len());
        if offset % BLOCK_SIZE as u64 != 0 || buf.len() % BLOCK_SIZE != 0 {
            return Err(-22);
        }
        dev.dev.read_block(offset as usize / BLOCK_SIZE, buf);
        debug!("READ rt len={}", buf.len());
        Ok(buf.len())
    }
    fn write_at(dev: &Self, offset: u64, buf: &[u8]) -> Result<usize, i32> {
        debug!("WRITE block device buf={}", buf.len());
        if offset % BLOCK_SIZE as u64 != 0 || buf.len() % BLOCK_SIZE != 0 {
            return Err(-22);
        }
        dev.dev.write_block(offset as usize / BLOCK_SIZE, buf);
        debug!("WRITE rt len={}", buf.len());
        Ok(buf.len())
    }
    fn size(dev: &Self) -> Result<u64, i32> {
        Ok(dev.size())
    }
    fn flush(dev: &Self::DevType) -> Result<usize, i32> {
        dev.dev.flush().map(|_| 0).map_err(|err| -(err as i32))
    }
}
