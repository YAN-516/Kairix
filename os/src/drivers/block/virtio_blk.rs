use super::BlockDevice;
// use crate::config::KERNEL_SPACE_OFFSET;
use crate::config::BLOCK_SIZE;
use crate::mm::frame_alloc_contiguous;
use crate::net::virtio::config::VIRTIO_F_VERSION_1;
use crate::sync::{SleepLock, SpinLock};
use alloc::vec::Vec;
use flat_device_tree::{Fdt, node::FdtNode, standard_nodes::Compatible};
use lazy_static::*;

use alloc::{string::ToString, sync::Arc};
use core::error;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use polyhal::consts::{PAGE_SIZE, VIRT_ADDR_START};
use virtio_drivers::Hal;
use virtio_drivers::device::blk::{BlkReq, BlkResp, VirtIOBlk};
use virtio_drivers::transport;
use virtio_drivers::transport::mmio::{MmioTransport, VirtIOHeader};
use virtio_drivers::transport::pci::bus::Cam;
use virtio_drivers::transport::pci::*;
use virtio_drivers::transport::{DeviceType, Transport};

use crate::error::{SysError, SysResult};
use crate::logging;
use log::*;
use polyhal::common::FrameTracker;
use polyhal::pagetable::*;
use polyhal::utils::addr::*;
use virtio_drivers::BufferDirection;

#[cfg(target_arch = "loongarch64")]
use polyhal::consts::FDT_ADDR;

#[cfg(target_arch = "riscv64")]
#[allow(unused)]
const FDT_ADDR: u64 = 0x9000_0000_0010_0000;

#[allow(unused)]
const VIRTIO0: usize = 0x10001000 + VIRT_ADDR_START;
const BLK_BOUNCE_SIZE: usize = PAGE_SIZE;
const BLK_BOUNCE_SECTORS: usize = BLK_BOUNCE_SIZE / BLOCK_SIZE;

static BLK_IO_ACTIVE: AtomicBool = AtomicBool::new(false);
static BLK_IO_OP: AtomicUsize = AtomicUsize::new(0);
static BLK_IO_PHASE: AtomicUsize = AtomicUsize::new(0);
static BLK_IO_BLOCK_ID: AtomicUsize = AtomicUsize::new(0);
static BLK_IO_SECTORS: AtomicUsize = AtomicUsize::new(0);
static BLK_IO_CHUNK_SECTOR: AtomicUsize = AtomicUsize::new(0);
static BLK_IO_TOKEN: AtomicUsize = AtomicUsize::new(usize::MAX);
static BLK_IO_POLLS: AtomicUsize = AtomicUsize::new(0);
static BLK_IO_REQUEST_SEQUENCE: AtomicUsize = AtomicUsize::new(0);
static BLK_IO_COMPLETION_SEQUENCE: AtomicUsize = AtomicUsize::new(0);
static BLK_IO_REQUESTED_SECTORS: AtomicUsize = AtomicUsize::new(0);
static BLK_IO_COMPLETED_SECTORS: AtomicUsize = AtomicUsize::new(0);
static BLK_IO_LAST_COMPLETION_NS: AtomicUsize = AtomicUsize::new(0);

/// Lock-free diagnostic snapshot of the synchronous VirtIO block request.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct VirtioBlockIoStats {
    pub active: bool,
    /// 0=idle, 1=read, 2=write, 3=flush.
    pub op: usize,
    /// 0=idle, 1=device locked, 2=waiting for bounce buffer,
    /// 3=bounce locked, 4=submitting, 5=polling used ring,
    /// 6=completing, 7=complete, 41=translating a DMA buffer.
    pub phase: usize,
    pub block_id: usize,
    pub sectors: usize,
    pub chunk_sector: usize,
    pub token: Option<usize>,
    pub polls: usize,
}

/// Return block-I/O progress without acquiring driver locks.
pub fn virtio_block_io_stats() -> VirtioBlockIoStats {
    let token = BLK_IO_TOKEN.load(Ordering::Acquire);
    VirtioBlockIoStats {
        active: BLK_IO_ACTIVE.load(Ordering::Acquire),
        op: BLK_IO_OP.load(Ordering::Acquire),
        phase: BLK_IO_PHASE.load(Ordering::Acquire),
        block_id: BLK_IO_BLOCK_ID.load(Ordering::Acquire),
        sectors: BLK_IO_SECTORS.load(Ordering::Acquire),
        chunk_sector: BLK_IO_CHUNK_SECTOR.load(Ordering::Acquire),
        token: (token != usize::MAX).then_some(token),
        polls: BLK_IO_POLLS.load(Ordering::Acquire),
    }
}

