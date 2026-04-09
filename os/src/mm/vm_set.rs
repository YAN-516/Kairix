// use super::page_table;
// use super::page_table::PTEFlags;
use super::heap::*;
use super::vm_area::{KernelMapArea, MapType, UserMapArea};
use super::{
    COW, UserMapAreaType,
    exception::{self, *},
    vm_area,
};
use alloc::collections::BTreeMap;
use super::{LazyAlloc, frame_alloc};
use crate::config;
use crate::config::MMAP_BASE;
use crate::config::{
    KERNEL_STACK_SIZE, MEMORY_END, MMIO, TRAP_CONTEXT, USER_MEMORY_SPACE, USER_STACK_BASE,
    USER_STACK_SIZE,
};
use crate::fs::File;
use crate::mm::{vm_set, MapArea};
use crate::mm::MmapType;
use crate::mm::vm_area::KernelAreaType;
use crate::sync::UPSafeCell;
use crate::task::{current_task, current_user_token};
use crate::task::task::TaskControlBlock;
use crate::trap::{self};
use alloc::collections::btree_map::Range;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use bitflags::Flags;
use core::arch::{self, asm};
use core::cell::RefCell;
use core::iter::Map;
use core::ops::{Deref, DerefMut};
use core::task;
use lazy_static::*;
use log::*;
use polyhal::consts::VIRT_ADDR_START;
use polyhal::{print, println};
use polyhal::common::FrameTracker;
// use riscv::addr::{Page, page};
// use riscv::paging::PTE;
pub use polyhal::pagetable::*;
pub use polyhal::utils::addr::*;
use riscv::paging::PageTableEntry;
#[cfg(target_arch = "riscv64")]
use riscv::register::satp;

// use crate::arch::riscv::sfence_vma_va;
// use crate::arch::TLB;
use crate::task::exit_current_and_run_next;
// use crate::trap::self;
use lazy_static::*;
// use sbi_rt::Sta;

unsafe extern "C" {
    safe fn stext();
    safe fn etext();
    safe fn srodata();
    safe fn erodata();
    safe fn sdata();
    safe fn edata();
    safe fn _sbss();
    safe fn _ebss();
    safe fn ekernel();
}
///
pub enum ExceptionType {
    ///
    Cow,
    ///
    None,
    ///
    Read,
    ///
    Execute,
    ///
    Write,
    ///
    Lazy,
}

lazy_static! {
    /// a memory set instance through lazy_static! managing kernel space
    pub static ref KERNEL_VMSET: Arc<UPSafeCell<KernelVMSet>> =
        Arc::new(unsafe { UPSafeCell::new(KernelVMSet::new()) });
}
///
#[derive(Debug)]
pub enum AccessType {
    ///
    Read,
    ///
    Write,
    ///
    Execute,
    ///
    None,
}

#[allow(missing_docs)]
pub trait VMSpace {
    fn page_table(&self) -> &PageTable;
    fn page_table_mut(&mut self) -> &mut PageTable;
    fn new_bare() -> Self;
    fn token(&self) -> usize;
    fn remove_area_with_start_vpn(&mut self, start_vpn: VirtPageNum);
    fn activate(&self);
    fn translate(&self, vpn: VirtPageNum) -> Option<PTE> {
        self.page_table().translate(vpn)
    }
}
///
pub struct VMSet<A: MapArea> {
    ///
    pub page_table: PageTable,
    areas: Vec<A>,
}
///
impl<A: MapArea> VMSet<A> {
    ///
    pub fn recycle_data_pages(&mut self) {
        self.areas.clear();
    }
    ///
    pub fn init() -> Self {
        Self {
            page_table: PageTable::new(),
            areas: Vec::new(),
        }
    }
}

impl VMSpace for UserVMSet {
    fn page_table(&self) -> &PageTable {
        &self.page_table
    }

    fn page_table_mut(&mut self) -> &mut PageTable {
        &mut self.page_table
    }

