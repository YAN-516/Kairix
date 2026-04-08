use alloc::borrow::ToOwned;
use alloc::sync::Arc;
use bitflags::Flag;
use core::ops::{BitAnd, BitOr, BitXor, Not, Range};
use core::{error, fmt};
use log::{SetLoggerError, error, info};
use polyhal::consts::VIRT_ADDR_START;
#[cfg(target_arch = "riscv64")]
use riscv::register::mcause::Exception;
#[cfg(target_arch = "riscv64")]
use sbi_rt::StartFlags;

use super::vm_set::{AccessType, ExceptionType};
use super::{exception::*, frame_alloc, frame_allocator};
use crate::fs::File;
use xmas_elf::sections;
// use super::{
//     PTEFlags, PageTable, PageTableEntry,
// };
use polyhal::common::FrameTracker;
pub use polyhal::pagetable::*;
pub use polyhal::utils::addr::*;

// use crate::arch::riscv::sfence_vma_va;
// use crate::config::{KERNEL_SPACE_OFFSET, PAGE_SIZE};
use alloc::collections::BTreeMap;
// use crate::arch::TLB;

bitflags! {
    #[derive(Clone, Copy)]
    /// map permission corresponding to that in pte: `R W X U`
    pub struct MapPermission: u64 {
        ///Readable
        const R = 1 << 1;
        ///Writable
        const W = 1 << 2;
        ///Excutable
        const X = 1 << 3;
        ///Accessible in U mode
        const U = 1 << 4;
        ///GLOBAL USED IN LA
        const G = 1 << 5;
        ///NOCACHE
        const MAT_NOCACHE = 1 << 6;
        #[allow(missing_docs)]
        const RW = Self::R.bits() | Self::W.bits();
        #[allow(missing_docs)]
        const RX = Self::R.bits() | Self::X.bits();
        #[allow(missing_docs)]
        const WX = Self::W.bits() | Self::X.bits();
        #[allow(missing_docs)]
        const RWX = Self::W.bits() | Self::X.bits() | Self::R.bits();

        #[allow(missing_docs)]
        const URW = Self::U.bits() | Self::R.bits() | Self::W.bits();
        #[allow(missing_docs)]
        const URX = Self::U.bits() | Self::R.bits() | Self::X.bits();
        #[allow(missing_docs)]
        const UWX = Self::U.bits() | Self::W.bits() | Self::X.bits();
        #[allow(missing_docs)]
        const URWX = Self::U.bits() | Self::W.bits() | Self::X.bits() | Self::R.bits();
        #[allow(missing_docs)]
        const UW = Self::U.bits() | Self::W.bits();
    }
}

impl MapPermission {
    /// 将 C 语言用户态传进来的 prot (PROT_READ / PROT_WRITE / PROT_EXEC)
    /// 安全地转换为内核的 MapPermission
    pub fn from_prot(prot: usize) -> Self {
        const PROT_READ: usize = 1;
        const PROT_WRITE: usize = 2;
        const PROT_EXEC: usize = 4;
        let mut perm = MapPermission::U;
        if (prot & PROT_READ) != 0 {
            perm |= MapPermission::R;
        }
        if (prot & PROT_WRITE) != 0 {
            perm |= MapPermission::W;
        }
        if (prot & PROT_EXEC) != 0 {
            perm |= MapPermission::X;
        }

        perm
    }
}
impl Into<MappingFlags> for MapPermission {
    fn into(self) -> MappingFlags {
        let mut flags = MappingFlags::empty();
        if self.contains(MapPermission::R) {
            flags |= MappingFlags::R;
        }
        if self.contains(MapPermission::W) {
            flags |= MappingFlags::W;
        }
        if self.contains(MapPermission::X) {
            flags |= MappingFlags::X;
        }
        if self.contains(MapPermission::U) {
            flags |= MappingFlags::U;
        }
        if self.contains(MapPermission::G) {
            flags |= MappingFlags::G;
        }
        if !self.contains(MapPermission::MAT_NOCACHE) {
            flags |= MappingFlags::Cache;
        }
        flags
    }
}

#[allow(unused)]
#[derive(Copy, Clone, PartialEq, Debug)]
#[allow(missing_docs)]
pub enum MapType {
    ///内核线性映射
    Identical,
    ///独立映射
    Framed,
}
#[allow(unused)]
#[allow(missing_docs)]
pub trait MapArea {
    fn range_va(&self) -> &Range<VirtAddr>;

