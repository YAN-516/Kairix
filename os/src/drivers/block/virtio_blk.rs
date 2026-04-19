use super::BlockDevice;
use crate::config::KERNEL_SPACE_OFFSET;
use crate::mm::{
    FrameTracker, PageTable, PhysAddr, PhysPageNum, StepByOne, VirtAddr, frame_alloc,
    frame_dealloc, KERNEL_VMSET, VMSpace,
};
use crate::config::BLOCK_SIZE;
use crate::sync::UPSafeCell;
use alloc::vec::Vec;
use lazy_static::*;

use alloc::{string::ToString, sync::Arc};
use core::ptr::NonNull;

use virtio_drivers::device::blk::VirtIOBlk;
use virtio_drivers::transport::mmio::{MmioTransport, VirtIOHeader};
use virtio_drivers::transport::{DeviceType, Transport};
use virtio_drivers::BufferDirection;

use log::*;
use crate::logging;

#[allow(unused)]
const VIRTIO0: usize = 0x10001000 + KERNEL_SPACE_OFFSET;

pub struct VirtIOBlock(UPSafeCell<VirtIOBlk<VirtioHal, MmioTransport>>);

lazy_static! {
    static ref QUEUE_FRAMES: UPSafeCell<Vec<FrameTracker>> = unsafe { UPSafeCell::new(Vec::new()) };
}
pub struct VirtioHal;

unsafe impl virtio_drivers::Hal for VirtioHal {
    fn dma_alloc(pages: usize, _direction: BufferDirection,) -> (virtio_drivers::PhysAddr, NonNull<u8>) {
        info!("dma_alloc");
        let mut ppn_base = PhysPageNum(0);
        for i in 0..pages {
            let frame = frame_alloc().unwrap();
            if i == 0 {
                ppn_base = frame.ppn;
            }
            assert_eq!(frame.ppn.0, ppn_base.0 + i);
            QUEUE_FRAMES.exclusive_access().push(frame);
        }
        let pa: PhysAddr = ppn_base.into();
        (pa.0, NonNull::new(pa.get_mut::<u8>()).unwrap())//第二个为内核使用的虚拟地址指针,因为内核页表还是恒等映射
    }

    //仅回收物理页所有权，保留内核段虚拟映射。通过避免频繁刷新 TLB (sfence.vma) 显著提升 I/O 性能；同时物理分配器已同步更新状态，不影响该页被再次分发使用。
    unsafe fn dma_dealloc(paddr: virtio_drivers::PhysAddr, _vaddr: NonNull<u8>, pages: usize) -> i32 {
        info!("dma_dealloc");
        let pa = PhysAddr::from(paddr);
        let mut ppn_base: PhysPageNum = pa.into();
        for _ in 0..pages {
            frame_dealloc(ppn_base);
            ppn_base.step();
        }
        0
    }

    unsafe fn mmio_phys_to_virt(paddr: virtio_drivers::PhysAddr, _size: usize) -> NonNull<u8> {
        NonNull::new(PhysAddr::from(paddr+KERNEL_SPACE_OFFSET).get_mut::<u8>()).unwrap()
    }

    unsafe fn share(
        buffer: NonNull<[u8]>,
        _direction: BufferDirection,
    ) -> virtio_drivers::PhysAddr {
        // use kernel space pagetable to get the physical address
        let page_table = PageTable::from_token(KERNEL_VMSET.exclusive_access().token());
        let pa = page_table.translate_va(VirtAddr::from(buffer.as_ptr() as *const u8 as usize)).unwrap();
        
        pa.0

    }

    unsafe fn unshare(
        _paddr: virtio_drivers::PhysAddr,
        _buffer: NonNull<[u8]>,
        _direction: BufferDirection,
    ) {}
    // fn phys_to_virt(addr: usize) -> usize {
    //     addr + KERNEL_SPACE_OFFSET
    // }
}
#[allow(unused)]
    fn virt_to_phys(vaddr: usize) -> usize {
        PageTable::from_token(KERNEL_VMSET.exclusive_access().token())
            .translate_va(VirtAddr::from(vaddr))
            .unwrap()
            .0
    }

impl VirtIOBlock {
    #[allow(unused)]
    pub fn new() -> Self {
        unsafe {
            let header = core::ptr::NonNull::new(VIRTIO0 as *mut VirtIOHeader).unwrap();
            let transport = MmioTransport::new(header).unwrap();
            Self(UPSafeCell::new(
                VirtIOBlk::<VirtioHal, MmioTransport>::new(transport).expect("failed to create blk driver"),
            ))
        }
    }
}

impl BlockDevice for VirtIOBlock {
    //总字节数
    fn size(&self) -> u64 {
        self.0
            .exclusive_access()
            .capacity() * (BLOCK_SIZE as u64)
    }

    fn block_size(&self) -> usize {
        BLOCK_SIZE
    }
    
    fn read_block(&self, block_id: usize, buf: &mut [u8]) {
        let mut driver = self.0.exclusive_access();
        if buf.len() % BLOCK_SIZE != 0 {
            error!(
                "virtio read invalid size: block_id={}, len={}, block_size={}",
                block_id,
                buf.len(),
                BLOCK_SIZE
            );
            buf.fill(0);
            return;
        }
        let req_blocks = buf.len() / BLOCK_SIZE;
        let dev_blocks = driver.capacity() as usize;
        let end_block = match block_id.checked_add(req_blocks) {
            Some(v) => v,
            None => {
                error!(
                    "virtio read overflow: block_id={}, req_blocks={}",
                    block_id,
                    req_blocks
                );
                buf.fill(0);
                return;
            }
        };
        if end_block > dev_blocks {
            error!(
                "virtio read out of range: block_id={}, req_blocks={}, capacity_blocks={}",
                block_id,
                req_blocks,
                dev_blocks
            );
            buf.fill(0);
            return;
        }

        for _ in 0..3 {
            if driver.read_blocks(block_id, buf).is_ok() {
                return;
            }
        }
        error!(
            "virtio read failed after retries: block_id={}, req_blocks={}",
            block_id,
            req_blocks
        );
        // 读失败时返回零填充，避免上层在坏盘块场景直接内核崩溃。
        buf.fill(0);
    }
    fn write_block(&self, block_id: usize, buf: &[u8]) {
        let mut driver = self.0.exclusive_access();
        if buf.len() % BLOCK_SIZE != 0 {
            error!(
                "virtio write invalid size: block_id={}, len={}, block_size={}",
                block_id,
                buf.len(),
                BLOCK_SIZE
            );
            return;
        }
        let req_blocks = buf.len() / BLOCK_SIZE;
        let dev_blocks = driver.capacity() as usize;
        let end_block = match block_id.checked_add(req_blocks) {
            Some(v) => v,
            None => {
                error!(
                    "virtio write overflow: block_id={}, req_blocks={}",
                    block_id,
                    req_blocks
                );
                return;
            }
        };
        if end_block > dev_blocks {
            error!(
                "virtio write out of range: block_id={}, req_blocks={}, capacity_blocks={}",
                block_id,
                req_blocks,
                dev_blocks
            );
            return;
        }

        for _ in 0..3 {
            if driver.write_blocks(block_id, buf).is_ok() {
                return;
            }
        }
        error!(
            "virtio write failed after retries: block_id={}, req_blocks={}",
            block_id,
            req_blocks
        );
    }
}