    fn new_bare() -> Self {
        Self {
            page_table: PageTable::new(),
            areas: Vec::new(),
        }
    }
    fn token(&self) -> usize {
        self.page_table.token()
    }

    fn remove_area_with_start_vpn(&mut self, start_vpn: VirtPageNum) {
        if let Some((idx, area)) = self
            .areas
            .iter_mut()
            .enumerate()
            .find(|(_, area)| area.start_vpn() == start_vpn)
        {
            area.unmap(&mut self.page_table);
            self.areas.remove(idx);
        }
    }

    fn activate(&self) {
        // let satp = self.page_table.token();
        // unsafe {
        //     satp::write(satp);
        //     asm!("sfence.vma");
        // }
        self.page_table.change();
    }
}
#[allow(missing_docs)]
pub struct UserVMSet {
    pub page_table: PageTable,
    pub areas: Vec<UserMapArea>,
}

impl SetPageFaultException for UserVMSet {
    fn handle_unalloc_page_fault(&mut self, va: VirtAddr) -> Option<()> {
        warn!("unalloc handler");
        let area = self.find_area(va).unwrap();
        let pte_flags: PTEFlags;
        let mut frames: BTreeMap<VirtPageNum, PhysPageNum> = BTreeMap::new();
        match area.areatype() {
            UserMapAreaType::Heap | UserMapAreaType::Stack => {
                error!("heap or stack");
                // let satp = riscv::register::satp::read();
                // println!("{:#x}", satp.bits());
                // if !area.get_lazy_flag(){
                //     return None;
                // }
                error!("start {:#x}, end {:#x}", area.vpn_range().start.0, area.vpn_range().end.0 );

                for vpn in area.vpn_range() {
                    let frame = frame_alloc().unwrap();
                        frames.insert(vpn, frame.ppn);
                        area.data_frames.insert(vpn, Arc::new(frame));
                    // if !area.data_frames.contains_key(&vpn) {
                    //     let frame = frame_alloc().unwrap();
                    //     frames.insert(vpn, frame.ppn);
                    //     area.data_frames.insert(vpn, Arc::new(frame));
                    // }

                }
                area.clear_lazy_flag();
            }
            UserMapAreaType::Mmap => {
                error!("mmap");
                if let Some(file) = &area.map_file {
                    for vpn in area.vpn_range() {
                        let offset_in_area = (vpn.0 - area.start_vpn().0) * PAGE_SIZE;
                        let file_offset = area.file_offset + offset_in_area;
                        let page_id = file_offset / PAGE_SIZE;
                        let frame = file.get_cache_frame(page_id);
                        // let bytes = frame.ppn.get_bytes_array();
                        // let s = core::str::from_utf8(&bytes[0..10]).unwrap_or("INVALID");
                        // println!("[DEBUG mmap] page_id: {}, 内存前10字节: {}", page_id, s);
                        frames.insert(vpn, frame.ppn);
                        area.data_frames.insert(vpn, frame);
                    }
                } else {
                    // 匿名映射
                    for vpn in area.vpn_range() {
                        let frame = frame_alloc().unwrap();
                        frames.insert(vpn, frame.ppn);
                        area.data_frames.insert(vpn, Arc::new(frame));
                    }
                }
                area.clear_lazy_flag();
            }
            _ => return None,
        }
        pte_flags = PTEFlags::from(MappingFlags::from(*area.perm()))| PTEFlags::V;
        // let frames = area.data_frames.clone();
        println!("{:?}",pte_flags);
        for (vpn, ppn) in frames {
            // if let Some(pte) = self.translate(vpn){
            //     println!("pte ppn {:#x}", pte.ppn().0);
            // }else{
            //     println!("no pte found");
            // }
            self.page_table
                .map_page(vpn, ppn, pte_flags.into(), MappingSize::Page4KB);

        }
        self.activate();
        TLB::flush_all();
        Some(())
    }

