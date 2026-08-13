//! 网络栈内部使用的 socket buffer。
//!
//! `Skb` 保存一段连续缓冲区，并通过 `data_start..data_end` 标记当前有效
//! 数据。协议栈收包时逐层 `pull` 剥离头部，发包时逐层 `push` 添加头部。

use crate::net::device::NetDevice;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
#[allow(unused)]
/// 网络数据包缓冲区。
///
/// 这个结构借鉴 Linux skb 的“头部空间 + 有效数据 + 尾部空间”模型，
/// 让协议头追加和剥离都可以只移动偏移，而不必频繁搬移整包数据。
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
    /// 创建新的空 skb，预留指定大小的缓冲区。
    ///
    /// 初始时有效数据长度为 0，`capacity` 只作为尾部可写空间。
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
    ///
    /// 发包路径通常先放 payload，再由 TCP/IP/以太网逐层向前 `push` 头部。
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

    /// 预留头部空间。
    ///
    /// 确保当前有效数据前至少有 `size` 字节空闲；空间不足时会重新分配并
    /// 把有效数据整体后移。
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

    /// 在头部添加数据（添加协议头）。
    ///
    /// 返回新增出来的可写头部切片，调用者可直接填充协议头结构。
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

    /// 在尾部添加数据（添加负载）。
    ///
    /// 返回新增出来的可写尾部切片。
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

    /// 从头部移除数据（剥离协议头）。
    ///
    /// 返回被移除的头部切片，常用于接收路径读取当前协议头。
    pub fn pull(&mut self, size: usize) -> Option<&[u8]> {
        if size > self.len() {
            return None;
        }

        let start = self.data_start;
        self.data_start += size;

        Some(&self.data[start..self.data_start])
    }

    /// 从尾部移除数据。
    ///
    /// 接收路径会用它丢弃网卡填充、FCS 或上层长度之外的尾部数据。
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
