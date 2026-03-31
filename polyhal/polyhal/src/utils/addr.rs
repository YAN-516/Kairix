//! Implementation of physical and virtual address and page number.
//use sbi_rt::legacy::set_timer;
// use super::PageTableEntry;
// use crate::config::{
//     KERNEL_MEMORY_SPACE, KERNEL_SPACE_OFFSET, PAGE_SIZE, PAGE_SIZE_BITS, PTES_PER_PAGE,
//     USER_MEMORY_SPACE,
// };

use crate::arch::consts::*;
use crate::pagetable::PTE;

use core::fmt::{self, Debug, Formatter};
use core::ops::Range;
use sbi_rt::{Timer, set_timer};
const PA_WIDTH_SV39: usize = 56;
const VA_WIDTH_SV39: usize = 39;
const PPN_WIDTH_SV39: usize = PA_WIDTH_SV39 - PAGE_SIZE_BITS;
#[allow(unused)]
const VPN_WIDTH_SV39: usize = VA_WIDTH_SV39 - PAGE_SIZE_BITS;
use core::iter::Step;

/// Definitions
#[repr(C)]
#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq)]
pub struct PhysAddr(pub usize);

#[repr(C)]
#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq)]
///virtual address
pub struct VirtAddr(pub usize);

#[repr(C)]
#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq)]
///phiscal page number
pub struct PhysPageNum(pub usize);

#[repr(C)]
#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq)]
///virtual page number
pub struct VirtPageNum(pub usize);

impl Step for VirtPageNum {
    fn steps_between(start: &Self, end: &Self) -> (usize, Option<usize>) {
        usize::steps_between(&start.0, &end.0)
    }

    fn forward_checked(start: Self, count: usize) -> Option<Self> {
        start.0.checked_add(count).map(VirtPageNum)
    }

    fn backward_checked(start: Self, count: usize) -> Option<Self> {
        start.0.checked_sub(count).map(VirtPageNum)
    }
}
/// Debugging

impl Debug for VirtAddr {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("VA:{:#x}", self.0))
    }
}
impl Debug for VirtPageNum {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("VPN:{:#x}", self.0))
    }
}
impl Debug for PhysAddr {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("PA:{:#x}", self.0))
    }
}
impl Debug for PhysPageNum {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("PPN:{:#x}", self.0))
    }
}

/// T: {PhysAddr, VirtAddr, PhysPageNum, VirtPageNum}
/// T -> usize: T.0
/// usize -> T: usize.into()

impl From<usize> for PhysAddr {
    fn from(v: usize) -> Self {
        Self(v & ((1 << PA_WIDTH_SV39) - 1))
    }
}
impl From<usize> for PhysPageNum {
    fn from(v: usize) -> Self {
        Self(v & ((1 << PPN_WIDTH_SV39) - 1))
    }
}
impl From<usize> for VirtAddr {
    fn from(v: usize) -> Self {
        //Self(v & ((1 << VA_WIDTH_SV39) - 1))
        Self(v)
    }
}
impl From<usize> for VirtPageNum {
    fn from(v: usize) -> Self {
        //Self(v & ((1 << VPN_WIDTH_SV39) - 1))
        Self(v)
    }
}
impl From<PhysAddr> for usize {
    fn from(v: PhysAddr) -> Self {
        v.0
    }
}
impl From<PhysPageNum> for usize {
    fn from(v: PhysPageNum) -> Self {
        v.0
    }
}
impl From<VirtAddr> for usize {
    fn from(v: VirtAddr) -> Self {
        /*if v.0 >= (1 << (VA_WIDTH_SV39 - 1)) {
            v.0 | (!((1 << VA_WIDTH_SV39) - 1))
        } else {
            v.0
        }*/
        v.0
    }
}
impl From<VirtPageNum> for usize {
    fn from(v: VirtPageNum) -> Self {
        v.0
    }
}
///
impl VirtAddr {
    ///`VirtAddr`->`VirtPageNum`
    pub fn floor(&self) -> VirtPageNum {
        VirtPageNum(self.0 / PAGE_SIZE)
    }
    ///`VirtAddr`->`VirtPageNum`
    pub fn ceil(&self) -> VirtPageNum {
        if self.0 == 0 {
            VirtPageNum(0)
        } else {
            VirtPageNum((self.0 - 1 + PAGE_SIZE) / PAGE_SIZE)
        }
    }
    ///Get page offset
    pub fn page_offset(&self) -> usize {
        self.0 & (PAGE_SIZE - 1)
    }
    ///Check page aligned
    pub fn aligned(&self) -> bool {
        self.page_offset() == 0
    }
}
impl From<VirtAddr> for VirtPageNum {
    fn from(v: VirtAddr) -> Self {
        assert_eq!(v.page_offset(), 0);
        v.floor()
    }
}
impl From<VirtPageNum> for VirtAddr {
    fn from(v: VirtPageNum) -> Self {
        Self(v.0 << PAGE_SIZE_BITS)
    }
}
impl PhysAddr {
    ///`PhysAddr`->`PhysPageNum`
    pub fn floor(&self) -> PhysPageNum {
        PhysPageNum(self.0 / PAGE_SIZE)
    }
    ///`PhysAddr`->`PhysPageNum`
    pub fn ceil(&self) -> PhysPageNum {
        if self.0 == 0 {
            PhysPageNum(0)
        } else {
            PhysPageNum((self.0 - 1 + PAGE_SIZE) / PAGE_SIZE)
        }
    }
    ///Get page offset
    pub fn page_offset(&self) -> usize {
        self.0 & (PAGE_SIZE - 1)
    }
    ///Check page aligned
    pub fn aligned(&self) -> bool {
        self.page_offset() == 0
    }
}
impl From<PhysAddr> for PhysPageNum {
    fn from(v: PhysAddr) -> Self {
        assert_eq!(v.page_offset(), 0);
        v.floor()
    }
}
impl From<PhysPageNum> for PhysAddr {
    fn from(v: PhysPageNum) -> Self {
        Self(v.0 << PAGE_SIZE_BITS)
    }
}