    fn handle_cow_page_fault(&mut self, va: VirtAddr) -> Option<()> {
        // println!("enter cow handler {:#x}", va.0);
        // let pte = self.page_table.translate(va.floor()).unwrap();
        // println!("{}", pte.bits);
        let area = self.find_area(va).unwrap();
        let mut ppns: Vec<(PhysPageNum, VirtPageNum)> = Vec::new();
        //let vpn = va.floor();
        let data = area.data_frames.clone();
        for vpn in data.keys() {
            //let mut new_ppn = PhysPageNum(0);
            match area.handle_cow_fault(*vpn) {
                Some(ppn) => {
                    ppns.push((ppn, *vpn));
                }
                _ => ppns.push((PhysPageNum(0), *vpn)),
            };
        }

        let flags = PTEFlags::from(MappingFlags::from(*area.perm())) | PTEFlags::V;
        let page_table = self.page_table_mut();
        for (ppn, vpn) in ppns {
            //处理pte
            if let Some(pte) = page_table.find_pte(vpn) {
                if ppn != PhysPageNum(0) {
                    //分配了新页
                    let new_pte = PTE::new(ppn, flags);
                    *pte = new_pte;
                } else {
                    //没有分配新页
                    pte.set_flag(flags);
                }

                //Some(())
            } else {
                panic!("pte not valid");
            }
        }
        // sfence_vma_va(va);
        TLB::flush_vaddr(va);
        Some(())
    }

    fn handle_store_page_fault_set(&mut self, va: VirtAddr, access: AccessType) -> Option<()> {
        // println!(
        //     "enter page fault handler, va = {:#x}, access type = {:?}",
        //     va.0, access
        // );
        let exceptiontype: ExceptionType;

        if let Some(area) = self.find_area(va) {
            exceptiontype = area.access_check(access);
            println!("perm {:?}", PTEFlags::from(MappingFlags::from(*area.perm())));
        } else {
            error!("no vma found for va: {:#x}", va.0);
            return None;
        }
        match exceptiontype {
            ExceptionType::Cow => self.handle_cow_page_fault(va),
            ExceptionType::Write => self.handle_unalloc_page_fault(va),
            ExceptionType::Read => self.handle_unalloc_page_fault(va),
            _ => {
                println!("permission denied");
                None
            }
        }
        // if let Some(pte) = pg.find_pte(vpn) {
        //     println!("PTE: {:?}", pte);
        //     println!("  Valid: {}", pte.is_valid());
        //     println!("  Read: {}", pte.readable());
        //     println!("  Write: {}", pte.writable());
        //     println!("  Execute: {}", pte.executable());
        // } else {
        //     println!("No PTE found!");
        // }
    }
}

impl UserVMSet {
    ///
    pub fn recycle_data_pages(&mut self) {
        self.areas.clear();
    }
    ///
    // pub fn init() -> Self {
    //     Self {
    //         page_table: PageTable::init(),
    //         areas: Vec::new(),
    //     }
    // }

    ///
    pub fn get_heap_area_mut(&mut self) -> &mut UserMapArea {
        self.areas
            .iter_mut()
            .find(|area| area.areatype() == UserMapAreaType::Heap)
            .unwrap()
    }
    ///
    pub fn get_heap_area(&self) -> &UserMapArea {
        &self
            .areas
            .iter()
            .find(|area| area.areatype() == UserMapAreaType::Heap)
            .unwrap()
    }

    ///
    pub fn find_area(&mut self, va: VirtAddr) -> Option<&mut UserMapArea> {
        self.areas
            .iter_mut()
            .find(|area| area.range_va().contains(&va))
    }