/// Cumulative request boundaries that remain meaningful after the current
/// synchronous request has returned and `VirtioBlockIoStats::active` is false.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct VirtioBlockCompletionStats {
    pub requests: usize,
    pub completions: usize,
    pub requested_sectors: usize,
    pub completed_sectors: usize,
    pub last_completion_ns: usize,
}

/// Return cumulative block request completion evidence without device locks.
pub fn virtio_block_completion_stats() -> VirtioBlockCompletionStats {
    VirtioBlockCompletionStats {
        requests: BLK_IO_REQUEST_SEQUENCE.load(Ordering::Acquire),
        completions: BLK_IO_COMPLETION_SEQUENCE.load(Ordering::Acquire),
        requested_sectors: BLK_IO_REQUESTED_SECTORS.load(Ordering::Relaxed),
        completed_sectors: BLK_IO_COMPLETED_SECTORS.load(Ordering::Relaxed),
        last_completion_ns: BLK_IO_LAST_COMPLETION_NS.load(Ordering::Relaxed),
    }
}

fn translate_dma_vaddr(vaddr: usize) -> virtio_drivers::PhysAddr {
    BLK_IO_PHASE.store(41, Ordering::Release);
    let pa = PageTable::current()
        .translate_va(VirtAddr::from(vaddr))
        .unwrap_or_else(|| panic!("virtio share unmapped buffer vaddr {:#x}", vaddr))
        .0;
    BLK_IO_PHASE.store(4, Ordering::Release);
    pa
}

#[cfg(target_arch = "loongarch64")]
fn loongarch_cached_dmw_phys(address: usize) -> Option<usize> {
    const DMW_VSEG_MASK: usize = 0xf000_0000_0000_0000;

    ((address & DMW_VSEG_MASK) == VIRT_ADDR_START).then(|| address - VIRT_ADDR_START)
}

