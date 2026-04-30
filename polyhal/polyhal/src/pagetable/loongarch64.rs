use loongArch64::register::pgdl;
use alloc::vec::Vec;
use super::{MappingFlags, PageTable, PTE, TLB};
use crate::utils::addr::*;
use loongArch64::register::pgdh;
use loongArch64::register::asid;
use crate::consts::VIRT_ADDR_START;
use core::arch::asm;

impl PTE {
    #[inline]
    pub const fn is_valid(&self) -> bool {
        self.0 != 0
    }

    pub fn ppn(&self) -> PhysPageNum {
        PhysPageNum((self.0 >> 12) & 0xFFFF_FFFF_FF)
    }

    #[inline]
    pub const fn flags(&self) -> PTEFlags {
        PTEFlags::from_bits_truncate(self.0 as u64)
    }

    #[inline]
    pub fn address(&self) -> PhysAddr {
        PhysAddr((self.0) & 0xffff_ffff_f000)
    }

    #[inline]
    pub fn is_table(&self) -> bool {
        self.0 != 0
    }

    #[inline]
    pub(crate) fn new_table(paddr: PhysAddr) -> Self {
        Self(paddr.0)
    }

    #[inline]
    pub fn new(paddr: PhysPageNum, flags: PTEFlags) -> Self {
        Self((paddr.0)<<12 | flags.bits() as usize)
    }

    pub fn readable(&self) -> bool {
        !self.flags().contains(PTEFlags::NR)
    }
    ///Check PTE writable
    pub fn writable(&self) -> bool {
        self.flags().contains(PTEFlags::W)
    }
    ///Check PTE executable
    pub fn executable(&self) -> bool {
        !self.flags().contains(PTEFlags::NX)
    }

    pub fn set_flag(&mut self, flag: PTEFlags){
        self.0 = ((self.0 >> 12) << 12) | flag.bits() as usize;
    }
}

impl From<MappingFlags> for PTEFlags {
    fn from(value: MappingFlags) -> Self {
        let mut flags = PTEFlags::V;
        if value.contains(MappingFlags::W) {
            flags |= PTEFlags::W | PTEFlags::D;
        }

        if !value.contains(MappingFlags::X) {
            flags |= PTEFlags::NX;
        }

        if value.contains(MappingFlags::U) {
            flags |= PTEFlags::PLV_USER;
        }

        if value.contains(MappingFlags::G) {
            flags |= PTEFlags::G;
        }

        if !value.contains(MappingFlags::Cache){
            flags |= PTEFlags::MAT_NOCACHE;
        }
        flags
    }
}

impl From<PTEFlags> for MappingFlags {
    fn from(val: PTEFlags) -> Self {
        let mut flags = MappingFlags::empty() ;
        if val.contains(PTEFlags::W) {
            flags |= MappingFlags::W;
        }

        if val.contains(PTEFlags::D) {
            flags |= MappingFlags::D;
        }

        if !val.contains(PTEFlags::NX) {
            flags |= MappingFlags::X;
        }

        if val.contains(PTEFlags::PLV_USER) {
            flags |= MappingFlags::U;
        }

        if val.contains(PTEFlags::G){
            flags |= MappingFlags::G;
        }

        if !val.contains(PTEFlags::MAT_NOCACHE){
            flags |= MappingFlags::Cache;
        }
        flags
    }
}

bitflags::bitflags! {
    /// Possible flags for a page table entry.
    #[derive(PartialEq, Eq, Clone, Copy, Debug)]
    pub struct PTEFlags: u64 {
        /// Page Valid
        const V = bit!(0);
        /// Dirty, The page has been writed.
        const D = bit!(1);

        const PLV_USER = 0b11 << 2;

        const MAT_NOCACHE = 0b01 << 4;

        /// Designates a global mapping OR Whether the page is huge page.
        const GH = bit!(6);

        /// Page is existing.
        const P = bit!(7);
        /// Page is writeable.
        const W = bit!(8);
        /// Is a Global Page if using huge page(GH bit).
        const G = bit!(10);
        /// Page is not readable.
        const NR = bit!(61);
        /// Page is not executable.
        /// FIXME: Is it just for a huge page?
        /// Linux related url: https://github.com/torvalds/linux/blob/master/arch/loongarch/include/asm/pgtable-bits.h
        const NX = bit!(62);
        /// Whether the privilege Level is restricted. When RPLV is 0, the PTE
        /// can be accessed by any program with privilege Level highter than PLV.
        const RPLV = bit!(63);
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
    pub fn restore(&self) {
        self.release();

        TLB::flush_all();
    }

    #[inline]
    pub fn current() -> Self {
        Self{
            root_ppn: PhysAddr(pgdl::read().base()).floor(),
            frames: Vec::new(),
        }
    }

    #[inline]
    pub fn change(&self) {
        // pgdl::set_base(self.root_ppn.0<<12);
        let root_paddr = self.root_ppn.0<<12;
        unsafe{
            asm!(
                "dbar 0  ",
                "csrwr {root_paddr}, 0x19",
                "invtlb 0x00, $r0, $r0  ",
                root_paddr = in(reg) root_paddr
            )
        }

        TLB::flush_all();
            // let pgdl = loongArch64::register::pgdl::read().base();
            // let pgdh = loongArch64::register::pgdh::read().base();
            // println!("pgdl {:#x} pgdh {:#x} root_paddr {:#x}", pgdl, pgdh, root_paddr);
            // let token = self.token();
            // let is_enabled = root_paddr == pgdl || root_paddr == pgdh;
            // println!("---------is enabled {:?}------------", is_enabled);
    }

    pub fn from_token(root_ppn: usize) -> Self {
        Self{
            root_ppn: PhysPageNum::from(root_ppn),
            frames: Vec::new(),
        }
    }

    
    pub fn token(&self) -> usize {
        self.root_ppn.0 
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
            core::arch::asm!("dbar 0; invtlb 0x05, $r0, {reg}", reg = in(reg) vaddr.0);
        }
    }



    /// flush all tlb entry
    ///
    /// how to use ?
    /// just
    /// TLB::flush_all();
    #[inline]
    pub fn flush_all() {
        unsafe {
            core::arch::asm!("dbar 0; invtlb 0x00, $r0, $r0");
        }
    }
}

pub fn boot_page_table() -> PageTable {
    // FIXME: This should return a valid page table.
    // ref solution: create a blank page table in boot stage.
    PageTable{
        root_ppn: PhysPageNum(0),
        frames: Vec::new(),
    }
}