    #[allow(missing_docs)]
    pub fn insert_framed_area(
        &mut self,
        start_va: VirtAddr,
        end_va: VirtAddr,
        permission: MapPermission,
        area_type: UserMapAreaType,
        file_info: Option<(Arc<dyn File>, usize, usize)>,
    ) {
        match area_type {
            UserMapAreaType::Heap => self.push(
                UserMapArea::new(
                    start_va,
                    end_va,
                    MapType::Framed,
                    permission,
                    area_type,
                    true,
                ),
                None,
                start_va.0,
            ),
            UserMapAreaType::Mmap => {
                let mut map_area = UserMapArea::new(
                    start_va,
                    end_va,
                    MapType::Framed,
                    permission,
                    area_type,
                    true,
                );
                if let Some((file, file_offset, flags)) = file_info {
                    // 文件映射
                    map_area.map_file = Some(file);
                    map_area.file_offset = file_offset;
                    map_area.flags = match flags {
                        0x1 => MmapType::MapShared,
                        0x2 => MmapType::MapPrivate,
                        _ => MmapType::MapPrivate,
                    };
                } else {
                    // 匿名映射
                    map_area.map_file = None;
                    map_area.flags = MmapType::MapPrivate;
                }

                self.push(map_area, None, start_va.0);
            }
            UserMapAreaType::Stack => {
                let eager_start = VirtAddr::from(end_va.0 - PAGE_SIZE);
                if eager_start.0 > start_va.0 {
                    println!("push lazy area {:#x}", start_va.0);
                    self.push(
                        UserMapArea::new(
                            start_va,
                            eager_start,
                            MapType::Framed,
                            permission,
                            area_type,
                            true,
                        ),
                        None,
                        start_va.0,
                    );
                }
                // 把最顶部的 1 页作为“立即分配区”插入
                println!("push without lazyalloc {:#x} ..{:#x}", eager_start.0, end_va.0);
                self.push(
                    UserMapArea::new(
                        eager_start,
                        end_va,
                        MapType::Framed,
                        permission,
                        area_type,
                        false,
                    ),
                    None,
                    eager_start.0,
                );
                if let Some(pte) = self.translate(eager_start.floor()){
                    println!("pte {:?}, ppn {:#x}", pte.flags(), pte.ppn().0);
                }else{
                    println!("map failed, pte not found");
                }

            }
            _ => self.push(
                UserMapArea::new(
                    start_va,
                    end_va,
                    MapType::Framed,
                    permission,
                    area_type,
                    false,
                ),
                None,
                start_va.0,
            ),
        }
    }

    ///继承内核页表映射
    pub fn from_kernel(kernel_vm_set: &KernelVMSet) -> Self {
        error!("from_kernel");
        let page_table = PageTable::new();
        page_table
            .root()
            .get_pte_array()
            .copy_from_slice(&kernel_vm_set.page_table.root().get_pte_array()[..]);
        Self {
            page_table: page_table,
            areas: Vec::new(),
        }
    }
    ///
    pub fn push(&mut self, mut map_area: UserMapArea, data: Option<&[u8]>, exact_start_va: usize) {
        if !map_area.lazy_flag {
            map_area.map(&mut self.page_table);
            if let Some(data) = data {
                map_area.copy_data(&self.page_table, data, exact_start_va);
            }
        }

        self.areas.push(map_area);
    }

