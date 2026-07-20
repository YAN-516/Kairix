#![allow(missing_docs)]

use crate::mm::vm_area::UserMapAreaType;
use crate::mm::{MapArea, MapPermission, MmapType};
use crate::task::current_process;
use alloc::format;
use alloc::string::String;

/// Generate the seven page-count fields documented by proc_pid_statm(5):
/// size, resident, shared, text, library, data+stack, dirty.
pub fn content() -> String {
    let process = current_process();
    let inner = process.inner_exclusive_access();
    let mut size = 0usize;
    let mut resident = 0usize;
    let mut shared = 0usize;
    let mut text = 0usize;
    let mut data = 0usize;

    for area in inner.vm_set.areas.iter() {
        let pages = area.end_vpn().0.saturating_sub(area.start_vpn().0);
        let resident_pages = area.data_frames.len();
        let is_shared = area.areatype() == UserMapAreaType::Shm
            || (area.areatype() == UserMapAreaType::Mmap && area.flags == MmapType::MapShared)
            || area.map_file.is_some();

        size = size.saturating_add(pages);
        resident = resident.saturating_add(resident_pages);
        if is_shared {
            shared = shared.saturating_add(resident_pages);
        }
        if area.perm().contains(MapPermission::X) {
            text = text.saturating_add(pages);
        }
        if area.perm().contains(MapPermission::W) && !is_shared {
            data = data.saturating_add(pages);
        }
    }

    format!("{} {} {} {} 0 {} 0\n", size, resident, shared, text, data)
}