impl VirtPageNum {
    ///Return VPN 3 level index
    pub fn indexes(&self) -> [usize; 3] {
        let mut vpn = self.0;
        let mut idx = [0usize; 3];
        for i in (0..3).rev() {
            idx[i] = vpn & 511;
            vpn >>= 9;
        }
        idx
    }
}

impl PhysAddr {
    ///Get reference to `PhysAddr` value
    pub fn get_ref<T>(&self) -> &'static T {
        unsafe {
            ((self.0 + VIRT_ADDR_START) as *const T)
                .as_ref()
                .unwrap()
        }
    }
    ///Get mutable reference to `PhysAddr` value
    pub fn get_mut<T>(&self) -> &'static mut T {
        unsafe { ((self.0 + VIRT_ADDR_START) as *mut T).as_mut().unwrap() }
    }
}
impl PhysPageNum {
    ///Get `PageTableEntry` on `PhysPageNum`
    pub fn get_pte_array(&self) -> &'static mut [PTE; PTES_PER_PAGE] {
        /*let pa: PhysAddr = (*self).into();
        unsafe { core::slice::from_raw_parts_mut(pa.0 as *mut PageTableEntry, 512) }*/
        self.get_mut()
    }
    ///Get u8 array on `PhysPageNum`
    pub fn get_bytes_array(&self) -> &'static mut [u8; PAGE_SIZE] {
        /*let pa: PhysAddr = (*self).into();
        unsafe { core::slice::from_raw_parts_mut(pa.0 as *mut u8, 4096) }*/
        self.get_mut()
    }

    #[allow(missing_docs)]
    pub fn get_bytes_array_phy(&self) -> &'static mut [u8; PAGE_SIZE] {
        self.get_mut_phy()
    }

    #[allow(missing_docs)]
    pub fn get_mut_phy<T>(&self) -> &'static mut T {
        /*let pa: PhysAddr = (*self).into();
        pa.get_mut()*/
        let kva = VirtAddr(self.0 << 12);
        unsafe { (kva.0 as *mut T).as_mut().unwrap() }
    }

    ///Get Get mutable reference to `PhysAddr` value on `PhysPageNum`
    pub fn get_mut<T>(&self) -> &'static mut T {
        /*let pa: PhysAddr = (*self).into();
        pa.get_mut()*/
        /*let satp = riscv::register::satp::read();
        let mmu_enabled = satp.mode() == riscv::register::satp::Mode::Sv39;

        let kva = if !mmu_enabled {
            // 初始化阶段：直接使用物理地址
            self.0<<12
        } else {
            // 正常运行阶段：使用虚拟地址
            (self.0<<12) + KERNEL_SPACE_OFFSET
        };*/
        let kva = VirtAddr((self.0 << 12) + VIRT_ADDR_START);
        //let kva = VirtAddr((self.0<<12) + KERNEL_SPACE_OFFSET);
        unsafe { (kva.0 as *mut T).as_mut().unwrap() }
    }
}
///Add value by one
pub trait StepByOne {
    ///Add value by one
    fn step(&mut self);
}
impl StepByOne for VirtPageNum {
    fn step(&mut self) {
        self.0 += 1;
    }
}
impl StepByOne for PhysPageNum {
    fn step(&mut self) {
        self.0 += 1;
    }
}

#[derive(Copy, Clone)]
/// a simple range structure for type T
pub struct SimpleRange<T>
where
    T: StepByOne + Copy + PartialEq + PartialOrd + Debug,
{
    l: T,
    r: T,
}
impl<T> SimpleRange<T>
where
    T: StepByOne + Copy + PartialEq + PartialOrd + Debug,
{
    ///
    pub fn new(start: T, end: T) -> Self {
        assert!(start <= end, "start {:?} > end {:?}!", start, end);
        Self { l: start, r: end }
    }
    ///
    #[allow(unused)]
    pub fn get_start(&self) -> T {
        self.l
    }
    ///
    #[allow(unused)]
    ///
    pub fn get_end(&self) -> T {
        self.r
    }
}
impl<T> IntoIterator for SimpleRange<T>
where
    T: StepByOne + Copy + PartialEq + PartialOrd + Debug,
{
    type Item = T;
    type IntoIter = SimpleRangeIterator<T>;
    fn into_iter(self) -> Self::IntoIter {
        SimpleRangeIterator::new(self.l, self.r)
    }
}
/// iterator for the simple range structure
pub struct SimpleRangeIterator<T>
where
    T: StepByOne + Copy + PartialEq + PartialOrd + Debug,
{
    current: T,
    end: T,
}
impl<T> SimpleRangeIterator<T>
where
    T: StepByOne + Copy + PartialEq + PartialOrd + Debug,
{
    ///
    pub fn new(l: T, r: T) -> Self {
        Self { current: l, end: r }
    }
}
impl<T> Iterator for SimpleRangeIterator<T>
where
    T: StepByOne + Copy + PartialEq + PartialOrd + Debug,
{
    type Item = T;
    fn next(&mut self) -> Option<Self::Item> {
        if self.current == self.end {
            None
        } else {
            let t = self.current;
            self.current.step();
            Some(t)
        }
    }
}
/// a simple range structure for virtual page number
pub type VPNRange = SimpleRange<VirtPageNum>;
///
pub type VARange = Range<VirtAddr>;