    /// Include sections in elf and trampoline and TrapContext and user stack,
    /// also returns user_sp and entry point.
    pub fn from_elf(elf_data: &[u8]) -> Option<(Self, usize, usize, Vec<(usize, usize)>)> {
        let mut vmset = Self::from_kernel(&KERNEL_VMSET.exclusive_access());
        // map program headers of elf, with U flag
        let elf = match xmas_elf::ElfFile::new(elf_data) {
            Ok(e) => e,
            Err(_) => {
                info!("[DEBUG execve] Not an ELF file! Returning ENOEXEC.");
                return None; // 不是 ELF，直接返回 None
            }
        };
        let elf_header = elf.header;
        let magic = elf_header.pt1.magic;
        assert_eq!(magic, [0x7f, 0x45, 0x4c, 0x46], "invalid elf!");
        let ph_count = elf_header.pt2.ph_count();
        let mut max_end_vpn = VirtPageNum(0);
        let mut phdr_addr = 0;
        for i in 0..ph_count {
            let ph = elf.program_header(i).unwrap();
            if ph.get_type().unwrap() == xmas_elf::program::Type::Interp {
                info!("[CRITICAL WARNING] 该 ELF 是动态链接的！需要加载解释器！");
            }
            if ph.get_type().unwrap() == xmas_elf::program::Type::Phdr {
                phdr_addr = ph.virtual_addr() as usize;
            }
            if ph.get_type().unwrap() == xmas_elf::program::Type::Load {
                let start_va: VirtAddr = (ph.virtual_addr() as usize).into();
                let end_va: VirtAddr = ((ph.virtual_addr() + ph.mem_size()) as usize).into();
                // error!("start_va {:#x}, end_va{:#x}", start_va.0, end_va.0);
                let mut map_perm = MapPermission::U;
                let ph_flags = ph.flags();
                if ph_flags.is_read() {
                    map_perm |= MapPermission::R;
                }
                if ph_flags.is_write() {
                    map_perm |= MapPermission::W;
                }
                if ph_flags.is_execute() {
                    map_perm |= MapPermission::X;
                }
                let map_area = UserMapArea::new(
                    start_va,
                    end_va,
                    MapType::Framed,
                    map_perm,
                    UserMapAreaType::Elf,
                    false,
                );
                max_end_vpn = map_area.end_vpn();
                vmset.push(
                    map_area,
                    Some(&elf.input[ph.offset() as usize..(ph.offset() + ph.file_size()) as usize]),
                    start_va.0,
                );
            }
        }
        let heap_base_vpn = max_end_vpn;
        vmset.alloc_user_heap(heap_base_vpn.into());
        // map user stack with U flags
        //let max_end_va: VirtAddr = max_end_vpn.into();
        //let mut user_stack_bottom: usize = max_end_va.into();

        let user_stack_top = USER_STACK_BASE;
        // {
        //     let user_stack_top = USER_MEMORY_SPACE.1 - PAGE_SIZE; // 0x3fffff000

        //     let user_stack_bottom = user_stack_top - USER_STACK_SIZE; // 0x3ffffd000

        //     //let guard_page = user_stack_bottom - PAGE_SIZE;  // 0x3ffffc000
        //     // guard page
        //     //user_stack_bottom += PAGE_SIZE;

        //     vmset.push(
        //         UserMapArea::new(
        //             user_stack_bottom.into(),
        //             user_stack_top.into(),
        //             MapType::Framed,
        //             MapPermission::R | MapPermission::W | MapPermission::U,
        //         ),
        //         None,
        //     );

        //     //map TrapContext
        //     vmset.push(
        //         UserMapArea::new(
        //             TRAP_CONTEXT.into(),
        //             (USER_MEMORY_SPACE.1).into(),
        //             MapType::Framed,
        //             MapPermission::R | MapPermission::W,
        //         ),
        //         None,
        //     );
        // }

        /*let trap_cx_va = VirtAddr::from(TRAP_CONTEXT);
        if let Some(pte) = vmset.page_table.translate(trap_cx_va.floor()) {
            println!("TrapContext mapped: PPN={:#x}, flags={:?}",
                     pte.ppn().0 << 12, pte.flags());
        } else {
            println!("TrapContext NOT MAPPED!");
            panic!();
        }*/
        /*println!("=== User Process Memory Layout ===");
        println!("Entry point: {:#x}", elf.header.pt2.entry_point() as usize,);
        println!("User stack top: {:#x}",  user_stack_top,);*/
        if phdr_addr == 0 {
            // 如果没找到 PHDR 段，Fallback 方案：
            let mut elf_base = 0;
            if let Ok(ph) = elf.program_header(0) {
                if ph.get_type().unwrap() == xmas_elf::program::Type::Load {
                    elf_base = ph.virtual_addr() as usize - ph.offset() as usize;
                }
            }
            phdr_addr = elf_base + elf.header.pt2.ph_offset() as usize;
        }
        // // 如果可执行文件有基址偏移（非 0 开始加载），计算基址
        // if let Ok(ph) = elf.program_header(0) {
        //     if ph.get_type().unwrap() == xmas_elf::program::Type::Load {
        //         elf_base = ph.virtual_addr() as usize - ph.offset() as usize;
        //     }
        // }
        const AT_PHDR: usize = 3;
        const AT_PHENT: usize = 4;
        const AT_PHNUM: usize = 5;
        const AT_PAGESZ: usize = 6;
        let auxv = vec![
            (AT_PHDR, phdr_addr),
            (AT_PHENT, elf.header.pt2.ph_entry_size() as usize),
            (AT_PHNUM, elf.header.pt2.ph_count() as usize),
            (AT_PAGESZ, PAGE_SIZE),
        ];
        // ==========================================

        Some((
            vmset,
            user_stack_top,
            elf.header.pt2.entry_point() as usize,
            auxv,
        ))
    }

