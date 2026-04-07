use core::iter::Map;

use log::SetLoggerError;
use lwext4_rust::bindings::EXT4_SUPERBLOCK_FLAGS_SIGNED_HASH;
use sbi_rt::StartFlags;
use virtio_drivers::transport::mmio::VirtIOHeader;
use xmas_elf::sections;

use super::page_table;
use super::MapPermission;
use super::MapType;
use super::UserMapArea;
use super::vm_set::*;
use super::vm_area::*;
use super::UserMapAreaType;
use super::VirtAddr;
///
pub trait HeapExt{
    ///
    fn alloc_user_heap(&mut self, heap_base: VirtAddr);
    ///
    fn insert_user_heap(&mut self, area: UserMapArea, data: Option<&[u8]>);   
///
    #[allow(unused)]
    ///

    fn heap_start_va(&self) -> VirtAddr;
    ///
    fn heap_end_va(&self) -> VirtAddr;
        ///
        fn append_to(&mut self, end_va: VirtAddr);
        ///
        fn shrink_to(&mut self, end_va: VirtAddr);
}

impl HeapExt for UserVMSet {
    fn alloc_user_heap(&mut self, heap_base: VirtAddr) {
        let area = UserMapArea::new(
            heap_base,
             heap_base, 
             MapType::Framed, 
             MapPermission::U|MapPermission::R|MapPermission::W, 
             UserMapAreaType::Heap, 
             true
            );
        self.insert_user_heap(area, None);
    }
    
    fn insert_user_heap(&mut self, mut area: UserMapArea, data: Option<&[u8]>) {
        area.map(self.page_table_mut());
        if let Some(data) = data {
            area.copy_data(&self.page_table_mut(), data, 0);
        }
        self.areas.push(area);
    }

    fn heap_start_va(&self) -> VirtAddr {
        self.get_heap_area().start_va()
    }

    fn heap_end_va(&self) -> VirtAddr {
        self.get_heap_area().end_va()
    }
    ///仅用于堆
    fn append_to(&mut self, end_va: VirtAddr) {
        let area = self.get_heap_area_mut();
        let current_end_va = area.end_va();
        if current_end_va>end_va{
            panic!("illegal end_va");
        }
        area.range_va_mut().end = end_va;
    }
    ///仅用于堆
    fn shrink_to(&mut self, end_va: VirtAddr) {
        let page_table = &mut self.page_table;

        let areas = &mut self.areas;
        let area = areas.iter_mut().find(|area| area.areatype() == UserMapAreaType::Heap).unwrap();
        let current_end_va = area.end_va();
        let origin_end_vpn = area.end_vpn();
        if current_end_va<end_va{
            panic!("illegal end_va");
        }
        for vpn in current_end_va.ceil()..origin_end_vpn{
            area.data_frames.remove(&vpn);
        }
        area.range_va_mut().end = end_va;

        for vpn in current_end_va.ceil()..origin_end_vpn{
            page_table.unmap(vpn);
        }
    }
}