pub(crate) fn validate_block_copy_buffer(op: &str, ptr: usize, len: usize) {
    if len == 0 {
        return;
    }
    let Some(end) = ptr.checked_add(len - 1) else {
        log::error!(
            "[VIRTIO_BLK_BUFFER_RANGE_OVERFLOW] op={} cpu={} ptr={:#x} len={} fwrite_detail={:?} write_source={:?} ext4_flush={:?}",
            op,
            polyhal::arch::hart_id(),
            ptr,
            len,
            crate::fs::lwext4::lwext4_fwrite_detail_progress(),
            crate::fs::lwext4::lwext4_write_source_progress(),
            crate::fs::lwext4::file::ext4_flush_stats(),
        );
        panic!("VirtIOBlk {} buffer range overflow", op);
    };
    let current = PageTable::current();
    let kernel_token = crate::mm::vm_set::kernel_page_table_token();
    let kernel = (kernel_token != 0).then(|| PageTable::from_token(kernel_token));
    for address in [ptr, end] {
        let va = VirtAddr::from(address);
        let current_pa = current.translate_va(va);
        let kernel_pa = kernel.as_ref().and_then(|table| table.translate_va(va));
        let direct_map_candidate = address
            .checked_sub(VIRT_ADDR_START)
            .filter(|_| address >= VIRT_ADDR_START);
        let in_platform_memory = direct_map_candidate.is_some_and(|pa| {
            polyhal::mem::get_mem_areas().any(|&(start, size)| {
                start
                    .checked_add(size)
                    .is_some_and(|region_end| pa >= start && pa < region_end)
            })
        });
        // Kernel stacks and statically linked sections also live above
        // VIRT_ADDR_START, but they are not physical-direct-map aliases. Only
        // impose the VA-offset equality for addresses inside a platform RAM
        // range owned by the frame allocator.
        let expected_direct_pa = direct_map_candidate.filter(|_| in_platform_memory);
        #[cfg(target_arch = "loongarch64")]
        let mapping_valid = if let Some(dmw_pa) = loongarch_cached_dmw_phys(address) {
            // PLV0 accesses to the cached 0x9... DMW bypass PGDL/PGDH. A user
            // root therefore need not contain a PTE for this address even
            // while the kernel is using it. Keep the permanent kernel mapping
            // as an independent check that the address has the expected PA.
            kernel_pa.is_some_and(|pa| pa.0 == dmw_pa)
        } else {
            current_pa.is_some()
                && kernel_pa.is_some()
                && expected_direct_pa.is_none_or(|expected| {
                    current_pa.is_some_and(|pa| pa.0 == expected)
                        && kernel_pa.is_some_and(|pa| pa.0 == expected)
                })
        };
        #[cfg(not(target_arch = "loongarch64"))]
        let mapping_valid = current_pa.is_some()
            && kernel_pa.is_some()
            && expected_direct_pa.is_none_or(|expected| {
                current_pa.is_some_and(|pa| pa.0 == expected)
                    && kernel_pa.is_some_and(|pa| pa.0 == expected)
            });
        if !mapping_valid {
            let pid = crate::task::current_task()
                .map(|task| task.process_id())
                .unwrap_or(0);
            let physical_info = direct_map_candidate.map(polyhal::mem::memory_address_info);
            let heap_info = crate::mm::heap_allocator::heap_pointer_info(ptr);
            let lwext4_allocation = lwext4_rust::allocation_pointer_info(ptr);
            let lwext4_source = crate::fs::lwext4::lwext4_buffer_progress();
            let lwext4_write_source = crate::fs::lwext4::lwext4_write_source_progress();
            let lwext4_fwrite_detail = crate::fs::lwext4::lwext4_fwrite_detail_progress();
            let lwext4_origin_heap = (lwext4_source.origin != 0)
                .then(|| crate::mm::heap_allocator::heap_pointer_info(lwext4_source.origin));
            let lwext4_origin_allocation = (lwext4_source.origin != 0)
                .then(|| lwext4_rust::allocation_pointer_info(lwext4_source.origin));
            log::error!(
                "[VIRTIO_BLK_LWEXT4_SOURCE] op={} ptr={:#x} matches_active_source={} source={:?} origin_heap={:?} origin_allocation={:?} allocation_stats={:?}",
                op,
                ptr,
                lwext4_source.phase == 1 && lwext4_source.data == ptr,
                lwext4_source,
                lwext4_origin_heap,
                lwext4_origin_allocation,
                lwext4_rust::allocation_stats(),
            );
            log::error!(
                "[VIRTIO_BLK_LWEXT4_WRITE_SOURCE] op={} ptr={:#x} source={:?}",
                op,
                ptr,
                lwext4_write_source,
            );
            log::error!(
                "[VIRTIO_BLK_LWEXT4_FWRITE_DETAIL] op={} ptr={:#x} detail={:?}",
                op,
                ptr,
                lwext4_fwrite_detail,
            );
            log::error!(
                "[VIRTIO_BLK_BUFFER_PROVENANCE] op={} cpu={} pid={} ptr={:#x} len={} checked_va={:#x} checked_offset={} physical_info={:?} heap_info={:?} lwext4_allocation={:?}",
                op,
                polyhal::arch::hart_id(),
                pid,
                ptr,
                len,
                address,
                address.saturating_sub(ptr),
                physical_info,
                heap_info,
                lwext4_allocation,
            );
            log::error!(
                "[VIRTIO_BLK_BUFFER_MAPPING_CORRUPTION] op={} cpu={} pid={} ptr={:#x} len={} checked_va={:#x} current_token={:#x} kernel_token={:#x} current_pa={:?} kernel_pa={:?} direct_map_candidate={:?} expected_direct_pa={:?} in_platform_memory={} ext4_flush={:?}",
                op,
                polyhal::arch::hart_id(),
                pid,
                ptr,
                len,
                address,
                current.token(),
                kernel_token,
                current_pa,
                kernel_pa,
                direct_map_candidate,
                expected_direct_pa,
                in_platform_memory,
                crate::fs::lwext4::file::ext4_flush_stats(),
            );
            panic!("VirtIOBlk {} buffer mapping invariant violated", op);
        }
    }
}

struct BlockIoProgress {
    sectors: usize,
}

