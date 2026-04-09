use log::warn;
extern crate alloc;
use alloc::vec::Vec;
use alloc::vec;
use bitflags::*;
cfg_if::cfg_if! {
    if #[cfg(target_arch = "loongarch64")] {
        mod loongarch64;
        pub use loongarch64::*;
    } else if #[cfg(target_arch = "aarch64")] {
        mod aarch64;
        pub use aarch64::*;
    } else if #[cfg(target_arch = "riscv64")] {
        mod riscv64;
        pub use riscv64::*;

    } else if #[cfg(target_arch = "x86_64")] {
        mod x86_64;
        pub use x86_64::*;
    } else {
        compile_error!("unsupported architecture!");
    }
}

use core::ops::Deref;

use crate::{common::FrameTracker, components::common::frame_alloc, utils::addr::PhysPageNum, PhysAddr, VirtAddr};

use super::common::frame_dealloc;
use crate::utils::addr::*;
/// The size of the page table.
pub const PAGE_SIZE: usize = PageTable::PAGE_SIZE;

/// Page table entry structure
///
/// Just define here. Should implement functions in specific architectures.
#[derive(Copy, Clone, Debug)]
pub struct PTE(pub usize);

impl PTE {
    pub const fn empty() -> Self {
        Self(0)
    }
}

/// Page Table
///
/// This is just the page table defination.
/// The implementation of the page table in the specific architecture mod.
/// Such as:
/// x86_64/page_table.rs
/// riscv64/page_table/sv39.rs
/// aarch64/page_table.rs
/// loongarch64/page_table.rs
#[repr(C)]
// #[derive(Debug, Clone, Copy)]
pub struct PageTable{
    pub root_ppn: PhysPageNum,
    pub frames: Vec<FrameTracker>
}

impl PageTable {
    fn find_pte_create(&mut self, vpn: VirtPageNum) -> Option<&mut PTE> {

        let idxs = vpn.indexes();
        let mut ppn = self.root_ppn;
        let mut result: Option<&mut PTE> = None;
        for (i, idx) in idxs.iter().enumerate() {
            let pte = &mut ppn.get_pte_array()[*idx];
            if i == 2 {
                result = Some(pte);
                break;
            }
            if !pte.is_valid() {
                let frame = frame_alloc().unwrap();
                *pte = PTE::new(frame.ppn, PTEFlags::V);
                self.frames.push(frame);
            }
            ppn = pte.ppn();
        }
        result
    }

    pub fn find_pte(&self, vpn: VirtPageNum) -> Option<&mut PTE> {
        let idxs = vpn.indexes();
        let mut ppn = self.root();
        let mut result: Option<&mut PTE> = None;
        for (i, idx) in idxs.iter().enumerate() {
            let pte = &mut ppn.get_pte_array()[*idx];
            if i == 2 {
                result = Some(pte);
                break;
            }
            if !pte.is_valid() {
                return None;
            }
            ppn = pte.ppn();
        }
        result
    }

