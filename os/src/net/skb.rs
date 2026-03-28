use crate::net::device::NetDevice;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
/// 网络数据包缓冲区
pub struct Skb {
    pub data: Vec<u8>,
    // 数据范围
    pub head: usize,
    pub tail: usize,
    // 协议头位置
    pub network_header: usize,
    pub transport_header: usize,
    // 元数据
    pub dev: Option<Arc<dyn NetDevice>>,
    pub len: usize,
}

impl Skb {
    /// 创建新的skb
    pub fn new(size: usize) -> Self {
        let mut data = Vec::with_capacity(size);
        data.resize(size, 0);

        Self {
            data,
            head: 0,
            tail: 0,
            network_header: 0,
            transport_header: 0,
            dev: None,
            len: 0,
        }
    }

    /// 预留头部空间
    pub fn reserve(&mut self, size: usize) {
        if size > self.head {
            // 简化实现：重新分配并移动数据
            let new_data = vec![0u8; self.data.len() + size];
            let old_data = core::mem::replace(&mut self.data, new_data);
            let offset = size;
            self.data[offset..offset + self.len].copy_from_slice(&old_data[self.head..self.tail]);
            self.head = offset;
            self.tail = offset + self.len;
            self.network_header += offset;
            self.transport_header += offset;
        }
    }

    /// 在头部添加数据
    pub fn push(&mut self, size: usize) -> Option<&mut [u8]> {
        if self.head < size {
            return None;
        }
        self.head -= size;
        self.len += size;
        Some(&mut self.data[self.head..self.head + size])
    }

    /// 在尾部添加数据
    pub fn put(&mut self, size: usize) -> Option<&mut [u8]> {
        if self.tail + size > self.data.len() {
            return None;
        }
        let start = self.tail;
        self.tail += size;
        self.len += size;
        Some(&mut self.data[start..self.tail])
    }

    /// 从头部移除数据
    pub fn pull(&mut self, size: usize) -> Option<&[u8]> {
        if self.len < size {
            return None;
        }
        let start = self.head;
        self.head += size;
        self.len -= size;
        Some(&self.data[start..self.head])
    }

    /// 获取数据切片
    pub fn data(&self) -> &[u8] {
        &self.data[self.head..self.tail]
    }

    /// 获取可变数据切片
    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data[self.head..self.tail]
    }

    /// 获取整个缓冲区
    pub fn buffer(&self) -> &[u8] {
        &self.data
    }

    /// 设置网络层头位置
    pub fn set_network_header(&mut self, offset: usize) {
        self.network_header = self.head + offset;
    }

    /// 获取网络层头
    pub fn network_header(&self) -> &[u8] {
        &self.data[self.network_header..]
    }

    /// 获取传输层头
    pub fn transport_header(&self) -> &[u8] {
        &self.data[self.transport_header..]
    }

    /// 克隆skb（简化版）
    pub fn clone(&self) -> Self {
        let mut new = Self::new(self.data.len());
        new.data.copy_from_slice(&self.data);
        new.head = self.head;
        new.tail = self.tail;
        new.len = self.len;
        new.network_header = self.network_header;
        new.transport_header = self.transport_header;
        new.dev = self.dev.clone();
        new
    }
}