impl BlockIoProgress {
    fn begin(op: usize, block_id: usize, sectors: usize) -> Self {
        BLK_IO_OP.store(op, Ordering::Release);
        BLK_IO_BLOCK_ID.store(block_id, Ordering::Release);
        BLK_IO_SECTORS.store(sectors, Ordering::Release);
        BLK_IO_CHUNK_SECTOR.store(block_id, Ordering::Release);
        BLK_IO_TOKEN.store(usize::MAX, Ordering::Release);
        BLK_IO_POLLS.store(0, Ordering::Release);
        BLK_IO_PHASE.store(1, Ordering::Release);
        BLK_IO_ACTIVE.store(true, Ordering::Release);
        BLK_IO_REQUESTED_SECTORS.fetch_add(sectors, Ordering::Relaxed);
        BLK_IO_REQUEST_SEQUENCE.fetch_add(1, Ordering::Release);
        Self { sectors }
    }
}

impl Drop for BlockIoProgress {
    fn drop(&mut self) {
        BLK_IO_PHASE.store(7, Ordering::Release);
        BLK_IO_ACTIVE.store(false, Ordering::Release);
        BLK_IO_COMPLETED_SECTORS.fetch_add(self.sectors, Ordering::Relaxed);
        BLK_IO_LAST_COMPLETION_NS.store(
            polyhal::timer::current_time().as_nanos() as usize,
            Ordering::Relaxed,
        );
        BLK_IO_COMPLETION_SEQUENCE.fetch_add(1, Ordering::Release);
    }
}

#[cfg(target_arch = "riscv64")]
pub struct VirtIOBlock(SleepLock<VirtIOBlk<VirtioHal, MmioTransport>>);

#[cfg(target_arch = "loongarch64")]
pub struct VirtIOBlock(SleepLock<VirtIOBlk<VirtioHal, PciTransport>>);

lazy_static! {
    static ref QUEUE_FRAMES: SpinLock<Vec<FrameTracker>> = SpinLock::new(Vec::new());
    static ref BLK_IO_BOUNCE: SleepLock<BlkIoBounce> = SleepLock::new(BlkIoBounce::new());
}

struct BlkIoBounce {
    req: DmaReq,
    resp: DmaResp,
    buf: DmaBuffer,
}

#[repr(C, align(4096))]
struct DmaReq(BlkReq);

#[repr(C, align(4096))]
struct DmaResp(BlkResp);

#[repr(C, align(4096))]
struct DmaBuffer {
    bytes: [u8; BLK_BOUNCE_SIZE],
}

impl BlkIoBounce {
    fn new() -> Self {
        Self {
            req: DmaReq(BlkReq::default()),
            resp: DmaResp(BlkResp::default()),
            buf: DmaBuffer {
                bytes: [0; BLK_BOUNCE_SIZE],
            },
        }
    }
}
pub struct VirtioHal;

unsafe impl virtio_drivers::Hal for VirtioHal {
    fn dma_alloc(
        pages: usize,
        _direction: BufferDirection,
    ) -> (virtio_drivers::PhysAddr, NonNull<u8>) {
        info!("dma_alloc");
        let frames = frame_alloc_contiguous(pages).unwrap();
        let ppn_base = frames
            .first()
            .map(|frame| frame.ppn)
            .unwrap_or(PhysPageNum(0));
        {
            let mut queue_frames = QUEUE_FRAMES.lock();
            queue_frames.extend(frames);
        }
        let pa: PhysAddr = ppn_base.into();
        // error!("dma alloc pa {:#x}", pa.0);
        (pa.0, NonNull::new(pa.get_mut::<u8>()).unwrap()) //第二个为内核使用的虚拟地址指针,因为内核页表还是恒等映射
    }

    // Release DMA pages through their FrameTracker owners to keep allocator ownership consistent.
    unsafe fn dma_dealloc(
        paddr: virtio_drivers::PhysAddr,
        _vaddr: NonNull<u8>,
        pages: usize,
    ) -> i32 {
        info!("dma_dealloc");
        let pa = PhysAddr::from(paddr);
        let ppn_base: PhysPageNum = pa.into();
        let mut released = Vec::with_capacity(pages);
        {
            let mut frames = QUEUE_FRAMES.lock();
            for i in 0..pages {
                let ppn = PhysPageNum(ppn_base.0 + i);
                let Some(pos) = frames.iter().position(|frame| frame.ppn == ppn) else {
                    panic!("dma_dealloc unknown ppn {:#x}", ppn.0);
                };
                released.push(frames.swap_remove(pos));
            }
        }

        // Drop after releasing QUEUE_FRAMES. FrameTracker::drop() re-enters the
        // frame allocator, while dma_alloc() takes these locks in the opposite order.
        drop(released);
        0
    }

