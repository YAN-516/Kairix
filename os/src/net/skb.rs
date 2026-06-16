use crate::net::device::NetDevice;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
#[allow(unused)]
/// 网络数据包缓冲区
pub struct Skb {
    /// 数据缓冲区（连续内存）
    pub data: Vec<u8>,
    /// 数据区域的起始偏移（包含所有协议头）
    pub data_start: usize,
    /// 数据区域的结束偏移
    pub data_end: usize,
    /// 关联的网络设备
    pub dev: Option<Arc<dyn NetDevice>>,
}
#[allow(unused)]
impl Skb {
    /// 创建新的空 skb，预留指定大小的缓冲区
    pub fn new(capacity: usize) -> Self {
        let mut data = Vec::with_capacity(capacity);
        data.resize(capacity, 0);

        Self {
            data,
            data_start: 0,
            data_end: 0,
            dev: None,
        }
    }

    /// 创建带有头部预留空间的 skb。
    pub fn with_headroom(headroom: usize, capacity: usize) -> Self {
        let mut data = Vec::with_capacity(headroom + capacity);
        data.resize(headroom + capacity, 0);

        Self {
            data,
            data_start: headroom,
            data_end: headroom,
            dev: None,
        }
    }

    /// 获取当前有效数据的长度
    pub fn len(&self) -> usize {
        self.data_end - self.data_start
    }

    /// 判断是否为空
    pub fn is_empty(&self) -> bool {
        self.data_start == self.data_end
    }

    /// 获取头部可用的空间大小
    pub fn headroom(&self) -> usize {
        self.data_start
    }

    /// 获取尾部可用的空间大小
    pub fn tailroom(&self) -> usize {
        self.data.len() - self.data_end
    }

    /// 预留头部空间
    /// 确保在数据前面至少有 `size` 字节的空闲空间
    pub fn reserve_head(&mut self, size: usize) {
        if size <= self.headroom() {
            return; // 空间足够
        }

        // 需要额外分配的空间
        let need = size - self.headroom();
        let new_capacity = self.data.len() + need;
        let mut new_data = vec![0u8; new_capacity];

        // 将现有数据移动到新缓冲区的后面（留出足够头部空间）
        let new_start = size;
        let new_end = new_start + self.len();

        new_data[new_start..new_end].copy_from_slice(&self.data[self.data_start..self.data_end]);

        self.data = new_data;
        self.data_start = new_start;
        self.data_end = new_end;
    }

    /// 预留尾部空间
    pub fn reserve_tail(&mut self, size: usize) {
        if size <= self.tailroom() {
            return;
        }

        let need = size - self.tailroom();
        self.data.resize(self.data.len() + need, 0);
        // data_end 不变，只是缓冲区变大了
    }

    /// 在头部添加数据（添加协议头）
    pub fn push(&mut self, size: usize) -> Option<&mut [u8]> {
        if size > self.headroom() {
            // 尝试预留空间
            self.reserve_head(size);
            if size > self.headroom() {
                return None;
            }
        }

        self.data_start -= size;
        let start = self.data_start;
        let end = start + size;
        Some(&mut self.data[start..end])
    }

    /// 在尾部添加数据（添加负载）
    pub fn put(&mut self, size: usize) -> Option<&mut [u8]> {
        if size > self.tailroom() {
            self.reserve_tail(size);
            if size > self.tailroom() {
                return None;
            }
        }

        let start = self.data_end;
        self.data_end += size;
        Some(&mut self.data[start..self.data_end])
    }

    /// 从头部移除数据（剥离协议头）
    pub fn pull(&mut self, size: usize) -> Option<&[u8]> {
        if size > self.len() {
            return None;
        }

        let start = self.data_start;
        self.data_start += size;

        Some(&self.data[start..self.data_start])
    }

    /// 从尾部移除数据
    pub fn trim(&mut self, size: usize) -> Option<&[u8]> {
        if size > self.len() {
            return None;
        }

        let end = self.data_end;
        self.data_end -= size;
        Some(&self.data[self.data_end..end])
    }

    /// 获取当前数据切片
    pub fn data(&self) -> &[u8] {
        &self.data[self.data_start..self.data_end]
    }

    /// 获取可变数据切片
    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data[self.data_start..self.data_end]
    }

    /// 获取完整缓冲区（用于调试）
    pub fn buffer(&self) -> &[u8] {
        &self.data
    }

    /// 克隆 skb（深拷贝）
    pub fn clone(&self) -> Self {
        let mut new = Self::new(self.data.len());
        new.data.copy_from_slice(&self.data);
        new.data_start = self.data_start;
        new.data_end = self.data_end;
        new.dev = self.dev.clone();
        new
    }
}
