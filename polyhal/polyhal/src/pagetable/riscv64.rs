use core::arch::riscv64::sfence_vma;
use alloc::vec::Vec;
use arrayvec::ArrayVec;
use bitflags::bitflags;
use riscv::register::satp::{self, Satp};
use super::{MappingFlags, PageTable, PTE, TLB};
use crate::{PhysAddr, VirtAddr};
use crate::utils::addr::*;
impl PTE {

    #[inline]
    pub(crate) fn address(&self) -> PhysAddr {
        PhysAddr::from((self.0 << 2) & 0xFFFF_FFFF_F000)
    }

    ///Create a PTE from ppn
    pub fn new(ppn: PhysPageNum, flags: PTEFlags) -> Self {
        PTE(
            ppn.0 << 10 | (flags.bits() as usize&0x3ff),
        )
    }

    ///Return 44bit ppn
    pub fn ppn(&self) -> PhysPageNum {
        (self.0 >> 10 & ((1usize << 44) - 1)).into()
    }
    ///
    pub fn set_flag(&mut self, flag: PTEFlags){
        self.0 = ((self.0 >> 10) << 10) | flag.bits() as usize;
    }

    #[inline]
    pub(crate) fn is_table(&self) -> bool {
        return self.flags().contains(PTEFlags::V)
            && !(self.flags().contains(PTEFlags::R)
                || self.flags().contains(PTEFlags::W)
                || self.flags().contains(PTEFlags::X));
    }

    ///Return 10bit flag
    pub fn flags(&self) -> PTEFlags {
        PTEFlags::from_bits_truncate(self.0 as u64)
    }

    ///Check PTE valid
    pub fn is_valid(&self) -> bool {
        (self.flags() & PTEFlags::V) != PTEFlags::empty()
    }
    ///Check PTE readable
    pub fn readable(&self) -> bool {
        (self.flags() & PTEFlags::R) != PTEFlags::empty()
    }
    ///Check PTE writable
    pub fn writable(&self) -> bool {
        (self.flags() & PTEFlags::W) != PTEFlags::empty()
    }
    ///Check PTE executable
    pub fn executable(&self) -> bool {
        (self.flags() & PTEFlags::X) != PTEFlags::empty()
    }
}

bitflags! {
    #[derive(PartialEq, Eq, Clone, Copy, Debug)]
    pub struct PTEFlags: u64 {
        const V = 1 << 0;
        const R = 1 << 1;
        const W = 1 << 2;
        const X = 1 << 3;
        const U = 1 << 4;
        const G = 1 << 5;
        const A = 1 << 6;
        const D = 1 << 7;
    }
}

impl From<MappingFlags> for PTEFlags {
    fn from(flags: MappingFlags) -> Self {
        if flags.is_empty() {
            Self::empty()
        } else {
            let mut res = Self::V;
            if flags.contains(MappingFlags::R) {
                res |= PTEFlags::R | PTEFlags::A;
            }
            if flags.contains(MappingFlags::W) {
                res |= PTEFlags::W | PTEFlags::D;
            }
            if flags.contains(MappingFlags::X) {
                res |= PTEFlags::X;
            }
            if flags.contains(MappingFlags::U) {
                res |= PTEFlags::U;
            }
            res
        }
    }
}

impl From<PTEFlags> for MappingFlags {
    fn from(value: PTEFlags) -> Self {
        let mut mapping_flags = MappingFlags::empty();
        if value.contains(PTEFlags::V) {
            mapping_flags |= MappingFlags::P;
        }
        if value.contains(PTEFlags::R) {
            mapping_flags |= MappingFlags::R;
        }
        if value.contains(PTEFlags::W) {
            mapping_flags |= MappingFlags::W;
        }
        if value.contains(PTEFlags::X) {
            mapping_flags |= MappingFlags::X;
        }
        if value.contains(PTEFlags::U) {
            mapping_flags |= MappingFlags::U;
        }
        if value.contains(PTEFlags::A) {
            mapping_flags |= MappingFlags::A;
        }
        if value.contains(PTEFlags::D) {
            mapping_flags |= MappingFlags::D;
        }

        mapping_flags
    }
}

impl PageTable {
    /// The size of the page for this platform.
    pub const PAGE_SIZE: usize = 0x1000;
    pub const PAGE_LEVEL: usize = 3;
    pub const PTE_NUM_IN_PAGE: usize = 0x200;
    pub(crate) const GLOBAL_ROOT_PTE_RANGE: usize = 0x100;
    pub(crate) const VADDR_BITS: usize = 39;
    pub(crate) const USER_VADDR_END: usize = (1 << Self::VADDR_BITS) - 1;
    pub(crate) const KERNEL_VADDR_START: usize = !Self::USER_VADDR_END;


    #[inline]
    pub fn current() -> Self {
        Self{
            root_ppn: PhysPageNum::from(satp::read().ppn()),
            frames: Vec::new(),
        }
    }

    #[inline]
    pub fn kernel_pte_entry(&self) -> PhysPageNum {
        self.root_ppn
    }

    #[inline]
    pub fn restore(&self) {
        self.release();
        // let kernel_arr = Self::get_pte_list(Self::current().0);
        // let arr = Self::get_pte_list(self.0);
        // arr[0x100..].copy_from_slice(&kernel_arr[0x100..]);
        // arr[0..0x100].fill(PTE(0));
    }

    #[inline]
    pub fn change(&self) {
        // Write page table entry for
        unsafe { satp::write(Satp::from_bits((8 << 60) | usize::from(self.root_ppn))) }
        TLB::flush_all();
    }

        /// Temporarily used to get arguments from user space.
        pub fn from_token(satp: usize) -> Self {
            Self{
                root_ppn: PhysPageNum::from(satp & ((1usize << 44) - 1)),
                frames: Vec::new(),
            }
        }

        
    pub fn token(&self) -> usize {
        8usize << 60 | usize::from(self.root())
    }
}

/// TLB operations
impl TLB {
    /// flush the TLB entry by VirtualAddress
    /// just use it directly
    ///
    /// TLB::flush_vaddr(arg0); // arg0 is the virtual address(VirtAddr)
    #[inline]
    pub fn flush_vaddr(vaddr: VirtAddr) {
        unsafe {
            sfence_vma(vaddr.0, 0);
        }
    }

    /// flush all tlb entry
    ///
    /// how to use ?
    /// just
    /// TLB::flush_all();
    #[inline]
    pub fn flush_all() {
        riscv::asm::sfence_vma_all();
    }
}