    #[allow(missing_docs)]
    pub fn from_existed_user(user_vmset: &UserVMSet) -> Self {
        let mut vmset = Self::from_kernel(&KERNEL_VMSET.exclusive_access());

        // copy data sections/trap_context/user_stack
        for area in user_vmset.areas.iter() {
            let new_area = UserMapArea::from_another(area);
            vmset.push(new_area, None, 0);
            // copy data from another space
            for vpn in area.vpn_range() {
                let src_ppn = user_vmset.translate(vpn).unwrap().ppn();
                let dst_ppn = vmset.translate(vpn).unwrap().ppn();
                dst_ppn
                    .get_bytes_array()
                    .copy_from_slice(src_ppn.get_bytes_array());
            }
        }
        vmset
    }

    ///
    pub fn from_existed_user_cow(user_vmset: &mut UserVMSet) -> Self {
        let mut vmset = Self::from_kernel(&KERNEL_VMSET.exclusive_access());
        let mut trap_cx_clone: Vec<VirtPageNum> = Vec::new();
        let mut frame_page: Vec<(VirtPageNum, PTEFlags)> = Vec::new();
        for area in user_vmset.areas.iter_mut() {
            //trap_cx不管
            if area.areatype() == UserMapAreaType::TrapContext {
                let new_area = UserMapArea::from_another(area);
                vmset.push(new_area, None, 0);
                // copy data from another space
                for vpn in area.vpn_range() {
                    trap_cx_clone.push(vpn);
                }
            } else {
                if area.perm().contains(MapPermission::W) {
                    area.perm_mut().remove(MapPermission::W);
                }
                area.set_cow_flag();

                for vpn in area.data_frames.keys() {
                    frame_page.push((
                        *vpn,
                        PTEFlags::from_bits(area.perm().bits()).unwrap() | PTEFlags::V,
                    ));
                }
                let new_area = UserMapArea::from_another(&area);
                vmset.push(new_area, None, 0);
            }
        }
        //trap_cx部分数据的复制
        for vpn in trap_cx_clone {
            let src_ppn = user_vmset.page_table.translate(vpn).unwrap().ppn();
            let dst_ppn = vmset.translate(vpn).unwrap().ppn();
            dst_ppn
                .get_bytes_array()
                .copy_from_slice(src_ppn.get_bytes_array());
        }
        //设置页表项
        for frame in frame_page {
            if let Some(pte) = user_vmset.page_table.find_pte(frame.0) {
                if !pte.is_valid() {
                    panic!("pte not valid");
                }
                pte.set_flag(frame.1);
                let va = VirtAddr::from(frame.0);
                // sfence_vma_va(va);
                TLB::flush_vaddr(va);
            } else {
                panic!("illegal vpn to fork");
            }
        }
        vmset
    }

