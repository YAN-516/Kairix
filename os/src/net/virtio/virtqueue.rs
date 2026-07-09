// net/virtio/virtqueue.rs
use super::config::{QUEUE_SIZE, VirtqAvail, VirtqDesc, VirtqUsed};
use alloc::vec;
use alloc::vec::Vec;
use core::ptr;
use polyhal::consts::PAGE_SIZE;

#[inline]
fn align_up(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}

/// Virtqueue
#[allow(unused)]
pub struct VirtQueue {
    pub queue_size: u16,
    pub desc: *mut VirtqDesc,
    pub avail: *mut VirtqAvail,
    pub used: *mut VirtqUsed,
    pub free_desc: Vec<u16>,
    pub last_used_idx: u16,
    pub desc_pa: u64,
    pub avail_pa: u64,
    pub used_pa: u64,
}

impl VirtQueue {
    pub fn empty() -> Self {
        Self {
            queue_size: 0,
            desc: ptr::null_mut(),
            avail: ptr::null_mut(),
            used: ptr::null_mut(),
            free_desc: Vec::new(),
            last_used_idx: 0,
            desc_pa: 0,
            avail_pa: 0,
            used_pa: 0,
        }
    }
    #[allow(unused)]
    pub fn new(
        size: u16,
        desc: *mut VirtqDesc,
        avail: *mut VirtqAvail,
        used: *mut VirtqUsed,
        desc_pa: u64,
        avail_pa: u64,
        used_pa: u64,
    ) -> Self {
        let mut free_desc = Vec::with_capacity(size as usize);
        for i in 0..size {
            free_desc.push(i);
        }
        Self {
            queue_size: size,
            desc,
            avail,
            used,
            free_desc,
            last_used_idx: 0,
            desc_pa,
            avail_pa,
            used_pa,
        }
    }

    pub fn alloc_desc(&mut self) -> Result<u16, &'static str> {
        self.free_desc.pop().ok_or("No free descriptor")
    }

    pub fn free_desc(&mut self, idx: u16) {
        self.free_desc.push(idx);
    }
    #[allow(unused)]
    /// 获取描述符的物理地址
    pub fn desc_phys_addr(&self, idx: u16) -> u64 {
        self.desc_pa + (idx as u64) * core::mem::size_of::<VirtqDesc>() as u64
    }
    #[allow(unused)]
    /// 获取 avail ring 的物理地址
    pub fn avail_phys_addr(&self) -> u64 {
        self.avail_pa
    }
    #[allow(unused)]
    /// 获取 used ring 的物理地址
    pub fn used_phys_addr(&self) -> u64 {
        self.used_pa
    }
}
#[allow(unused)]
/// 分配 VirtQueue 内存
pub fn alloc_virtqueue_memory(size: u16) -> Result<VirtQueueMemory, &'static str> {
    let desc_size = (size as usize) * core::mem::size_of::<VirtqDesc>();
    // legacy virtio-mmio requires the descriptor table and used ring to follow
    // QueuePFN/QueueAlign layout. The same page-aligned layout is also valid
    // for modern transports.
    let avail_size = 6 + core::mem::size_of::<u16>() * (size as usize);
    let used_offset = align_up(desc_size + avail_size, PAGE_SIZE);
    let used_size = 6 + core::mem::size_of::<super::config::VirtqUsedElem>() * (size as usize);
    let total = used_offset + used_size;

    let mut memory = vec![0u8; total + PAGE_SIZE];
    let base = memory.as_mut_ptr() as usize;
    let desc_addr = align_up(base, PAGE_SIZE);
    let avail_addr = desc_addr + desc_size;
    let used_addr = align_up(avail_addr + avail_size, PAGE_SIZE);

    let desc_ptr = desc_addr as *mut VirtqDesc;
    let avail_ptr = avail_addr as *mut VirtqAvail;
    let used_ptr = used_addr as *mut VirtqUsed;

    // 初始化 avail ring
    unsafe {
        (*avail_ptr).flags = 0;
        (*avail_ptr).idx = 0;
    }

    // 初始化 used ring
    unsafe {
        (*used_ptr).flags = 0;
        (*used_ptr).idx = 0;
    }

    let desc_pa = desc_ptr as u64;
    let avail_pa = avail_ptr as u64;
    let used_pa = used_ptr as u64;

    Ok(VirtQueueMemory {
        _memory: memory,
        desc_ptr,
        avail_ptr,
        used_ptr,
        desc_pa,
        avail_pa,
        used_pa,
        size,
    })
}

/// VirtQueue 内存（持有内存所有权）
#[allow(unused)]
pub struct VirtQueueMemory {
    _memory: Vec<u8>,
    pub desc_ptr: *mut VirtqDesc,
    pub avail_ptr: *mut VirtqAvail,
    pub used_ptr: *mut VirtqUsed,
    pub desc_pa: u64,
    pub avail_pa: u64,
    pub used_pa: u64,
    pub size: u16,
}
#[allow(unused)]
impl VirtQueueMemory {
    pub fn as_virtqueue(&self) -> VirtQueue {
        VirtQueue::new(
            self.size,
            self.desc_ptr,
            self.avail_ptr,
            self.used_ptr,
            self.desc_pa,
            self.avail_pa,
            self.used_pa,
        )
    }

    pub fn into_virtqueue(self) -> VirtQueue {
        VirtQueue::new(
            self.size,
            self.desc_ptr,
            self.avail_ptr,
            self.used_ptr,
            self.desc_pa,
            self.avail_pa,
            self.used_pa,
        )
    }
}