    unsafe fn mmio_phys_to_virt(paddr: virtio_drivers::PhysAddr, _size: usize) -> NonNull<u8> {
        NonNull::new(PhysAddr::from(paddr + VIRT_ADDR_START).get_mut::<u8>()).unwrap()
    }
    #[cfg(target_arch = "loongarch64")]
    unsafe fn share(
        buffer: NonNull<[u8]>,
        _direction: BufferDirection,
    ) -> virtio_drivers::PhysAddr {
        let vaddr = buffer.as_ptr() as *mut u8 as usize;

        // vaddr - VIRT_ADDR_START
        if (vaddr >> 60) == (VIRT_ADDR_START >> 60) {
            vaddr - VIRT_ADDR_START
        } else {
            translate_dma_vaddr(vaddr)
        }
        // let page_table = PageTable::from_token(KERNEL_VMSET.lock().token());

        // let pa = page_table.translate_va(VirtAddr::from(buffer.as_ptr() as *const u8 as usize)).unwrap();
        // info!("buffer len {}", buffer.len());
        // info!("pa {:#x}, va {:#x}", pa.0, buffer.as_ptr() as *const u8 as usize);
        // pa.0
    }
    #[cfg(target_arch = "riscv64")]
    unsafe fn share(
        buffer: NonNull<[u8]>,
        _direction: BufferDirection,
    ) -> virtio_drivers::PhysAddr {
        translate_dma_vaddr(buffer.as_ptr() as *const u8 as usize)

        // let page_table = PageTable::from_token(KERNEL_VMSET.lock().token());

        // let pa = page_table.translate_va(VirtAddr::from(buffer.as_ptr() as *const u8 as usize)).unwrap();
        // info!("buffer len {}", buffer.len());
        // info!("pa {:#x}, va {:#x}", pa.0, buffer.as_ptr() as *const u8 as usize);
        // pa.0
    }
    unsafe fn unshare(
        _paddr: virtio_drivers::PhysAddr,
        _buffer: NonNull<[u8]>,
        _direction: BufferDirection,
    ) {
    }
    // fn phys_to_virt(addr: usize) -> usize {
    //     addr + KERNEL_SPACE_OFFSET
    // }
}
#[allow(unused)]
fn virt_to_phys(vaddr: usize) -> usize {
    PageTable::current()
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
            // error!("VirtIOBlock: base={:#x}", VIRTIO0);
            let transport = match MmioTransport::new(header) {
                Ok(t) => {
                    polyhal::println!("MmioTransport created");
                    t
                }
                Err(e) => {
                    panic!("MmioTransport creation failed: {:?}", e);
                }
            };
            // let transport = MmioTransport::new(header).unwrap();
            Self(SleepLock::new(
                VirtIOBlk::<VirtioHal, MmioTransport>::new(transport)
                    .expect("failed to create blk driver"),
            ))
        }
    }
    #[cfg(target_arch = "loongarch64")]
    pub fn new() -> Self {
        // 获取设备树地址（从 bootloader 传入，通常在 a1 寄存器）

        // let fdt_addr = get_fdt_addr();
        let fdt_addr: u64 = FDT_ADDR;

        polyhal::println!("FDT physical address: {:#x}", fdt_addr);
        let magic = unsafe { core::ptr::read_unaligned(fdt_addr as *const u32) };
        polyhal::println!("magic {:#x}", magic);
        let fdt = unsafe { Fdt::from_ptr(fdt_addr as *const u8).unwrap() };
        // fn print_fdt_nodes(fdt: &Fdt) {
        //     for node in fdt.all_nodes() {
        //         println!("Node: {}", node.name);
        //         if let Some(reg) = node.reg().and_then(|mut r| r.next()) {
        //             println!("  reg: base={:#x}, size={:#x}", reg.starting_address as usize, reg.size.unwrap_or(0));
        //         }
        //         if let Some(compat) = node.compatible() {
        //             println!("  compatible: {:?}", compat.all());
        //         }
        //     }
        // }
        // 查找 PCI 节点
        // 使用 ECAM（增强配置访问机制）
        // let pci_node = fdt.find_node("/pci@10000000").unwrap();

        let pci_node = fdt.find_compatible(&["pci-host-ecam-generic"]).unwrap();
        let cam = Cam::Ecam;
        let transport = super::pci::enumerate_pci(pci_node, cam).unwrap();
        polyhal::println!("create transport success");
        Self::new_pci(transport)
    }
    #[cfg(target_arch = "loongarch64")]
    #[allow(unused)]
    pub fn new_pci(transport: PciTransport) -> Self {
        unsafe {
            Self(SleepLock::new(
                VirtIOBlk::<VirtioHal, PciTransport>::new(transport)
                    .expect("failed to create blk driver"),
            ))
        }
    }
}

