use core::any::Any;

use crate::error::SysResult;
/// Trait for block devices
/// which reads and writes data in the unit of blocks
pub trait BlockDevice: Send + Sync + Any {
    fn size(&self) -> u64;

    fn block_size(&self) -> usize;

    ///Read data form block to buffer
    fn read_block(&self, block_id: usize, buf: &mut [u8]);
    ///Write data from buffer to block
    fn write_block(&self, block_id: usize, buf: &[u8]);

    /// Force completed writes through any volatile device cache.
    fn flush(&self) -> SysResult<()> {
        Ok(())
    }
}