    fn range_va_mut(&mut self) -> &mut Range<VirtAddr>;

    fn start_va(&self) -> VirtAddr {
        self.range_va().start
    }
    fn end_va(&self) -> VirtAddr {
        self.range_va().end
    }

    fn vpn_range(&self) -> Range<VirtPageNum> {
        self.start_vpn()..self.end_vpn()
    }
    fn start_vpn(&self) -> VirtPageNum {
        self.start_va().floor()
    }
    fn end_vpn(&self) -> VirtPageNum {
        self.end_va().ceil()
    }
    fn perm(&self) -> &MapPermission;
    fn perm_mut(&mut self) -> &mut MapPermission;

    fn map_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum);
    fn unmap_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum);
    fn map(&mut self, page_table: &mut PageTable);
    fn unmap(&mut self, page_table: &mut PageTable);

    // fn copy_data(&mut self, page_table: &PageTable, data: &[u8]) {
    //     //assert_eq!(self.map_type, MapType::Framed);
    //     let mut start: usize = 0;
    //     let mut current_vpn = self.start_vpn();
    //     let len = data.len();
    //     loop {
    //         let src = &data[start..len.min(start + PAGE_SIZE)];
    //         let dst = &mut page_table
    //             .translate(current_vpn)
    //             .unwrap()
    //             .ppn()
    //             .get_bytes_array()[..src.len()];
    //         dst.copy_from_slice(src);
    //         start += PAGE_SIZE;
    //         if start >= len {
    //             break;
    //         }
    //         current_vpn.step();
    //     }
    // }
    //按照传入的虚拟地址和数据，进行跨页复制，之前是忽略起始的offset，这里进行了debug修复
    fn copy_data(&mut self, page_table: &PageTable, data: &[u8], mut exact_start_va: usize) {
        let mut offset = 0;
        while offset < data.len() {
            let page_offset = exact_start_va % PAGE_SIZE;
            let write_len = (PAGE_SIZE - page_offset).min(data.len() - offset);
            let ppn = page_table
                .translate(VirtAddr::from(exact_start_va).floor())
                .unwrap()
                .ppn();
            let dst_ptr = (ppn.0 << 12) + page_offset;
            let dst_slice = &mut ppn.get_bytes_array()[page_offset..page_offset + write_len];
            let src_slice = &data[offset..offset + write_len];
            dst_slice.copy_from_slice(src_slice);
            exact_start_va += write_len;
            offset += write_len;
        }
    }
}
#[derive(Debug)]
#[derive(Clone, Copy, PartialEq, Eq)]
///
pub enum UserMapAreaType {
    ///
    Elf,
    ///
    Stack,
    ///
    Heap,
    ///
    TrapContext,
    ///
    Mmap,
}
#[derive(Clone, Copy, PartialEq, Eq)]
///
pub enum MmapType {
    ///共享映射
    MapShared,
    ///私有映射
    MapPrivate,
}

///
pub trait LazyAlloc {
    ///
    fn get_lazy_flag(&self) -> bool;
    ///
    fn set_lazy_flag(&mut self);
    ///
    fn clear_lazy_flag(&mut self);
}
#[allow(missing_docs)]
pub struct UserMapArea {
    va_range: VARange,
    pub data_frames: BTreeMap<VirtPageNum, Arc<FrameTracker>>,
    pub map_type: MapType,
    map_perm: MapPermission,
    pub area_type: UserMapAreaType,
    cow_flag: bool,
    pub lazy_flag: bool,
    pub map_file: Option<Arc<dyn File>>, // 绑定的文件，匿名映射就是 None
    pub file_offset: usize,              // 映射从文件的哪个字节开始
    pub flags: MmapType,                 // mmap 的 flags，比如 MAP_SHARED 还是 MAP_PRIVATE
}

impl LazyAlloc for UserMapArea {
    fn clear_lazy_flag(&mut self) {
        self.lazy_flag = false;
    }
    fn get_lazy_flag(&self) -> bool {
        self.lazy_flag
    }
    fn set_lazy_flag(&mut self) {
        self.lazy_flag = true;
    }
}

