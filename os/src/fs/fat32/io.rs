use crate::devices::BlockDevice;
use alloc::sync::Arc;
use fatfs::{Read,IoBase,Write};
use alloc::vec;
use fatfs::SeekFrom;
pub struct FatIoAdapter {
    device: Arc<dyn BlockDevice>,
    offset: u64, 
}

impl FatIoAdapter {
    pub fn new(device: Arc<dyn BlockDevice>) -> Self {
        Self { device, offset: 0 }
    }
}

impl IoBase for FatIoAdapter {
    type Error = ();
}

impl Read for FatIoAdapter {
    //读取一个块的数据到buf中，返回实际读取的字节数
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        let block_size = self.device.block_size() as usize;
        let mut read_bytes = 0;
        let mut current_offset = self.offset as usize;
        let mut temp_buf = vec![0u8; block_size];
        while read_bytes < buf.len() {
            let block_id = current_offset / block_size;
            let offset_in_block = current_offset % block_size;
            let copy_len = (block_size - offset_in_block).min(buf.len() - read_bytes);
            self.device.read_block(block_id, &mut temp_buf);
            buf[read_bytes .. read_bytes + copy_len]
                .copy_from_slice(&temp_buf[offset_in_block .. offset_in_block + copy_len]);
            read_bytes += copy_len;
            current_offset += copy_len;
        }
        self.offset = current_offset as u64;
        Ok(read_bytes)
    }
}

impl Write for FatIoAdapter {
    //
    fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        let block_size = self.device.block_size() as usize;
        let mut written_bytes = 0;
        let mut current_offset = self.offset as usize;
        let mut temp_buf = vec![0u8; block_size];//装载块本身的数据，防止被覆盖
        while written_bytes < buf.len() {
            let block_id = current_offset / block_size;
            let offset_in_block = current_offset % block_size;
            let copy_len = (block_size - offset_in_block).min(buf.len() - written_bytes);
            if copy_len < block_size {
                //覆盖之前的脏数据
                self.device.read_block(block_id, &mut temp_buf);
            }
            //拼接数据，如果copy_len == block_size 也是成功覆盖之前的数据
            temp_buf[offset_in_block .. offset_in_block + copy_len]
                .copy_from_slice(&buf[written_bytes .. written_bytes + copy_len]);
            self.device.write_block(block_id, &temp_buf);
            written_bytes += copy_len;
            current_offset += copy_len;
        }
        self.offset = current_offset as u64;
        Ok(written_bytes)
    }
    fn flush(&mut self) -> Result<(), Self::Error> {
        // 因为块设备写入通常是直接落盘（或者由 QEMU 底层处理了缓存）
        Ok(())
    }
}

impl fatfs::Seek for FatIoAdapter {
     fn seek(&mut self, pos: SeekFrom) -> Result<u64, Self::Error> {
        // 根据要求移动指针位置
        let new_offset = match pos {
            SeekFrom::Start(off) => off as i64,
            SeekFrom::Current(off) => self.offset as i64 + off,
            SeekFrom::End(off) => (self.device.size() as i64) + off, // 需要你的 BlockDevice 接口支持 .size() 返回总字节数
        };
        if new_offset < 0 {
            return Err(());
        }

        self.offset = new_offset as u64;
        Ok(self.offset)
    }
}