    /// 在用户地址空间找一块没有被占用的虚拟地址区间
    pub fn find_free_area(&self, start: usize, len: usize) -> Option<usize> {
        // 如果没有start，默认从0x4000_0000开始找
        let mut current_addr = if start == 0 { MMAP_BASE } else { start };
        let page_aligned_len = (len + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        loop {
            let current_end = current_addr + page_aligned_len;
            let mut overlap = false;
            for area in self.areas.iter() {
                let area_start = area.start_va().0;
                let area_end = area.end_va().0;
                // 检查区间是否重叠
                if !(current_end <= area_start || current_addr >= area_end) {
                    overlap = true;
                    current_addr = area_end; // 跳到有冲突的区间之后继续找
                    break;
                }
            }
            if !overlap {
                return Some(current_addr);
            }
            if current_addr >= config::USER_MEMORY_SPACE.1 {
                return None;
            }
        }
    }
}

///
pub struct KernelVMSet {
    page_table: PageTable,
    areas: Vec<KernelMapArea>,
}

impl VMSpace for KernelVMSet {
    fn page_table(&self) -> &PageTable {
        &self.page_table
    }

    fn page_table_mut(&mut self) -> &mut PageTable {
        &mut self.page_table
    }

    fn new_bare() -> Self {
        Self {
            page_table: PageTable::new(),
            areas: Vec::new(),
        }
    }
    fn token(&self) -> usize {
        self.page_table.token()
    }

    fn remove_area_with_start_vpn(&mut self, start_vpn: VirtPageNum) {
        if let Some((idx, area)) = self
            .areas
            .iter_mut()
            .enumerate()
            .find(|(_, area)| area.start_vpn() == start_vpn)
        {
            area.unmap(&mut self.page_table);
            self.areas.remove(idx);
        }
    }