    /// Get the root Physical Page
    pub const fn root(&self) -> PhysPageNum {
        self.root_ppn
    }
    /// Get the page table list through the physical address
    #[inline]
    pub(crate) fn get_pte_list(paddr: PhysAddr) -> &'static mut [PTE] {
        paddr.floor().get_pte_array()
    }

    /// Mapping a page to specific virtual page (user space address).
    ///
    /// Ensure that PageTable is which you want to map.
    /// vpn: Virtual page will be mapped.
    /// ppn: Physical page.
    /// flags: Mapping flags, include Read, Write, Execute and so on.
    /// size: MappingSize. Just support 4KB page currently.
    pub fn map_page(
        &mut self,
        vpn: VirtPageNum,
        ppn: PhysPageNum,
        flags: MappingFlags,
        _size: MappingSize,
    ) {
        let pte = self.find_pte_create(vpn).unwrap();
        // error!("{:#x}", vpn.0);
        // warn!("map vpn {:#x}", vpn.0);
        assert!(!pte.is_valid(), "vpn {:?} is mapped before mapping", vpn);
        // println!("mapping {:#x} to {:#x}", vpn.0, ppn.0);
        *pte = PTE::new(ppn, flags.into());
        TLB::flush_vaddr(vpn.into());
    }

    /// Mapping a page to specific address(kernel space address).
    ///
    /// TODO: This method is not implemented.
    /// TIPS: If we mapped to kernel, the page will be shared between different pagetable.
    ///
    /// Ensure that PageTable is which you want to map.
    /// vpn: Virtual page will be mapped.
    /// ppn: Physical page.
    /// flags: Mapping flags, include Read, Write, Execute and so on.
    /// size: MappingSize. Just support 4KB page currently.    
    ///
    /// How to implement shared.
    pub fn map_kernel(
        &mut self,
        vpn: VirtPageNum,
        ppn: PhysPageNum,
        flags: MappingFlags,
        _size: MappingSize,
    ) {
        let pte = self.find_pte_create(vpn).unwrap();
        // warn!("map vpn {}", vpn.0);
        assert!(!pte.is_valid(), "vpn {:?} is mapped before mapping", vpn);
        *pte = PTE::new(ppn, flags.into());
        TLB::flush_vaddr(vpn.into());
    }

    /// Unmap a page from specific virtual page (user space address).
    ///
    /// Ensure the virtual page is exists.
    /// vpn: Virtual address.
    pub fn unmap_page(&self, vpn: VirtPageNum) {
        let pte = self.find_pte(vpn).unwrap();
        assert!(pte.is_valid(), "vpn {:?} is invalid before unmapping", vpn);
        *pte = PTE::empty();
    }

    /// Translate a virtual adress to a physical address and mapping flags.
    ///
    /// Return None if the vaddr isn't mapped.
    /// vpn: The virtual address will be translated.
    pub fn translate(&self, vpn: VirtPageNum) -> Option<PTE> {
        self.find_pte(vpn).map(|pte| *pte)
    }

    pub fn translate_va(&self, va: VirtAddr) -> Option<PhysAddr> {
        self.find_pte(va.clone().floor()).map(|pte| {
            let aligned_pa: PhysAddr = pte.ppn().into();
            let offset = va.page_offset();
            let aligned_pa_usize: usize = aligned_pa.into();
            (aligned_pa_usize + offset).into()
        })
    }


    pub fn new() -> Self {
        let frame = frame_alloc().unwrap();
        // println!("new pagetable{:#x}",frame.ppn.0);
        PageTable{
            root_ppn: frame.ppn,
            frames: vec![frame],
        }
    }

    /// Release the page table entry.
    ///
    /// The page table entry in the user space address will be released.
    /// [Page Table Wikipedia](https://en.wikipedia.org/wiki/Page_table).
    /// You don't need to care about this if you just want to use.
    pub fn release(&self) {
        let drop_l2 = |pte_list: &[PTE]| {
            pte_list.iter().for_each(|x| {
                if x.is_table() {
                    frame_dealloc(x.address().into());
                }
            });
        };
        let drop_l3 = |pte_list: &[PTE]| {
            pte_list.iter().for_each(|x| {
                if x.is_table() {
                    drop_l2(Self::get_pte_list(x.address()));
                    frame_dealloc(x.address().into());
                }
            });
        };
        let drop_l4 = |pte_list: &[PTE]| {
            pte_list.iter().for_each(|x| {
                if x.is_table() {
                    drop_l3(Self::get_pte_list(x.address()));
                    frame_dealloc(x.address().into());
                }
            });
        };

        // Drop all sub page table entry and clear root page.
        let pte_list = &mut Self::get_pte_list(self.root().into())[..Self::GLOBAL_ROOT_PTE_RANGE];
        if Self::PAGE_LEVEL == 4 {
            drop_l4(pte_list);
        } else {
            drop_l3(pte_list);
        }
        pte_list.fill(PTE(0));
    }
}

bitflags::bitflags! {
    /// Mapping flags for page table.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct MappingFlags: u64 {
        /// Persent
        const P = bit!(0);
        /// User Accessable Flag
        const U = bit!(1);
        /// Readable Flag
        const R = bit!(2);
        /// Writeable Flag
        const W = bit!(3);
        /// Executeable Flag
        const X = bit!(4);
        /// Accessed Flag
        const A = bit!(5);
        /// Dirty Flag, indicating that the page was written
        const D = bit!(6);
        /// Global Flag
        const G = bit!(7);
        /// Device Flag, indicating that the page was used for device memory
        const Device = bit!(8);
        /// Cache Flag, indicating that the page will be cached
        const Cache = bit!(9);

        /// Read | Write | Executeable Flags
        const RWX = Self::R.bits() | Self::W.bits() | Self::X.bits();
        /// User | Read | Write Flags
        const URW = Self::U.bits() | Self::R.bits() | Self::W.bits();
        /// User | Read | Executeable Flags
        const URX = Self::U.bits() | Self::R.bits() | Self::X.bits();
        /// User | Read | Write | Executeable Flags
        const URWX = Self::URW.bits() | Self::X.bits();
    }
}


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

impl From<MapPermission> for MappingFlags {
    fn from(perm: MapPermission) -> Self {
        let mut flags = MappingFlags::empty();
        if perm.contains(MapPermission::R) {
            flags |= MappingFlags::R;
        }
        if perm.contains(MapPermission::W) {
            flags |= MappingFlags::W;
        }
        if perm.contains(MapPermission::X) {
            flags |= MappingFlags::X;
        }
        if perm.contains(MapPermission::U) {
            flags |= MappingFlags::U;
        }
        if perm.contains(MapPermission::G) {
            flags |= MappingFlags::G;
        }
        if !perm.contains(MapPermission::MAT_NOCACHE) {
            flags |= MappingFlags::Cache;
        }
        flags
    }
}


/// This structure indicates size of the page that will be mapped.
///
/// TODO: Support More Page Size, 16KB or 32KB
/// Just support 4KB right now.
#[derive(Debug)]
pub enum MappingSize {
    Page4KB,
    // Page2MB,
    // Page1GB,
}

/// TLB Operation set.
/// Such as flush_vaddr, flush_all.
/// Just use it in the fn.
///
/// there are some methods in the TLB implementation
///
/// ### Flush the tlb entry through the specific virtual address
///
/// ```rust
/// TLB::flush_vaddr(arg0);  arg0 should be VirtAddr
/// ```
/// ### Flush all tlb entries
/// ```rust
/// TLB::flush_all();
/// ```
pub struct TLB;