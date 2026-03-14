use core::ops::{BitAnd, BitOr, BitXor, Not, Range};
use core::fmt;
use alloc::borrow::ToOwned;
use bitflags::Flag;
use log::SetLoggerError;
use sbi_rt::StartFlags;

use super::{FrameTracker, frame_alloc};
use super::{PTEFlags, PageTable, VPNRange, VirtPageNum, VirtAddr, StepByOne, 
    VARange, PhysAddr, PhysPageNum};
use crate::config::{KERNEL_SPACE_OFFSET, PAGE_SIZE};
use alloc::collections::BTreeMap;



bitflags! {
    #[derive(Clone, Copy)] 
    /// map permission corresponding to that in pte: `R W X U`
    pub struct MapPermission: u8 {
        ///Readable
        const R = 1 << 1;
        ///Writable
        const W = 1 << 2;
        ///Excutable
        const X = 1 << 3;
        ///Accessible in U mode
        const U = 1 << 4;
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

    fn start_va(&self) -> VirtAddr{
        self.range_va().start
    }
    fn end_va(&self) -> VirtAddr{
        self.range_va().end
    }

    fn vpn_range(&self) -> Range<VirtPageNum>{
        self.start_vpn()..self.end_vpn()
    }
    fn start_vpn(&self) -> VirtPageNum{
        self.start_va().floor()
    }
    fn end_vpn(&self) -> VirtPageNum{
        self.end_va().ceil()
    }
    fn perm(&self) -> &MapPermission;
    fn perm_mut(&mut self) -> &mut MapPermission;

    fn map_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum);
    fn unmap_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum);
    fn map(&mut self, page_table: &mut PageTable);
    fn unmap(&mut self, page_table: &mut PageTable);

    fn copy_data(&mut self, page_table: &PageTable, data: &[u8]){
        //assert_eq!(self.map_type, MapType::Framed);
        let mut start: usize = 0;
        let mut current_vpn = self.start_vpn();
        let len = data.len();
        loop {
            let src = &data[start..len.min(start + PAGE_SIZE)];
            let dst = &mut page_table
                .translate(current_vpn)
                .unwrap()
                .ppn()
                .get_bytes_array()[..src.len()];
            dst.copy_from_slice(src);
            start += PAGE_SIZE;
            if start >= len {
                break;
            }
            current_vpn.step();
        }
    }


}
#[allow(missing_docs)]
pub struct UserMapArea {
    va_range: VARange,
    data_frames: BTreeMap<VirtPageNum, FrameTracker>,
    map_type: MapType,
    map_perm: MapPermission,
}

#[allow(unused)]
#[allow(missing_docs)]
impl UserMapArea {
    pub fn new(start_va: VirtAddr,
        end_va: VirtAddr,
        map_type: MapType,
        map_perm: MapPermission,) -> Self{
        Self { va_range: start_va..end_va, 
            data_frames: BTreeMap::new(), 
            map_type: map_type, 
            map_perm: map_perm 
        }
    }
    pub fn from_another(another: &UserMapArea) -> Self {
        Self {
            va_range: another.start_va()..another.end_va(),
            data_frames: BTreeMap::new(),
            map_type: another.map_type,
            map_perm: another.map_perm,
        }
    }
    ///懒分配只映射
    pub fn lazy_map(&mut self, page_table: &mut PageTable) {
        let vpn_range = VPNRange::new(self.start_va().floor(), self.end_va().ceil());
        for vpn in vpn_range{
            self.lazy_map_one(page_table, vpn);
        }
    }
    pub fn lazy_map_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
        let ppn: PhysPageNum;
        let frame = frame_alloc().unwrap();
        ppn = frame.ppn;
        self.data_frames.insert(vpn, frame);
        let pte_flags = PTEFlags::from_bits(self.map_perm.bits()).unwrap();
        page_table.lazy_map(vpn, ppn, pte_flags);
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
        let ppn: PhysPageNum;
        let frame = frame_alloc().unwrap();
        ppn = frame.ppn;
        self.data_frames.insert(vpn, frame);
        let pte_flags = PTEFlags::from_bits(self.map_perm.bits()).unwrap();
        page_table.map(vpn, ppn, pte_flags);
    }
    fn unmap_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
        self.data_frames.remove(&vpn);
        page_table.unmap(vpn);
    }
    fn map(&mut self, page_table: &mut PageTable) {
        let vpn_range = VPNRange::new(self.start_va().floor(), self.end_va().ceil());
        for vpn in vpn_range{
            self.map_one(page_table, vpn);
        }
    }
    
    fn unmap(&mut self, page_table: &mut PageTable) {
        let vpn_range = VPNRange::new(self.start_va().floor(), self.end_va().ceil());
        for vpn in vpn_range{
            self.unmap_one(page_table, vpn);
        }
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
    Text, Rodata, Data, Bss, PhysMem, MemMappedReg, KernelStack, INIT,
}

