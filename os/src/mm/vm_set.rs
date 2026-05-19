// use super::page_table;
// use super::page_table::PTEFlags;
use super::heap::*;
use super::vm_area::{KernelMapArea, MapType, UserMapArea};
use super::{
    COW, UserMapAreaType,
    exception::{self, *},
    vm_area,
};
use super::{LazyAlloc, frame_alloc};
use crate::config;
use crate::config::MMAP_BASE;
use crate::config::{MEMORY_END, MMIO};
use alloc::collections::BTreeMap;
use polyhal_trap::trapframe::TrapFrameArgs;
// use crate::config::{
//     KERNEL_STACK_SIZE, MEMORY_END, MMIO, TRAP_CONTEXT, USER_MEMORY_SPACE, USER_STACK_BASE,
//     USER_STACK_SIZE,
// };
use crate::fs::File;
use crate::fs::vfs::OpenFlags;
use crate::fs::vfs::dcache::GLOBAL_DCACHE;
use crate::fs::vfs::file::open_file;
use crate::mm::MmapType;
use crate::mm::vm_area::KernelAreaType;
use crate::mm::{MapArea, vm_set};
use crate::sync::SpinNoIrqLock;
use crate::task::task::TaskControlBlock;
use crate::task::{current_task, current_trap_cx, current_user_token};
use crate::trap::{self};
use alloc::collections::btree_map::Range;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use bitflags::Flags;
use core::arch::{self, asm};
use core::cell::RefCell;
use core::error;
use core::iter::Map;
use core::ops::{Deref, DerefMut};
use core::task;
use lazy_static::*;
use log::*;
use polyhal::common::FrameTracker;
use polyhal::consts::VIRT_ADDR_START;
use polyhal::consts::*;
use polyhal::{print, println};
// use riscv::addr::{Page, page};
// use riscv::paging::PTE;
pub use polyhal::pagetable::*;
pub use polyhal::utils::addr::*;

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
    #[allow(unused)]
    safe fn ekernel();
}
///
#[derive(Debug)]
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
    pub static ref KERNEL_VMSET: Arc<SpinNoIrqLock<KernelVMSet>> =
        Arc::new(SpinNoIrqLock::new(KernelVMSet::new()));
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
        // warn!("unalloc handler");
        let fault_vpn = va.floor();

        // 已映射则无需重复处理，避免二次 map 触发 panic。
        // 兜底：如果已有 PTE 是 RISC-V 保留组合 W=1,R=0，修正它并刷 TLB，否则死循环。
        // 另外，如果 PTE 权限与 area 当前权限不一致（例如 mprotect 修改了权限但 PTE 未更新），
        // 也需要更新 PTE 权限，否则会陷入缺页死循环。
        // 先检查 PTE 是否存在，如果存在则尝试修正权限
        let pte_exists = self.page_table.find_pte(fault_vpn).map(|pte| {
            let flags = pte.flags();
            let ppn = pte.ppn();
            (flags, ppn)
        });
        if let Some((flags, ppn)) = pte_exists {
            if !flags.contains(PTEFlags::V) {
                // PTE 无效，继续处理
            } else if flags.writable() && !flags.readable() {
                // RISC-V 保留组合 W=1,R=0，修正它
                if let Some(pte) = self.page_table.find_pte(fault_vpn) {
                    pte.set_flag(flags | PTEFlags::from(MappingFlags::from(MapPermission::R)));
                }
                TLB::flush_vaddr(va);
                return Some(());
            } else {
                // 检查 PTE 权限是否与 area 当前权限一致
                if let Some(area) = self.find_area(va) {
                    let expected_base = PTEFlags::from(MappingFlags::from(*area.perm())) | PTEFlags::V;
                    let perm_mask = PTEFlags::from(MappingFlags::from(MapPermission::R|MapPermission::W|MapPermission::X|MapPermission::U))| PTEFlags::V;
                    if (flags & perm_mask) != (expected_base & perm_mask) {
                        info!("fixing PTE permissions from {:?} to {:?}", flags, expected_base);
                        if let Some(pte) = self.page_table.find_pte(fault_vpn) {
                            let new_flags = (flags & !perm_mask) | expected_base;
                            *pte = PTE::new(ppn, new_flags);
                        }
                        TLB::flush_vaddr(va);
                    }
                }
                return Some(());
            }
        }

        let (target_ppn, pte_flags) = {
            let area = self.find_area(va)?;
            let frame = if let Some(existing) = area.data_frames.get(&fault_vpn) {
                existing.clone()
            } else {
                let new_frame = match area.areatype() {
                    UserMapAreaType::Heap | UserMapAreaType::Stack | UserMapAreaType::Elf | UserMapAreaType::TrapContext => {
                        Arc::new(frame_alloc().unwrap())
                    }
                    UserMapAreaType::Mmap | UserMapAreaType::Shm => {
                        if let Some(file) = &area.map_file {
                            let offset_in_area = (fault_vpn.0 - area.start_vpn().0) * PAGE_SIZE;
                            let file_offset = area.file_offset + offset_in_area;
                            let page_id = file_offset / PAGE_SIZE;
                            let file_frame = file
                                .get_cache_frame(page_id)
                                .expect("mmap: file does not support page cache");
                            if area.flags == MmapType::MapPrivate {
                                let private_frame = Arc::new(frame_alloc().unwrap());
                                private_frame
                                    .ppn
                                    .get_bytes_array()
                                    .copy_from_slice(file_frame.ppn.get_bytes_array());
                                private_frame
                            } else {
                                file_frame
                            }
                        } else {
                            Arc::new(frame_alloc().unwrap())
                        }
                    }
                    // _ => return None,
                };
                area.data_frames.insert(fault_vpn, new_frame.clone());
                area.clear_lazy_flag();
                new_frame
            };
            (frame.ppn, PTEFlags::from(MappingFlags::from(*area.perm())))
        };

        self.page_table.map_page(
            fault_vpn,
            target_ppn,
            pte_flags.into(),
            MappingSize::Page4KB,
        );
        // info!("handle_unalloc_page_fault mapped vpn {:#x} ok", fault_vpn.0);
        Some(())
    }

    fn handle_cow_page_fault(&mut self, va: VirtAddr) -> Option<()> {
        let vpn = va.floor();
        let _pte = self.page_table.translate(vpn)?;

        // 如果 PTE 已经是可写的，说明这个页已经处理过 COW，直接返回
        if let Some(pte) = self.page_table.translate(vpn) {
            if pte.writable() {
                return Some(());
            }
        }

        let area = self.find_area(va)?;
        let _area_perm = *area.perm();

        let ppn = {
            let frame = area.data_frames.get(&vpn)?;
            let ppn = frame.ppn;
            if Arc::strong_count(frame) == 1 {
                // 引用计数为 1，不需要复制，直接恢复写权限
                area.perm_mut().insert(MapPermission::W);
                ppn
            } else {
                let new_frame = Arc::new(frame_alloc().unwrap());
                let new_ppn = new_frame.ppn;
                new_ppn
                    .get_bytes_array()
                    .copy_from_slice(frame.ppn.get_bytes_array());
                area.data_frames.insert(vpn, new_frame);
                area.perm_mut().insert(MapPermission::W);
                new_ppn
            }
        };

        let flags = PTEFlags::from(MappingFlags::from(*area.perm())) | PTEFlags::V;
        let page_table = self.page_table_mut();
        if let Some(pte) = page_table.find_pte(vpn) {
            *pte = PTE::new(ppn, flags);
        }
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
            error!(
                "perm {:?}",
                PTEFlags::from(MappingFlags::from(*area.perm()))
            );
        } else {
            match access {
                AccessType::Write | AccessType::Read => {
                    if self.try_expand_stack(va).is_some() {
                        return Some(());
                    }
                }
                _ => {}
            }
            error!("no vma found for va: {:#x}", va.0);
            return None;
        }

        // println!(
        //     "enter page fault handler, va = {:#x},{:?}",
        //     va.0, exceptiontype
        // );
        match exceptiontype {
            ExceptionType::Cow => {
                // 如果 PTE 不存在（lazy 分配的页），按 unalloc 处理而不是 COW
                if self.page_table.translate(va.floor()).is_some() {
                    self.handle_cow_page_fault(va)
                } else {
                    self.handle_unalloc_page_fault(va)
                }
            }
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
    pub fn recycle_data_pages(&mut self) -> Vec<UserMapArea> {
        let mut areas = Vec::new();
        core::mem::swap(&mut areas, &mut self.areas);
        areas
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
            .find(|area| area.range_va().start <= va && va <= area.range_va().end)
    }

    /// 尝试向下扩展用户栈，用于处理栈溢出时的缺页异常
    pub(crate) fn try_expand_stack(&mut self, va: VirtAddr) -> Option<()> {
        // 获取当前用户态 sp（trap 上下文中保存的 sp）
        let current_sp = current_trap_cx()[TrapFrameArgs::SP];

        // 找到 va 下方最近的栈区域
        let mut best_idx = None;
        let mut best_start = 0usize;
        for (idx, area) in self.areas.iter().enumerate() {
            if area.areatype() != UserMapAreaType::Stack {
                continue;
            }
            let area_start = area.start_va().0;
            if va.0 < area_start {
                let near_area = area_start.saturating_sub(va.0) <= STACK_EXPAND_LIMIT;
                let near_sp = va.0 >= current_sp.saturating_sub(PAGE_SIZE);
                if near_area || near_sp {
                    if area_start > best_start {
                        best_start = area_start;
                        best_idx = Some(idx);
                    }
                }
            }
        }

        let idx = best_idx?;
        let new_start_vpn = va.floor();
        let old_start_vpn = self.areas[idx].start_vpn();
        if new_start_vpn >= old_start_vpn {
            return None;
        }

        let new_start_va = VirtAddr::from(new_start_vpn.0 * PAGE_SIZE);
        let old_start_va = VirtAddr::from(old_start_vpn.0 * PAGE_SIZE);

        // 总大小限制
        if old_start_va.0 - new_start_va.0 > MAX_STACK_SIZE {
            return None;
        }

        // 检查扩展后是否会与任何其他区域重叠（包括其他线程的栈）
        for other in self.areas.iter() {
            if new_start_va.0 < other.end_va().0 && old_start_va.0 > other.start_va().0 {
                return None;
            }
        }

        let page_table = &mut self.page_table;
        let area = &mut self.areas[idx];
        // 只映射缺页地址所在的那一页，避免一次性分配大量物理页
        let frame = frame_alloc()?;
        let ppn = frame.ppn;
        let zero_ptr = ((ppn.0 << 12) + VIRT_ADDR_START) as *mut u8;
        unsafe {
            core::ptr::write_bytes(zero_ptr, 0, PAGE_SIZE);
        }
        area.data_frames.insert(new_start_vpn, Arc::new(frame));
        page_table.map_page(
            new_start_vpn,
            ppn,
            area.map_perm.into(),
            MappingSize::Page4KB,
        );
        area.range_va_mut().start = new_start_va;
        area.clear_lazy_flag();
        TLB::flush_vaddr(va);
        Some(())
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
                    map_area.flags = match flags & 0x3 {
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
                // 栈统一作为一个连续区域插入
                self.push(
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
                );
            }
            UserMapAreaType::TrapContext => self.push(
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

    #[cfg(target_arch = "riscv64")]
    ///继承内核页表映射
    pub fn from_kernel(kernel_vm_set: &KernelVMSet) -> Self {
        trace!("from_kernel");
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
    #[cfg(target_arch = "loongarch64")]
    ///
    pub fn from_kernel(_kernel_vm_set: &KernelVMSet) -> Self {
        trace!("from_kernel");
        let page_table = PageTable::new();
        page_table
            .root()
            .get_pte_array()
            .copy_from_slice(&_kernel_vm_set.page_table.root().get_pte_array()[..]);
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
                trace!("perm {:?}", map_area.perm().contains(MapPermission::X));
                map_area.copy_data(&self.page_table, data, exact_start_va);
            }
        } else if !map_area.data_frames.is_empty() {
            // lazy 但已有预分配的物理页（如共享内存）：直接建立映射，不复制的帧
            for (&vpn, frame) in map_area.data_frames.iter() {
                self.page_table.map_page(
                    vpn,
                    frame.ppn,
                    map_area.map_perm.into(),
                    MappingSize::Page4KB,
                );
            }
        }
        // 否则 lazy 且 data_frames 为空（普通 mmap/堆/栈），不预映射

        self.areas.push(map_area);
    }

    /// Include sections in elf and trampoline and TrapContext and user stack,
    /// also returns user_sp and entry point.
    pub fn from_elf(elf_data: &[u8]) -> Option<(Self, usize, usize, Vec<(usize, usize)>)> {
        let mut vmset = Self::from_kernel(&KERNEL_VMSET.lock());
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
        let mut max_end_va: usize = 0;
        let mut phdr_addr = 0;
        let mut interp_path: Option<&str> = None;
        for i in 0..ph_count {
            let ph = elf.program_header(i).unwrap();
            if ph.get_type().unwrap() == xmas_elf::program::Type::Interp {
                let path_bytes =
                    &elf.input[ph.offset() as usize..(ph.offset() + ph.file_size()) as usize];
                interp_path = core::str::from_utf8(path_bytes)
                    .ok()
                    .and_then(|s| s.split('\0').next());
                if let Some(path) = interp_path {
                    info!(
                        "[from_elf] Dynamic ELF detected, interpreter path: {}",
                        path
                    );
                }
            }
            if ph.get_type().unwrap() == xmas_elf::program::Type::Phdr {
                phdr_addr = ph.virtual_addr() as usize;
            }
            if ph.get_type().unwrap() == xmas_elf::program::Type::Load {
                 let raw_start_va: VirtAddr = (ph.virtual_addr() as usize).into();
                let raw_end_va: VirtAddr = ((ph.virtual_addr() + ph.mem_size()) as usize).into();
                // 将虚拟地址范围对齐到页面边界，确保 va_range 与页表映射范围一致
                let start_va = VirtAddr::from(raw_start_va.floor().0 * PAGE_SIZE);
                let end_va = VirtAddr::from(raw_end_va.ceil().0 * PAGE_SIZE);
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
                 let end_va_usize: usize = raw_end_va.into();
                if end_va_usize > max_end_va {
                    max_end_va = end_va_usize;
                }
                vmset.push(
                    map_area,
                    Some(&elf.input[ph.offset() as usize..(ph.offset() + ph.file_size()) as usize]),
                     raw_start_va.0,
                );
            }
        }

        let mut interp_base: usize = 0;
        let mut final_entry = elf.header.pt2.entry_point() as usize;

        if let Some(path) = interp_path {
            let root_dentry = match GLOBAL_DCACHE.get("/") {
                Some(d) => d,
                None => {
                    warn!("[from_elf] Failed to get root dentry, cannot load interpreter");
                    return None;
                }
            };
            let interp_file = match open_file(
                root_dentry,
                path,
                OpenFlags::RDONLY,
                crate::fs::vfs::inode::InodeMode::FILE,
            ) {
                Ok(f) => f,
                Err(_) => {
                    warn!("[from_elf] Failed to open interpreter: {}", path);
                    return None;
                }
            };
            let interp_data = interp_file.read_all();
            let interp_elf = match xmas_elf::ElfFile::new(&interp_data) {
                Ok(e) => e,
                Err(_) => {
                    warn!("[from_elf] Interpreter is not a valid ELF");
                    return None;
                }
            };

            interp_base = (max_end_va + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
            info!("[from_elf] Loading interpreter at base {:#x}", interp_base);

            let interp_ph_count = interp_elf.header.pt2.ph_count();
            let mut interp_max_end_va: usize = 0;
            for i in 0..interp_ph_count {
                let ph = interp_elf.program_header(i).unwrap();
                if ph.get_type().unwrap() == xmas_elf::program::Type::Load {
                    let raw_start_va: VirtAddr = (interp_base + ph.virtual_addr() as usize).into();
                    let raw_end_va: VirtAddr =
                        (interp_base + (ph.virtual_addr() + ph.mem_size()) as usize).into();
                    // 将虚拟地址范围对齐到页面边界，确保 va_range 与页表映射范围一致
                    let start_va = VirtAddr::from(raw_start_va.floor().0 * PAGE_SIZE);
                    let end_va = VirtAddr::from(raw_end_va.ceil().0 * PAGE_SIZE);
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
                    let end_va_usize: usize = raw_end_va.into();
                    if end_va_usize > interp_max_end_va {
                        interp_max_end_va = end_va_usize;
                    }
                    vmset.push(
                        map_area,
                        Some(
                            &interp_data
                                [ph.offset() as usize..(ph.offset() + ph.file_size()) as usize],
                        ),
                         raw_start_va.0,
                    );
                }
            }
            max_end_va = interp_max_end_va;
            final_entry = interp_base + interp_elf.header.pt2.entry_point() as usize;
            info!("[from_elf] Interpreter entry point: {:#x}", final_entry);
        }

        let heap_base_vpn = VirtAddr::from(max_end_va).ceil();
        vmset.alloc_user_heap(heap_base_vpn.into());

        let user_stack_top = USER_STACK_BASE;

        if phdr_addr == 0 {
            // 如果没找到 PHDR 段，Fallback 方案：
            let mut elf_base = 0;
            for i in 0..ph_count {
                if let Ok(ph) = elf.program_header(i) {
                    if ph.get_type().unwrap() == xmas_elf::program::Type::Load {
                        elf_base = ph.virtual_addr() as usize - ph.offset() as usize;
                        break;
                    }
                }
            }
            phdr_addr = elf_base + elf.header.pt2.ph_offset() as usize;
        }
        const AT_PHDR: usize = 3;
        const AT_PHENT: usize = 4;
        const AT_PHNUM: usize = 5;
        const AT_PAGESZ: usize = 6;
        const AT_BASE: usize = 7;
        const AT_FLAGS: usize = 8;
        const AT_ENTRY: usize = 9;
        const AT_UID: usize = 11;
        const AT_EUID: usize = 12;
        const AT_GID: usize = 13;
        const AT_EGID: usize = 14;
        const AT_SECURE: usize = 23;
        const AT_CLKTCK: usize = 17;
        let auxv = vec![
            (AT_PHDR, phdr_addr),
            (AT_PHENT, elf.header.pt2.ph_entry_size() as usize),
            (AT_PHNUM, elf.header.pt2.ph_count() as usize),
            (AT_PAGESZ, PAGE_SIZE),
            (AT_BASE, interp_base),
            (AT_FLAGS, 0),
            (AT_ENTRY, elf.header.pt2.entry_point() as usize),
            (AT_UID, 0),
            (AT_EUID, 0),
            (AT_GID, 0),
            (AT_EGID, 0),
            (AT_SECURE, 0),
            (AT_CLKTCK, 100),
        ];

        Some((vmset, user_stack_top, final_entry, auxv))
    }

    #[allow(missing_docs)]
    pub fn from_existed_user(user_vmset: &UserVMSet) -> Self {
        let mut vmset = Self::from_kernel(&KERNEL_VMSET.lock());
        // let mut vmset = Self::new_bare();
        // let pte = user_vmset.translate(VirtPageNum(0x10)).unwrap();
        // println!("user vmset satp {:#x}", user_vmset.token());
        // println!("entry ppn {:#x}", pte.ppn().0);
        // unsafe{
        //     let pgdl: usize;
        //     core::arch::asm!("csrrd {}, 0x1B", out(reg) pgdl);
        //     error!("PGDL = 0x{:016x}", pgdl);
        //     }
        // copy data sections/trap_context/user_stack
        for area in user_vmset.areas.iter() {
            // println!("is lazyalloc {:?}", area.lazy_flag);
            // println!("is cow {:?}", area.cow_flag());
            // println!("area type {:?}", area.areatype());
            let new_area = UserMapArea::from_another(area);

            vmset.push(new_area, None, 0);

            // copy data from another space
            // 只复制已经分配的页面（对 lazy 区域尤其重要）
            for (&vpn, frame) in area.data_frames.iter() {
                let src_ppn = frame.ppn;
                let dst_ppn = vmset.translate(vpn).unwrap().ppn();
                dst_ppn
                    .get_bytes_array()
                    .copy_from_slice(src_ppn.get_bytes_array());
                // info!("src ppn {:#x}, dst ppn {:#x}", src_ppn.0, dst_ppn.0);
            }
        }

        // TLB::flush_all();
        vmset
    }

    ///
    pub fn from_existed_user_cow(user_vmset: &mut UserVMSet) -> Self {
        let mut vmset = Self::from_kernel(&KERNEL_VMSET.lock());
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
            } else if area.areatype() == UserMapAreaType::Shm
                || (area.areatype() == UserMapAreaType::Mmap && area.flags == MmapType::MapShared)
            {
                // 共享内存区域或 mmap MAP_SHARED：父子直接共享物理页，不做 COW，不修改父进程权限
                let new_area = UserMapArea::from_another(area);
                for (&vpn, frame) in area.data_frames.iter() {
                    vmset.page_table.map_page(
                        vpn,
                        frame.ppn,
                        area.map_perm.into(),
                        MappingSize::Page4KB,
                    );
                }
                vmset.areas.push(new_area);
            } else {
                if area.lazy_flag {
                    for vpn in area.vpn_range() {
                        let frame = frame_alloc().unwrap();
                        area.data_frames.insert(vpn, Arc::new(frame));
                    }
                    area.clear_lazy_flag();

                    let frames = area.data_frames.clone();

                    for (vpn, frame) in frames {
                        user_vmset.page_table.map_page(
                            vpn,
                            frame.ppn,
                            MappingFlags::from(*area.perm()),
                            MappingSize::Page4KB,
                        );
                    }
                }

                if area.perm().contains(MapPermission::W) {
                    area.perm_mut().remove(MapPermission::W);
                }
                area.set_cow_flag();
                warn!(
                    "area vpn {:#x}..{:#x}",
                    area.start_vpn().0,
                    area.end_vpn().0
                );

                for vpn in area.data_frames.keys() {
                    // info!("vpn in dataframes {:#x}", vpn.0);
                    frame_page.push((
                        *vpn,
                        PTEFlags::from(MappingFlags::from(*(area.perm()))) | PTEFlags::V,
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
                    panic!("pte not valid {:#x}", frame.0.0);
                }
                pte.set_flag(frame.1);
                let _va = VirtAddr::from(frame.0);
                // sfence_vma_va(va);
                TLB::flush_all();
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
            if current_addr >= USER_MEMORY_SPACE.1 {
                return None;
            }
        }
    }
}

// impl UserVMSet {
//     // 获取指定范围内的内存区域（不可变引用）
//     pub fn get_areas_in_range(&self, start_va: VirtAddr, end_va: VirtAddr) -> Vec<&UserMapArea> {
//         let mut result = Vec::new();
//         let start = start_va.0;
//         let end = end_va.0;

//         for area in self.areas.iter() {
//             let area_start = area.va_range.start;
//             let area_end = area.va_range.end;

//             // 检查区间是否重叠：[area_start, area_end) 与 [start, end) 有交集
//             if usize::from(area_end) > start && usize::from(area_start) < end {
//                 result.push(area);
//             }
//         }

//         result
//     }

//     // 获取指定范围内的内存区域（可变引用）
//     pub fn get_areas_in_range_mut(
//         &mut self,
//         start_va: VirtAddr,
//         end_va: VirtAddr,
//     ) -> Vec<&mut UserMapArea> {
//         let mut result = Vec::new();
//         let start = start_va.0;
//         let end = end_va.0;

//         // 收集索引避免借用冲突
//         let mut indices = Vec::new();
//         for (i, area) in self.areas.iter().enumerate() {
//             let area_start = area.va_range.start;
//             let area_end = area.va_range.end;

//             if usize::from(area_end) > start && usize::from(area_start) < end {
//                 indices.push(i);
//             }
//         }

//         for i in indices {
//             result.push(&mut self.areas[i]);
//         }

//         result
//     }

//     // 获取完全覆盖指定范围的内存区域
//     pub fn get_areas_covering_range(
//         &self,
//         start_va: VirtAddr,
//         end_va: VirtAddr,
//     ) -> Vec<&UserMapArea> {
//         let mut result = Vec::new();
//         let start = start_va.0;
//         let end = end_va.0;

//         for area in self.areas.iter() {
//             let area_start = area.va_range.start;
//             let area_end = area.va_range.end;

//             // 检查范围是否完全在当前区域内
//             if usize::from(area_end) > start && usize::from(area_start) < end {
//                 result.push(area);
//             }
//         }

//         result
//     }

//     // 检查范围是否完全被内存区域覆盖（可以跨多个区域）
//     pub fn is_range_fully_covered(&self, start_va: VirtAddr, end_va: VirtAddr) -> bool {
//         let start = start_va.0;
//         let end = end_va.0;
//         let mut current = start;

//         // 按起始地址排序
//         let mut sorted_areas: Vec<&UserMapArea> = self.areas.iter().collect();
//         sorted_areas.sort_by_key(|a| a.va_range.start);

//         for area in sorted_areas {
//             let area_start = area.va_range.start;
//             let area_end = area.va_range.end;

//             if usize::from(area_start) <= current && usize::from(area_end) > current {
//                 current = usize::from(area_end);
//                 if current >= end {
//                     return true;
//                 }
//             }
//         }

//         false
//     }
// }
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
        warn!("kernel page_table activate");
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
    #[cfg(target_arch = "riscv64")]
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
        // println!("start va {:#x}, end_va {:#x}", stext as usize, etext as usize);

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
        // println!("start va {:#x}, end_va {:#x}", srodata as usize, erodata as usize);

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
        // println!("start va {:#x}, end_va {:#x}", sdata as usize, edata as usize);
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
        println!(
            "start va {:#x}, end_va {:#x}",
            _sbss as usize, _ebss as usize
        );

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
    #[cfg(target_arch = "loongarch64")]
    ///
    pub fn new() -> Self{
        Self::new_bare()
    }
}

#[allow(missing_docs, unused)]
pub fn remap_test() {
    let mut kernel_space = KERNEL_VMSET.lock();
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