#[allow(unused)]
#[allow(missing_docs)]
impl UserMapArea {
    pub fn expand(&mut self, end_va: VirtAddr) {
        self.va_range.end = end_va
    }
    pub fn access_check(&self, access: AccessType) -> ExceptionType {
        match access {
            AccessType::Read => {
                if self.perm().contains(MapPermission::R) {
                    ExceptionType::Read
                } else {
                    ExceptionType::None
                }
            }
            AccessType::Write => {
                if self.cow_flag {
                    ExceptionType::Cow
                } else if self.perm().contains(MapPermission::W) {
                    ExceptionType::Write
                } else {
                    ExceptionType::None
                }
            }
            AccessType::Execute => {
                if self.perm().contains(MapPermission::X) {
                    ExceptionType::Execute
                } else {
                    ExceptionType::None
                }
            }
            _ => ExceptionType::None,
        }
    }

    pub fn new(
        start_va: VirtAddr,
        end_va: VirtAddr,
        map_type: MapType,
        map_perm: MapPermission,
        area_type: UserMapAreaType,
        lazy_flag: bool,
    ) -> Self {
        Self {
            va_range: start_va..end_va,
            data_frames: BTreeMap::new(),
            map_type: map_type,
            map_perm: map_perm,
            area_type,
            cow_flag: false,
            lazy_flag,
            map_file: None,
            file_offset: 0,
            flags: MmapType::MapPrivate,
        }
    }
    pub fn areatype(&self) -> UserMapAreaType {
        self.area_type
    }
    pub fn from_another(another: &UserMapArea) -> Self {
        Self {
            va_range: another.start_va()..another.end_va(),
            data_frames: another.data_frames.clone(),
            map_type: another.map_type,
            map_perm: another.map_perm,
            area_type: another.area_type,
            cow_flag: another.cow_flag,
            lazy_flag: another.lazy_flag,
            map_file: another.map_file.clone(),
            file_offset: another.file_offset,
            flags: another.flags,
        }
    }
}

impl MapArea for UserMapArea {
    fn range_va(&self) -> &Range<VirtAddr> {
        &self.va_range
    }
    fn range_va_mut(&mut self) -> &mut Range<VirtAddr> {
        &mut self.va_range
    }
    fn perm(&self) -> &MapPermission {
        &self.map_perm
    }
    fn perm_mut(&mut self) -> &mut MapPermission {
        &mut self.map_perm
    }
    fn map_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
        if let Some(pte) = page_table.translate(vpn) {
            if pte.is_valid() {
                // 硬件页表里已经有这个页了，直接跳过！不会触发 polyhal 的 panic
                // warn!("Hardware PTE already valid for vpn {:#x}, skip mapping", vpn.0);
                return;
            }
        }
        let ppn: PhysPageNum;

        let frame = frame_alloc().unwrap();
        ppn = frame.ppn;

        // if vpn.0 == 0x10||vpn.0 == 0x11{
        //     error!("pagetable {:#x}", page_table.root().0);
        //     error!("vpn {:#x}", vpn.0);
        //     error!("ppn {:#x}", ppn.0);
        // }

        self.data_frames.insert(vpn, Arc::new(frame));
        // let pte_flags = PTEFlags::from_bits(self.map_perm.bits()).unwrap();
        page_table.map_page(vpn, ppn, self.map_perm.into(), MappingSize::Page4KB);
    }
    fn unmap_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
        self.data_frames.remove(&vpn);
        page_table.unmap_page(vpn);
    }
    fn map(&mut self, page_table: &mut PageTable) {
        let vpn_range = VPNRange::new(self.start_va().floor(), self.end_va().ceil());
        if !self.cow_flag {
            match self.area_type {
                UserMapAreaType::Elf | UserMapAreaType::TrapContext => {
                    for vpn in vpn_range {
                        // if self.start_va().0 == 0x10000{
                        //     error!("{:#x}", vpn.0);
                        // }
                        self.map_one(page_table, vpn);
                    }
                }
                _ => {
                    for vpn in vpn_range {
                        self.map_one(page_table, vpn);
                    }
                }
            }
        } else {
            for (&vpn, frame) in self.data_frames.iter() {
                self.map_cow(page_table, vpn, frame.ppn);
            }
        }
    }
    fn unmap(&mut self, page_table: &mut PageTable) {
        // let vpn_range = VPNRange::new(self.start_va().floor(), self.end_va().ceil());
        // for vpn in vpn_range {
        //     self.unmap_one(page_table, vpn);
        // }
        for vpn in self.vpn_range() {
            if self.data_frames.contains_key(&vpn) {
                self.unmap_one(page_table, vpn);
            }
        }
    }
}

///
pub trait COW {
    ///
    fn cow_flag(&self) -> bool;
    ///
    fn set_cow_flag(&mut self);
    ///
    fn clear_cow_flag(&mut self);
    ///
    fn map_cow(&self, page_table: &mut PageTable, vpn: VirtPageNum, ppn: PhysPageNum);
}