#[allow(unused, missing_docs)]
impl KernelMapArea {
    pub fn new(start_va: VirtAddr,
        end_va: VirtAddr,
        map_type: MapType,
        map_perm: MapPermission,
        area_type: KernelAreaType) -> Self{
        let range = start_va..end_va;

        Self { va_range: start_va..end_va, 
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

    fn identical_map(&mut self, page_table: &mut PageTable, vpn: VirtPageNum){
        let ppn = PhysPageNum(vpn.0 & !(KERNEL_SPACE_OFFSET>>12));
        let flags = PTEFlags::from_bits(self.map_perm.bits()).unwrap();
        page_table.map(vpn, ppn, flags);
    }

    fn frame_map(&mut self, page_table: &mut PageTable, vpn: VirtPageNum){
        let ppn: PhysPageNum;
        let frame = frame_alloc().unwrap();
        ppn = frame.ppn;
        self.data_frames.insert(vpn, frame);
        let flags = PTEFlags::from_bits(self.map_perm.bits()).unwrap();
        page_table.lazy_map(vpn, ppn, flags);
    }

    fn init_map(&mut self, page_table: &mut PageTable, vpn: VirtPageNum){
        let ppn: PhysPageNum;
        let frame = frame_alloc().unwrap();
        ppn = frame.ppn;
        self.data_frames.insert(vpn, frame);
        let flags = PTEFlags::from_bits(self.map_perm.bits()).unwrap();
        page_table.map(vpn, ppn, flags);
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
            KernelAreaType::Bss|KernelAreaType::Data|KernelAreaType::MemMappedReg|
            KernelAreaType::PhysMem|KernelAreaType::Rodata|KernelAreaType::Text
            => self.identical_map(page_table, vpn),

            KernelAreaType::KernelStack
            => {
                self.init_map(page_table, vpn);
            },

            KernelAreaType::INIT => self.init_map(page_table, vpn),
        }
    }

    fn unmap_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
        match self.area_type {
            KernelAreaType::Bss|KernelAreaType::Data|KernelAreaType::MemMappedReg|
            KernelAreaType::PhysMem|KernelAreaType::Rodata|KernelAreaType::Text
            => page_table.unmap(vpn),

            KernelAreaType::KernelStack | KernelAreaType::INIT
            => {
                self.data_frames.remove(&vpn);
                page_table.unmap(vpn);
            },
        }
    }

    fn map(&mut self, page_table: &mut PageTable) {
        let vpn_range = VPNRange::new(self.start_vpn(), self.end_vpn());

        for vpn in vpn_range{
            self.map_one(page_table, vpn);
        }
    }

    fn unmap(&mut self, page_table: &mut PageTable) {
        let vpn_range = VPNRange::new(self.start_vpn(), self.end_vpn());
        for vpn in vpn_range{
            self.unmap_one(page_table, vpn);
        }
    }
}

