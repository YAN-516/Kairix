//! VirtIO virtqueue 内存分配和队列状态。
//!
//! Virtqueue 包含 descriptor table、available ring 和 used ring。该文件
//! 负责按规范布局分配连续内存，并维护驱动侧的空闲 descriptor 栈。

use super::config::{QUEUE_SIZE, VirtqAvail, VirtqDesc, VirtqUsed};
use alloc::vec;
use alloc::vec::Vec;
use core::ptr;
use polyhal::consts::{PAGE_SIZE, VIRT_ADDR_START};

#[cfg(target_arch = "loongarch64")]
const LOONGARCH_UNCACHED_DMW_BASE: usize = 0x8000_0000_0000_0000;

/// 向上对齐到指定边界。
#[inline]
fn align_up(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}

/// 将内核虚拟地址转换为设备可见的物理地址。
#[inline]
fn virt_to_phys_addr(addr: usize) -> usize {
    #[cfg(target_arch = "loongarch64")]
    {
        if addr >= VIRT_ADDR_START {
            return addr - VIRT_ADDR_START;
        }
        if addr >= LOONGARCH_UNCACHED_DMW_BASE {
            return addr - LOONGARCH_UNCACHED_DMW_BASE;
        }
    }

    #[cfg(not(target_arch = "loongarch64"))]
    {
        if addr >= VIRT_ADDR_START {
            return addr - VIRT_ADDR_START;
        }
    }

    addr
}

/// 将一个 DMA buffer 地址转换为 CPU 应访问的地址。
///
/// LoongArch 使用 uncached DMW 访问 DMA 内存，避免缓存一致性问题。
#[inline]
fn dma_cpu_addr(addr: usize) -> usize {
    #[cfg(target_arch = "loongarch64")]
    {
        LOONGARCH_UNCACHED_DMW_BASE + virt_to_phys_addr(addr)
    }

    #[cfg(not(target_arch = "loongarch64"))]
    {
        addr
    }
}

/// Virtqueue 运行态状态。
#[allow(unused)]
pub struct VirtQueue {
    /// 队列大小。
    pub queue_size: u16,
    /// descriptor table 指针。
    pub desc: *mut VirtqDesc,
    /// available ring 指针。
    pub avail: *mut VirtqAvail,
    /// used ring 指针。
    pub used: *mut VirtqUsed,
    /// 当前空闲 descriptor 索引。
    pub free_desc: Vec<u16>,
    /// 驱动已消费到的 used ring idx。
    pub last_used_idx: u16,
    /// descriptor table 物理地址。
    pub desc_pa: u64,
    /// available ring 物理地址。
    pub avail_pa: u64,
    /// used ring 物理地址。
    pub used_pa: u64,
}

impl VirtQueue {
    /// 构造一个未初始化的空队列占位。
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
    /// 从已经分配好的 queue 内存构造运行态队列。
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

    /// 分配一个空闲 descriptor。
    pub fn alloc_desc(&mut self) -> Result<u16, &'static str> {
        self.free_desc.pop().ok_or("No free descriptor")
    }

    /// 释放一个 descriptor 回空闲栈。
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
/// 分配 VirtQueue 内存。
///
/// legacy virtio-mmio 要求 descriptor table 和 used ring 满足
/// QueuePFN/QueueAlign 布局；这里使用同样布局以兼容 legacy 与 modern。
pub fn alloc_virtqueue_memory(size: u16) -> Result<VirtQueueMemory, &'static str> {
    let desc_size = (size as usize) * core::mem::size_of::<VirtqDesc>();
    // legacy virtio-mmio requires the descriptor table and used ring to follow
    // QueuePFN/QueueAlign layout. The same page-aligned layout is also valid
    // for modern transports.
    let avail_size = 6 + core::mem::size_of::<u16>() * (size as usize);
    let used_offset = align_up(desc_size + avail_size, PAGE_SIZE);
    let used_size = 6 + core::mem::size_of::<super::config::VirtqUsedElem>() * (size as usize);
    let total = used_offset + used_size;

    #[cfg(target_arch = "loongarch64")]
    let mut memory = {
        let mut memory = Vec::with_capacity(total + PAGE_SIZE);
        unsafe {
            memory.set_len(total + PAGE_SIZE);
            ptr::write_bytes(
                dma_cpu_addr(memory.as_mut_ptr() as usize) as *mut u8,
                0,
                memory.len(),
            );
        }
        memory
    };

    #[cfg(not(target_arch = "loongarch64"))]
    let mut memory = vec![0u8; total + PAGE_SIZE];

    let base = memory.as_mut_ptr() as usize;
    let desc_addr = align_up(base, PAGE_SIZE);
    let avail_addr = desc_addr + desc_size;
    let used_addr = align_up(avail_addr + avail_size, PAGE_SIZE);

    let desc_ptr = dma_cpu_addr(desc_addr) as *mut VirtqDesc;
    let avail_ptr = dma_cpu_addr(avail_addr) as *mut VirtqAvail;
    let used_ptr = dma_cpu_addr(used_addr) as *mut VirtqUsed;

    // 初始化 avail ring
    unsafe {
        ptr::write_volatile(&mut (*avail_ptr).flags, 0);
        ptr::write_volatile(&mut (*avail_ptr).idx, 0);
    }

    // 初始化 used ring
    unsafe {
        ptr::write_volatile(&mut (*used_ptr).flags, 0);
        ptr::write_volatile(&mut (*used_ptr).idx, 0);
    }

    let desc_pa = virt_to_phys_addr(desc_addr) as u64;
    let avail_pa = virt_to_phys_addr(avail_addr) as u64;
    let used_pa = virt_to_phys_addr(used_addr) as u64;

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

/// VirtQueue 内存（持有内存所有权）。
#[allow(unused)]
pub struct VirtQueueMemory {
    /// 原始 backing buffer，保证 queue 内存生命周期。
    _memory: Vec<u8>,
    /// descriptor table 指针。
    pub desc_ptr: *mut VirtqDesc,
    /// available ring 指针。
    pub avail_ptr: *mut VirtqAvail,
    /// used ring 指针。
    pub used_ptr: *mut VirtqUsed,
    /// descriptor table 物理地址。
    pub desc_pa: u64,
    /// available ring 物理地址。
    pub avail_pa: u64,
    /// used ring 物理地址。
    pub used_pa: u64,
    /// 队列大小。
    pub size: u16,
}
#[allow(unused)]
impl VirtQueueMemory {
    /// 借用当前内存布局生成一个 `VirtQueue`。
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

    /// 消费内存对象并生成 `VirtQueue`。
    ///
    /// 当前代码主要使用 `as_virtqueue`，因为设备结构体需要继续持有内存所有权。
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
