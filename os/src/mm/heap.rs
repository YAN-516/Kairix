use core::iter::Map;

use log::SetLoggerError;
use lwext4_rust::bindings::EXT4_SUPERBLOCK_FLAGS_SIGNED_HASH;
use virtio_drivers::transport::mmio::VirtIOHeader;
use xmas_elf::sections;

use super::MapPermission;
use super::MapType;
use super::UserMapArea;
use super::UserMapAreaType;
use super::VirtAddr;
use super::vm_area::*;
use super::vm_set::*;
///
pub trait HeapExt {
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
            VirtAddr::from(heap_base.0 + 1),
            MapType::Framed,
            MapPermission::U | MapPermission::R | MapPermission::W,
            UserMapAreaType::Heap,
            true,
        );
        self.insert_user_heap(area, None);
    }

    fn insert_user_heap(&mut self, mut area: UserMapArea, data: Option<&[u8]>) {
        area.map(self.page_table_mut());
        if let Some(data) = data {
            area.copy_data(&self.page_table_mut(), data, 0);
        }
        self.insert_area_sorted(area);
    }

    fn heap_start_va(&self) -> VirtAddr {
        self.areas
            .iter()
            .filter(|area| area.areatype() == UserMapAreaType::Heap)
            .map(|area| area.start_va())
            .min()
            .unwrap()
    }

    fn heap_end_va(&self) -> VirtAddr {
        self.areas
            .iter()
            .filter(|area| area.areatype() == UserMapAreaType::Heap)
            .map(|area| area.end_va())
            .max()
            .unwrap()
    }
    ///仅用于堆
    fn append_to(&mut self, end_va: VirtAddr) {
        let current_end_va = self.heap_end_va();
        if current_end_va > end_va {
            panic!("illegal end_va");
        }
        let area = self.get_heap_area_mut();
        area.range_va_mut().end = VirtAddr::from(end_va.0 + 1);
    }
    ///仅用于堆
    fn shrink_to(&mut self, end_va: VirtAddr) {
        let page_table = &mut self.page_table;

        let area_idx = self
            .areas
            .iter()
            .enumerate()
            .filter(|(_, area)| area.areatype() == UserMapAreaType::Heap)
            .max_by_key(|(_, area)| area.end_va())
            .map(|(idx, _)| idx)
            .unwrap();
        let area = &mut self.areas[area_idx];
        let old_end_va = area.end_va();
        let origin_end_vpn = area.end_vpn();
        if old_end_va < end_va {
            panic!("illegal end_va");
        }
        let new_end_vpn = VirtAddr::from(end_va.0 + 1).ceil();
        let mapped_vpns = area
            .data_frames
            .range(new_end_vpn..origin_end_vpn)
            .map(|(vpn, _)| *vpn)
            .collect::<alloc::vec::Vec<_>>();
        area.range_va_mut().end = VirtAddr::from(end_va.0 + 1);

        for vpn in mapped_vpns {
            // Unpublish and invalidate the mapping while the FrameTracker is
            // still alive.  Dropping it first exposes a recycled frame through
            // the CPU's stale user TLB entry.
            page_table.unmap_page(vpn);
            area.data_frames.remove(&vpn);
        }
    }
}