impl BlockDevice for VirtIOBlock {
    //总字节数
    fn size(&self) -> u64 {
        self.0.lock().capacity() * (BLOCK_SIZE as u64)
    }

    fn block_size(&self) -> usize {
        BLOCK_SIZE
    }

    fn read_block(&self, block_id: usize, buf: &mut [u8]) {
        // info!("Reading block {} with buf len {}", block_id, buf.len());
        // warn!("read_block: block_id={}, buf_len={}", block_id, buf.len());

        let mut blk = self.0.lock();
        assert_ne!(buf.len(), 0);
        assert_eq!(buf.len() % BLOCK_SIZE, 0);
        let capacity = blk.capacity() as usize;
        let sectors = buf.len() / BLOCK_SIZE;
        let _progress = BlockIoProgress::begin(1, block_id, sectors);
        if block_id
            .checked_add(sectors)
            .map_or(true, |end| end > capacity)
        {
            panic!(
                "VirtIOBlk read out of range: block_id={} sectors={} capacity={} buf_len={} buf_va={:#x}",
                block_id,
                sectors,
                capacity,
                buf.len(),
                buf.as_ptr() as usize
            );
        }

        BLK_IO_PHASE.store(2, Ordering::Release);
        let mut bounce = BLK_IO_BOUNCE.lock();
        BLK_IO_PHASE.store(3, Ordering::Release);
        let BlkIoBounce {
            req,
            resp,
            buf: dma_buf,
        } = &mut *bounce;
        let req = &mut req.0;
        let resp = &mut resp.0;
        let bounce_buf = &mut dma_buf.bytes;

        for (chunk_index, chunk) in buf.chunks_mut(BLK_BOUNCE_SIZE).enumerate() {
            *resp = BlkResp::default();
            let sector = block_id + chunk_index * BLK_BOUNCE_SECTORS;
            BLK_IO_CHUNK_SECTOR.store(sector, Ordering::Release);
            let bounce_slice = &mut bounce_buf[..chunk.len()];
            BLK_IO_PHASE.store(4, Ordering::Release);
            let token = match unsafe { blk.read_blocks_nb(sector, req, bounce_slice, resp) } {
                Ok(token) => token,
                Err(err) => {
                    panic!(
                        "Error when submitting VirtIOBlk read: {:?}, block_id={} sector={} sectors={} capacity={} buf_len={} buf_va={:#x}",
                        err,
                        block_id,
                        sector,
                        sectors,
                        capacity,
                        buf.len(),
                        buf.as_ptr() as usize
                    );
                }
            };
            BLK_IO_TOKEN.store(token as usize, Ordering::Release);
            BLK_IO_PHASE.store(5, Ordering::Release);
            let mut polls = 0usize;
            while blk.peek_used() != Some(token) {
                core::hint::spin_loop();
                polls = polls.wrapping_add(1);
                if polls & 0xfff == 0 {
                    BLK_IO_POLLS.store(polls, Ordering::Release);
                }
            }
            BLK_IO_POLLS.store(polls, Ordering::Release);
            BLK_IO_PHASE.store(6, Ordering::Release);
            if let Err(err) = unsafe { blk.complete_read_blocks(token, req, bounce_slice, resp) } {
                panic!(
                    "Error when reading VirtIOBlk: {:?}, block_id={} sector={} sectors={} capacity={} buf_len={} buf_va={:#x}",
                    err,
                    block_id,
                    sector,
                    sectors,
                    capacity,
                    buf.len(),
                    buf.as_ptr() as usize
                );
            }
            validate_block_copy_buffer("read-destination", chunk.as_ptr() as usize, chunk.len());
            chunk.copy_from_slice(bounce_slice);
            BLK_IO_PHASE.store(3, Ordering::Release);
        }
    }