    fn activate(&self) {
        // let satp = self.page_table.token();
        // unsafe {
        //     satp::write(satp);
        //     asm!("sfence.vma");
        // }
        self.page_table.change();
    }
}

impl KernelVMSet {
    ///
    pub fn recycle_data_pages(&mut self) {
        self.areas.clear();
    }
    ///
    // pub fn init() -> Self {
    //     Self {
    //         page_table: PageTable::init(),
    //         areas: Vec::new(),
    //     }
    // }
    ///
    pub fn insert_framed_area(
        &mut self,
        start_va: VirtAddr,
        end_va: VirtAddr,
        permission: MapPermission,
        areatype: KernelAreaType,
    ) {
        /*println!("mapping kernel stack");
        println!("  kernel stack top {:#x}", end_va.0);
        println!("  kernel stack bottem {:#x}", start_va.0);*/
        self.push(
            KernelMapArea::new(start_va, end_va, MapType::Framed, permission, areatype),
            None,
        );
    }
    ///
    pub fn push(&mut self, mut map_area: KernelMapArea, data: Option<&[u8]>) {
        map_area.map(&mut self.page_table);
        if let Some(data) = data {
            map_area.copy_data(&self.page_table, data, 0);
        }

        self.areas.push(map_area);
    }
    ///
    pub fn new() -> Self {
        let mut kvm_set = Self::new_bare();
        // map kernel sections

        println!("map kernel sections");
        println!(".text [{:#x}, {:#x})", stext as usize, etext as usize);
        println!(".rodata [{:#x}, {:#x})", srodata as usize, erodata as usize);
        println!(".data [{:#x}, {:#x})", sdata as usize, edata as usize);
        println!(".bss [{:#x}, {:#x})", _sbss as usize, _ebss as usize);
        println!("mapping .text section");
        println!("va = {:#018x}", VirtAddr::from(stext as usize).0);

        kvm_set.push(
            KernelMapArea::new(
                (stext as usize).into(),
                (etext as usize).into(),
                MapType::Identical,
                MapPermission::R | MapPermission::X,
                KernelAreaType::Text,
            ),
            None,
        );
        println!("mapping .rodata section");
        kvm_set.push(
            KernelMapArea::new(
                (srodata as usize).into(),
                (erodata as usize).into(),
                MapType::Identical,
                MapPermission::R,
                KernelAreaType::Rodata,
            ),
            None,
        );
        println!("mapping .data section");
        kvm_set.push(
            KernelMapArea::new(
                (sdata as usize).into(),
                (edata as usize).into(),
                MapType::Identical,
                MapPermission::R | MapPermission::W,
                KernelAreaType::Data,
            ),
            None,
        );
        let vpn = VirtAddr::from(sdata as usize).floor();
        if let Some(pte) = kvm_set.page_table.translate(vpn) {
            println!(
                "  Mapped: PPN={:#x}, flags={:?}",
                pte.ppn().0 << 12,
                pte.flags()
            );
        } else {
            println!("  ERROR: MMIO not mapped!");
        }
        println!("mapping .bss section");
        kvm_set.push(
            KernelMapArea::new(
                (_sbss as usize).into(),
                (_ebss as usize).into(),
                MapType::Identical,
                MapPermission::R | MapPermission::W,
                KernelAreaType::Bss,
            ),
            None,
        );
        println!("mapping physical memory");
        println!(
            "start_va {:#x}, end_va {:#x}",
            ekernel as usize,
            MEMORY_END + VIRT_ADDR_START
        );
        kvm_set.push(
            KernelMapArea::new(
                (ekernel as usize).into(),
                (MEMORY_END + VIRT_ADDR_START).into(),
                MapType::Identical,
                MapPermission::R | MapPermission::W,
                KernelAreaType::PhysMem,
            ),
            None,
        );
        println!("mapping memory-mapped registers");
        for pair in MMIO {
            error!(
                "start_va {:#x} end_va {:#x}",
                (*pair).0,
                (*pair).0 + (*pair).1
            );
            kvm_set.push(
                KernelMapArea::new(
                    ((*pair).0 + VIRT_ADDR_START).into(),
                    (((*pair).0 + (*pair).1) + VIRT_ADDR_START).into(),
                    MapType::Identical,
                    MapPermission::R
                        | MapPermission::W
                        | MapPermission::G
                        | MapPermission::MAT_NOCACHE,
                    KernelAreaType::MemMappedReg,
                ),
                None,
            );
            // let start_virt = (*pair).0 + VIRT_ADDR_START;

            // let vpn = VirtAddr::from(start_virt).floor();

            // if let Some(pte) = kvm_set.page_table.translate(vpn) {
            //     println!("MMIO {:#x}: PPN={:#x}, flags={:?}", pair.0, pte.ppn().0, pte.flags());
            //     // 检查是否可以访问
            //     unsafe {
            //         let ptr = start_virt as *const u32;
            //         let magic = ptr.read_volatile();
            //         println!("  Magic at {:#x}: {:#x}", start_virt, magic);
            //     }
            // } else {
            //     println!("MMIO {}: NOT MAPPED!", pair.0);
            // }
        }
        kvm_set.page_table.change();
        println!("map over");

        kvm_set
    }
}

#[allow(missing_docs, unused)]
pub fn remap_test() {
    let mut kernel_space = KERNEL_VMSET.exclusive_access();
    let mid_text: VirtAddr = (stext as usize + ((etext as usize - stext as usize) >> 1)).into();
    let mid_rodata: VirtAddr =
        (srodata as usize + ((erodata as usize - srodata as usize) >> 1)).into();
    let mid_data: VirtAddr = (sdata as usize + ((edata as usize - sdata as usize) >> 1)).into();
    assert!(
        !kernel_space
            .page_table
            .translate(mid_text.floor())
            .unwrap()
            .writable(),
    );
    assert!(
        !kernel_space
            .page_table
            .translate(mid_rodata.floor())
            .unwrap()
            .writable(),
    );
    assert!(
        !kernel_space
            .page_table
            .translate(mid_data.floor())
            .unwrap()
            .executable(),
    );
    println!("remap_test passed!");
}
///
pub fn user_stack_top() -> usize {
    USER_MEMORY_SPACE.1 - PAGE_SIZE
}
///
pub fn user_stack_bottom() -> usize {
    user_stack_top() - USER_STACK_SIZE
}