impl COW for UserMapArea {
    fn cow_flag(&self) -> bool {
        self.cow_flag
    }

    fn clear_cow_flag(&mut self) {
        self.cow_flag = false;
    }

    fn set_cow_flag(&mut self) {
        self.cow_flag = true;
    }

    fn map_cow(&self, page_table: &mut PageTable, vpn: VirtPageNum, ppn: PhysPageNum) {
        //info!("map_cow start vma:{:#x}, end vma:{:#x}",vpn.0,vpn.0 + PAGE_SIZE);
        let pte_flags = PTEFlags::from_bits(self.map_perm.bits()).unwrap();
        page_table.map_page(vpn, ppn, pte_flags.into(), MappingSize::Page4KB);
    }
}

#[allow(unused, missing_docs)]
pub struct KernelMapArea {
    va_range: VARange,
    data_frames: BTreeMap<VirtPageNum, FrameTracker>,
    map_type: MapType,
    map_perm: MapPermission,
    area_type: KernelAreaType,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(unused, missing_docs)]
pub enum KernelAreaType {
    Text,
    Rodata,
    Data,
    Bss,
    PhysMem,
    MemMappedReg,
    KernelStack,
}

#[allow(unused, missing_docs)]
impl KernelMapArea {
    pub fn new(
        start_va: VirtAddr,
        end_va: VirtAddr,
        map_type: MapType,
        map_perm: MapPermission,
        area_type: KernelAreaType,
    ) -> Self {
        let range = start_va..end_va;

        Self {
            va_range: start_va..end_va,
            data_frames: BTreeMap::new(),
            map_type: map_type,
            map_perm: map_perm,
            area_type: area_type,
        }
    }

    #[allow(missing_docs)]
    pub fn from_another(another: &KernelMapArea) -> Self {
        Self {
            va_range: another.start_va()..another.end_va(),
            data_frames: BTreeMap::new(),
            map_type: another.map_type,
            map_perm: another.map_perm,
            area_type: another.area_type,
        }
    }

    fn identical_map(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
        let ppn = PhysPageNum(vpn.0 & !(VIRT_ADDR_START >> 12));
        // println!("{}", flags.bits());
        page_table.map_page(vpn, ppn, (*self.perm()).into(), MappingSize::Page4KB);
    }

    fn frame_map(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
        let ppn: PhysPageNum;
        let frame = frame_alloc().unwrap();
        ppn = frame.ppn;
        self.data_frames.insert(vpn, frame);
        page_table.map_page(vpn, ppn, (*self.perm()).into(), MappingSize::Page4KB);
    }
}

impl MapArea for KernelMapArea {
    fn range_va(&self) -> &Range<VirtAddr> {
        &self.va_range
    }

    fn range_va_mut(&mut self) -> &mut Range<VirtAddr> {
        &mut self.va_range
    }

    fn perm(&self) -> &MapPermission {
        &self.map_perm
    }

    fn perm_mut(&mut self) -> &mut MapPermission {
        &mut self.map_perm
    }

    fn map_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
        match self.area_type {
            KernelAreaType::Bss
            | KernelAreaType::Data
            | KernelAreaType::MemMappedReg
            | KernelAreaType::PhysMem
            | KernelAreaType::Rodata
            | KernelAreaType::Text => self.identical_map(page_table, vpn),

            KernelAreaType::KernelStack => self.frame_map(page_table, vpn),
        }
    }

    fn unmap_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
        match self.area_type {
            KernelAreaType::Bss
            | KernelAreaType::Data
            | KernelAreaType::MemMappedReg
            | KernelAreaType::PhysMem
            | KernelAreaType::Rodata
            | KernelAreaType::Text => page_table.unmap_page(vpn),

            KernelAreaType::KernelStack => {
                self.data_frames.remove(&vpn);
                page_table.unmap_page(vpn);
            }
        }
    }

    fn map(&mut self, page_table: &mut PageTable) {
        let vpn_range = VPNRange::new(self.start_vpn(), self.end_vpn());

        for vpn in vpn_range {
            self.map_one(page_table, vpn);
        }
    }

    fn unmap(&mut self, page_table: &mut PageTable) {
        let vpn_range = VPNRange::new(self.start_vpn(), self.end_vpn());
        for vpn in vpn_range {
            self.unmap_one(page_table, vpn);
        }
    }
}
