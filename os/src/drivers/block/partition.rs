use crate::config::BLOCK_SIZE;
use crate::devices::BlockDevice;
use alloc::sync::Arc;
use log::{info, warn};
#[allow(unused)]
const GPT_HEADER_LBA: usize = 1;
#[allow(unused)]
const GPT_SIGNATURE: &[u8; 8] = b"EFI PART";
#[allow(unused)]
const GPT_HEADER_ENTRIES_LBA: usize = 72;
#[allow(unused)]
const GPT_HEADER_ENTRY_COUNT: usize = 80;
#[allow(unused)]
const GPT_HEADER_ENTRY_SIZE: usize = 84;
#[allow(unused)]
const GPT_ENTRY_FIRST_LBA: usize = 32;
#[allow(unused)]
const GPT_ENTRY_LAST_LBA: usize = 40;

pub struct PartitionBlock {
    inner: Arc<dyn BlockDevice>,
    start_lba: usize,
    block_count: usize,
}

impl PartitionBlock {
    #[allow(unused)]
    pub fn new(inner: Arc<dyn BlockDevice>, start_lba: usize, block_count: usize) -> Self {
        Self {
            inner,
            start_lba,
            block_count,
        }
    }
}

impl BlockDevice for PartitionBlock {
    fn size(&self) -> u64 {
        self.block_count as u64 * BLOCK_SIZE as u64
    }

    fn block_size(&self) -> usize {
        self.inner.block_size()
    }

    fn read_block(&self, block_id: usize, buf: &mut [u8]) {
        assert_eq!(buf.len() % BLOCK_SIZE, 0);
        let blocks = buf.len() / BLOCK_SIZE;
        let end_block = block_id
            .checked_add(blocks)
            .expect("partition read block range overflow");
        assert!(
            end_block <= self.block_count,
            "partition read out of range: block_id={} blocks={} partition_blocks={}",
            block_id,
            blocks,
            self.block_count
        );
        self.inner.read_block(self.start_lba + block_id, buf);
    }

    fn write_block(&self, block_id: usize, buf: &[u8]) {
        assert_eq!(buf.len() % BLOCK_SIZE, 0);
        let blocks = buf.len() / BLOCK_SIZE;
        let end_block = block_id
            .checked_add(blocks)
            .expect("partition write block range overflow");
        assert!(
            end_block <= self.block_count,
            "partition write out of range: block_id={} blocks={} partition_blocks={}",
            block_id,
            blocks,
            self.block_count
        );
        self.inner.write_block(self.start_lba + block_id, buf);
    }
}
#[allow(unused)]
pub fn gpt_partition(
    parent: Arc<dyn BlockDevice>,
    partition_number: usize,
) -> Option<Arc<dyn BlockDevice>> {
    if partition_number == 0 {
        return None;
    }

    let mut header = [0u8; BLOCK_SIZE];
    parent.read_block(GPT_HEADER_LBA, &mut header);
    if &header[..GPT_SIGNATURE.len()] != GPT_SIGNATURE {
        warn!("[gpt] no GPT signature on block device");
        return None;
    }

    let entries_lba = read_le_u64(&header, GPT_HEADER_ENTRIES_LBA)? as usize;
    let entry_count = read_le_u32(&header, GPT_HEADER_ENTRY_COUNT)? as usize;
    let entry_size = read_le_u32(&header, GPT_HEADER_ENTRY_SIZE)? as usize;
    if entry_size < GPT_ENTRY_LAST_LBA + 8 {
        warn!("[gpt] invalid entry size {}", entry_size);
        return None;
    }

    let entry_index = partition_number - 1;
    if entry_index >= entry_count {
        warn!(
            "[gpt] partition {} out of range, entries={}",
            partition_number, entry_count
        );
        return None;
    }

    let entry_offset = entry_index.checked_mul(entry_size)?;
    let block_id = entries_lba + entry_offset / BLOCK_SIZE;
    let offset_in_block = entry_offset % BLOCK_SIZE;
    if offset_in_block.checked_add(entry_size)? > BLOCK_SIZE * 2 {
        warn!(
            "[gpt] unsupported entry size {} at block offset {}",
            entry_size, offset_in_block
        );
        return None;
    }
    let mut block = [0u8; BLOCK_SIZE * 2];
    parent.read_block(block_id, &mut block);
    let entry = &block[offset_in_block..offset_in_block + entry_size];

    if entry[..16].iter().all(|byte| *byte == 0) {
        warn!("[gpt] partition {} is empty", partition_number);
        return None;
    }

    let first_lba = read_le_u64(entry, GPT_ENTRY_FIRST_LBA)? as usize;
    let last_lba = read_le_u64(entry, GPT_ENTRY_LAST_LBA)? as usize;
    if last_lba < first_lba {
        warn!(
            "[gpt] partition {} invalid range {}..{}",
            partition_number, first_lba, last_lba
        );
        return None;
    }

    let block_count = last_lba - first_lba + 1;
    info!(
        "[gpt] partition {} start_lba={:#x} blocks={:#x}",
        partition_number, first_lba, block_count
    );
    Some(Arc::new(PartitionBlock::new(
        parent,
        first_lba,
        block_count,
    )))
}
#[allow(unused)]
fn read_le_u32(buf: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        buf.get(offset..offset + 4)?.try_into().ok()?,
    ))
}
#[allow(unused)]
fn read_le_u64(buf: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        buf.get(offset..offset + 8)?.try_into().ok()?,
    ))
}
