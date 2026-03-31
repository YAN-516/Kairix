// use super::page_table;
// use super::page_table::PTEFlags;
use super::vm_area::{KernelMapArea, MapType, UserMapArea};
use super::{
    COW, UserMapAreaType,
    exception::{self, *},
    vm_area,
};
use super::{LazyAlloc, frame_alloc};
use super::{MapPermission,vm_area::MapArea};
use crate::config::{
    KERNEL_SPACE_OFFSET, KERNEL_STACK_SIZE, MEMORY_END, MMIO, TRAP_CONTEXT,
    USER_MEMORY_SPACE, USER_STACK_BASE, USER_STACK_SIZE,
};
use crate::mm::vm_area::KernelAreaType;
use crate::sync::UPSafeCell;
use alloc::collections::btree_map::Range;
use alloc::sync::Arc;
use alloc::vec::Vec;
use bitflags::Flags;
use polyhal::consts::VIRT_ADDR_START;
use core::arch::{self, asm};
use core::cell::RefCell;
use core::iter::Map;
use core::ops::{Deref, DerefMut};
use core::task;
use lazy_static::*;
use log::error;
use riscv::addr::{Page, page};
// use riscv::paging::PTE;
use riscv::register::satp;
pub use polyhal::utils::addr::*;
pub use polyhal::pagetable::*;

// use crate::arch::riscv::sfence_vma_va;
// use crate::arch::TLB;
use crate::task::{current_task, exit_current_and_run_next};
use crate::task::task::TaskControlBlock;
// use crate::trap::self;
use lazy_static::*;
use sbi_rt::Sta;

unsafe extern "C" {
    safe fn stext();
    safe fn etext();
    safe fn srodata();
    safe fn erodata();
    safe fn sdata();
    safe fn edata();
    safe fn sbss_with_stack();
    safe fn ebss();
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

impl<A: MapArea> VMSpace for VMSet<A> {
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
        let satp = self.page_table.token();
        unsafe {
            satp::write(satp);
            asm!("sfence.vma");
        }
    }
}
#[allow(missing_docs)]
pub struct UserVMSet {
    pub inner: VMSet<UserMapArea>,
}

impl Deref for UserVMSet {
    type Target = VMSet<UserMapArea>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for UserVMSet {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl UserVMSet {
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
    ) {
        match area_type {
            UserMapAreaType::Stack | UserMapAreaType::Heap => self.push(
                UserMapArea::new(
                    start_va,
                    end_va,
                    MapType::Framed,
                    permission,
                    area_type,
                    true,
                ),
                None,
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
            inner: VMSet {
                page_table: page_table,
                areas: Vec::new(),
            },
        }
    }

    fn push(&mut self, mut map_area: UserMapArea, data: Option<&[u8]>) {
        map_area.map(&mut self.page_table);

        if let Some(data) = data {
            map_area.copy_data(&self.page_table, data);
        }


        self.areas.push(map_area);
    }

    /// Include sections in elf and trampoline and TrapContext and user stack,
    /// also returns user_sp and entry point.
    pub fn from_elf(elf_data: &[u8]) -> (Self, usize, usize) {
        let mut vmset = Self::from_kernel(&KERNEL_VMSET.exclusive_access());
        // map program headers of elf, with U flag
        let elf = xmas_elf::ElfFile::new(elf_data).unwrap();
        let elf_header = elf.header;
        let magic = elf_header.pt1.magic;
        assert_eq!(magic, [0x7f, 0x45, 0x4c, 0x46], "invalid elf!");
        let ph_count = elf_header.pt2.ph_count();
        //let mut max_end_vpn = VirtPageNum(0);
        for i in 0..ph_count {
            let ph = elf.program_header(i).unwrap();
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
                //max_end_vpn = map_area.end_vpn();
                // error!("data {}, {}",ph.offset() as usize, (ph.offset() + ph.file_size())as usize);
                vmset.push(
                    map_area,
                    Some(&elf.input[ph.offset() as usize..(ph.offset() + ph.file_size()) as usize]),
                );
            }
        }

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
        error!("from_elf over");
        (vmset, user_stack_top, elf.header.pt2.entry_point() as usize)
    }

    #[allow(missing_docs)]
    pub fn from_existed_user(user_vmset: &UserVMSet) -> Self {
        let mut vmset = Self::from_kernel(&KERNEL_VMSET.exclusive_access());

        // copy data sections/trap_context/user_stack
        for area in user_vmset.areas.iter() {
            let new_area = UserMapArea::from_another(area);
            vmset.push(new_area, None);
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
                vmset.push(new_area, None);
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
                vmset.push(new_area, None);
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
}
///
pub struct KernelVMSet {
    inner: VMSet<KernelMapArea>,
}
impl Deref for KernelVMSet {
    type Target = VMSet<KernelMapArea>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for KernelVMSet {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl KernelVMSet {
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
            map_area.copy_data(&self.page_table, data);
        }

        self.areas.push(map_area);
    }
    ///
    pub fn new() -> Self {
        let mut kvm_set = Self {
            inner: VMSet::new_bare(),
        };
        // map kernel sections

        println!("map kernel sections");
        println!(".text [{:#x}, {:#x})", stext as usize, etext as usize);
        println!(".rodata [{:#x}, {:#x})", srodata as usize, erodata as usize);
        println!(".data [{:#x}, {:#x})", sdata as usize, edata as usize);
        println!(
            ".bss [{:#x}, {:#x})",
            sbss_with_stack as usize, ebss as usize
        );
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
        println!("mapping .bss section");
        kvm_set.push(
            KernelMapArea::new(
                (sbss_with_stack as usize).into(),
                (ebss as usize).into(),
                MapType::Identical,
                MapPermission::R | MapPermission::W,
                KernelAreaType::Bss,
            ),
            None,
        );
        println!("mapping physical memory");
        println!("start_va {:#x}, end_va {:#x}", ekernel as usize, MEMORY_END + VIRT_ADDR_START );
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
            error!("start_va {:#x} end_va {:#x}", (*pair).0, (*pair).0 + (*pair).1);
            kvm_set.push(
                KernelMapArea::new(
                    ((*pair).0 + VIRT_ADDR_START).into(),
                    (((*pair).0 + (*pair).1) + VIRT_ADDR_START).into(),
                    MapType::Identical,
                    MapPermission::R | MapPermission::W,
                    KernelAreaType::MemMappedReg,
                ),
                None,
            );
        }
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
