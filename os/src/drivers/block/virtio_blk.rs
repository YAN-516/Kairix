use super::BlockDevice;
// use crate::config::KERNEL_SPACE_OFFSET;
use crate::mm::{ frame_alloc,
    frame_dealloc, KERNEL_VMSET, VMSpace,
};
use crate::config::BLOCK_SIZE;
use crate::sync::UPSafeCell;
use alloc::vec::Vec;
use lazy_static::*;
use flat_device_tree::{node::FdtNode, standard_nodes::Compatible, Fdt};

use alloc::{string::ToString, sync::Arc};
use polyhal::consts::VIRT_ADDR_START;
use virtio_drivers::transport::pci::bus::Cam;
use core::ptr::NonNull;

use virtio_drivers::device::blk::VirtIOBlk;
use virtio_drivers::transport::mmio::{MmioTransport, VirtIOHeader,};
use virtio_drivers::transport::{DeviceType, Transport};
use virtio_drivers::transport::pci::*;
use virtio_drivers::BufferDirection;
use log::*;
use crate::logging;
use polyhal::pagetable::*;
use polyhal::common::FrameTracker;
use polyhal::utils::addr::*;
use polyhal::println;

#[allow(unused)]
const VIRTIO0: usize = 0x10001000 + VIRT_ADDR_START;

#[cfg(target_arch = "riscv64")]
pub struct VirtIOBlock(UPSafeCell<VirtIOBlk<VirtioHal, MmioTransport>>);

#[cfg(target_arch = "loongarch64")]
pub struct VirtIOBlock(UPSafeCell<VirtIOBlk<VirtioHal, PciTransport>>);

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
        NonNull::new(PhysAddr::from(paddr+VIRT_ADDR_START).get_mut::<u8>()).unwrap()
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
    #[cfg(target_arch = "riscv64")]
    #[allow(unused)]
    pub fn new() -> Self {
        unsafe {
            let header = core::ptr::NonNull::new(VIRTIO0 as *mut VirtIOHeader).unwrap();
            error!("VirtIOBlock: base={:#x}", VIRTIO0);
            let transport = match MmioTransport::new(header) {
                Ok(t) => {
                    println!("MmioTransport created");
                    t
                }
                Err(e) => {
                    panic!("MmioTransport creation failed: {:?}", e);
                }
            };
            // let transport = MmioTransport::new(header).unwrap();
            Self(UPSafeCell::new(
                VirtIOBlk::<VirtioHal, MmioTransport>::new(transport).expect("failed to create blk driver"),
            ))
        }
    }

    #[cfg(target_arch = "loongarch64")]
    #[allow(unused)]
    pub fn new_pci(transport: PciTransport) -> Self {
        
        unsafe{
            Self(UPSafeCell::new(
                VirtIOBlk::<VirtioHal, PciTransport>::new(transport)
                    .expect("failed to create blk driver"),
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
    // info!("Reading block {} with buf len {}", block_id, buf.len());
    
    // 检查缓冲区地址
    // let buf_addr = buf.as_ptr() as usize;
    // info!("Buffer virtual address: {:#x}", buf_addr);
    // info!("Buffer physical address: {:#x}", buf_addr - VIRT_ADDR_START);
    
    // 执行读取
    match self.0.exclusive_access().read_blocks(block_id, buf) {
        Ok(_) => info!("Read block {} success", block_id),
        Err(e) => error!("Read block {} failed: {:?}", block_id, e),
    }
    }
    fn write_block(&self, block_id: usize, buf: &[u8]) {
        self.0
            .exclusive_access()
            .write_blocks(block_id, buf)
            .expect("Error when writing VirtIOBlk");
    }
}

#[cfg(target_arch = "loongarch64")]
pub fn init_virtio_pci() {
    
    
    // 获取设备树地址（从 bootloader 传入，通常在 a1 寄存器）
    let fdt_addr = get_fdt_addr();
    let fdt = unsafe { Fdt::from_ptr(fdt_addr as *const u8).unwrap() };
    
    // 查找 PCI 节点
    if let Some(pci_node) = fdt.find_node("/pci@10000000") {
        // 使用 ECAM（增强配置访问机制）
        let cam = Cam::Ecam;
        super::pci::enumerate_pci(pci_node, cam);
    } else {
        error!("PCI node not found!");
    }
}

#[cfg(target_arch = "loongarch64")]
fn get_fdt_addr() -> usize {
    let fdt_addr: usize;
    unsafe {
        core::arch::asm!("move {}, $a1", out(reg) fdt_addr);
    }
    fdt_addr
}