    fn write_block(&self, block_id: usize, buf: &[u8]) {
        // warn!("write_block: block_id={}, buf_len={}", block_id, buf.len());
        let mut blk = self.0.lock();
        assert_ne!(buf.len(), 0);
        assert_eq!(buf.len() % BLOCK_SIZE, 0);
        let capacity = blk.capacity() as usize;
        let sectors = buf.len() / BLOCK_SIZE;
        let _progress = BlockIoProgress::begin(2, block_id, sectors);
        if block_id
            .checked_add(sectors)
            .map_or(true, |end| end > capacity)
        {
            panic!(
                "VirtIOBlk write out of range: block_id={} sectors={} capacity={} buf_len={} buf_va={:#x}",
                block_id,
                sectors,
                capacity,
                buf.len(),
                buf.as_ptr() as usize
            );
        }

        BLK_IO_PHASE.store(2, Ordering::Release);
        let mut bounce = BLK_IO_BOUNCE.lock();
        BLK_IO_PHASE.store(3, Ordering::Release);
        let BlkIoBounce {
            req,
            resp,
            buf: dma_buf,
        } = &mut *bounce;
        let req = &mut req.0;
        let resp = &mut resp.0;
        let bounce_buf = &mut dma_buf.bytes;

        for (chunk_index, chunk) in buf.chunks(BLK_BOUNCE_SIZE).enumerate() {
            let bounce_slice = &mut bounce_buf[..chunk.len()];
            BLK_IO_PHASE.store(31, Ordering::Release);
            validate_block_copy_buffer("write-source", chunk.as_ptr() as usize, chunk.len());
            BLK_IO_PHASE.store(32, Ordering::Release);
            bounce_slice.copy_from_slice(chunk);
            *resp = BlkResp::default();
            let sector = block_id + chunk_index * BLK_BOUNCE_SECTORS;
            BLK_IO_CHUNK_SECTOR.store(sector, Ordering::Release);
            BLK_IO_PHASE.store(4, Ordering::Release);
            let token = match unsafe { blk.write_blocks_nb(sector, req, bounce_slice, resp) } {
                Ok(token) => token,
                Err(err) => {
                    panic!(
                        "Error when submitting VirtIOBlk write: {:?}, block_id={} sector={} sectors={} capacity={} buf_len={} buf_va={:#x}",
                        err,
                        block_id,
                        sector,
                        sectors,
                        capacity,
                        buf.len(),
                        buf.as_ptr() as usize
                    );
                }
            };
            BLK_IO_TOKEN.store(token as usize, Ordering::Release);
            BLK_IO_PHASE.store(5, Ordering::Release);
            let mut polls = 0usize;
            while blk.peek_used() != Some(token) {
                core::hint::spin_loop();
                polls = polls.wrapping_add(1);
                if polls & 0xfff == 0 {
                    BLK_IO_POLLS.store(polls, Ordering::Release);
                }
            }
            BLK_IO_POLLS.store(polls, Ordering::Release);
            BLK_IO_PHASE.store(6, Ordering::Release);
            if let Err(err) = unsafe { blk.complete_write_blocks(token, req, bounce_slice, resp) } {
                panic!(
                    "Error when writing VirtIOBlk: {:?}, block_id={} sector={} sectors={} capacity={} buf_len={} buf_va={:#x}",
                    err,
                    block_id,
                    sector,
                    sectors,
                    capacity,
                    buf.len(),
                    buf.as_ptr() as usize
                );
            }
            BLK_IO_PHASE.store(3, Ordering::Release);
        }
    }

    fn flush(&self) -> SysResult<()> {
        let mut blk = self.0.lock();
        let _progress = BlockIoProgress::begin(3, 0, 0);
        BLK_IO_PHASE.store(4, Ordering::Release);
        blk.flush().map_err(|err| {
            error!("[VIRTIO_BLK_FLUSH] device flush failed: {:?}", err);
            SysError::EIO
        })?;
        BLK_IO_PHASE.store(6, Ordering::Release);
        Ok(())
    }
}

#[cfg(target_arch = "loongarch64")]
pub fn _init_virtio_pci() {
    // 获取设备树地址（从 bootloader 传入，通常在 a1 寄存器）
    // let fdt_addr = get_fdt_addr();
    // let fdt_addr: u64 = 0x9000_0000_0010_0000;
    let fdt_addr: u64 = 0x9000_0000_0ecc_f480;
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

// #[cfg(target_arch = "loongarch64")]
// #[allow(unused)]
// fn get_fdt_addr() -> usize {
//     let fdt_addr: usize;
//     unsafe {
//         core::arch::asm!("move {}, $a1", out(reg) fdt_addr);
//     }
//     fdt_addr
// }
