use log::error;
use crate::arch::TLB;
use super::address::*;
use alloc::sync::Arc;
use alloc::vec::Vec;
use super::page_table;
use super::page_table::*;
use super::vm_set::AccessType;
use super::frame_alloc;
use crate::mm::{vm_set::*,vm_area::*, LazyAlloc};
use crate::task::task::TaskControlBlock;
use crate::task::*;
// use crate::trap::TrapContext;
///
pub trait AreaPageFaultException{
    ///
    fn handle_cow_fault(&mut self, vpn: VirtPageNum) -> Option<PhysPageNum>;
}
///
pub trait SetPageFaultException {
    ///
    fn handle_store_page_fault_set(&mut self, va: VirtAddr, accsess: AccessType) -> Option<()>;
    ///
    fn handle_cow_page_fault(&mut self, va: VirtAddr) -> Option<()>;
    ///
    fn handle_unalloc_page_fault(&mut self, va: VirtAddr) -> Option<()>;
}

impl SetPageFaultException for UserVMSet {
    fn handle_unalloc_page_fault(&mut self, va: VirtAddr) -> Option<()>{
        println!("unalloc handler");

        let area = self.find_area(va).unwrap();
        let pte_flags: PTEFlags;
        match area.areatype() {
            UserMapAreaType::Heap | UserMapAreaType::Stack => {
                // if !area.get_lazy_flag(){
                //     return None;
                // }
                for vpn in area.vpn_range() {
                    let frame = frame_alloc().unwrap();
                    area.data_frames.insert(vpn, Arc::new(frame));
                }
                area.clear_lazy_flag();
            }
            _ => return None,
        }
        pte_flags = PTEFlags::from_bits(area.perm().bits()).unwrap();
        let frames = area.data_frames.clone();
        for (vpn, frame) in frames {
            self.page_table.map(vpn, frame.ppn, pte_flags);
        }
        Some(())
    }

    fn handle_cow_page_fault(&mut self, va: VirtAddr) -> Option<()>{
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

        let flags = PTEFlags::from_bits(area.perm().bits()).unwrap() | PTEFlags::V;
        let page_table = self.page_table_mut();
        for (ppn, vpn) in ppns {
            //处理pte
            if let Some(pte) = page_table.find_pte(vpn) {
                if ppn != PhysPageNum(0) {
                    //分配了新页
                    let new_pte = PageTableEntry::new(ppn, flags);
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
        TLB::flush_vaddr(va.into());
        Some(())
        // polyhal::pagetable::TLB::flush_vaddr(va.into());
    }

    fn handle_store_page_fault_set(&mut self, va: VirtAddr, access: AccessType) -> Option<()> {
        // println!(
        //     "enter page fault handler, va = {:#x}, access type = {:?}",
        //     va.0, access
        // );
        let exceptiontype: ExceptionType;
        if let Some(area) = self.find_area(va) {
            exceptiontype = area.access_check(access);
        } else {
            error!("no vma found");
            return None;
        }
        match exceptiontype {
            ExceptionType::Cow => {self.handle_cow_page_fault(va);
            Some(())},
            ExceptionType::Write => {
                self.handle_unalloc_page_fault(va);
                Some(())
            }
            ExceptionType::Read => {
                self.handle_unalloc_page_fault(va);
                Some(())
            }
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

impl AreaPageFaultException for UserMapArea {
    ///VMA处理，权限恢复，返回新分配物理页的ppn
    fn handle_cow_fault(&mut self, vpn: VirtPageNum) -> Option<PhysPageNum> {
        let frame = self.data_frames.get(&vpn).unwrap();
        if Arc::strong_count(frame) == 1 {
            self.clear_cow_flag();
            self.perm_mut().insert(MapPermission::W);
            // sfence_vma_va(vpn.into());
            TLB::flush_vaddr(vpn.into());
            None
        } else {
            let new_frame = Arc::new(frame_alloc().unwrap());
            let ppn = new_frame.ppn;
            ppn.get_bytes_array()
                .copy_from_slice(frame.ppn.get_bytes_array());
            *self.data_frames.get_mut(&vpn).unwrap() = new_frame;
            self.perm_mut().insert(MapPermission::W);
            self.clear_cow_flag();
            // sfence_vma_va(vpn.into());
            TLB::flush_vaddr(vpn.into());

            Some(ppn)
        }
    }
}
