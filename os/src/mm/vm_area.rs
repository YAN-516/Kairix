use alloc::borrow::ToOwned;
use alloc::sync::Arc;
use bitflags::Flag;
use core::ops::{BitAnd, BitOr, BitXor, Not, Range};
use core::{error, fmt};
use log::{SetLoggerError, error, info, warn};
use polyhal::consts::VIRT_ADDR_START;

#[cfg(target_arch = "riscv64")]
use riscv::register::mcause::Exception;
#[cfg(target_arch = "riscv64")]
use sbi_rt::StartFlags;

use super::vm_set::{AccessType, ExceptionType};
use super::{exception::*, frame_alloc, frame_allocator};
use crate::fs::File;
use crate::sync::SpinNoIrqLock;
use xmas_elf::sections;
// use super::{
//     PTEFlags, PageTable, PageTableEntry,
// };
use alloc::vec::Vec;
use polyhal::common::FrameTracker;
use polyhal::consts::*;
pub use polyhal::pagetable::*;
pub use polyhal::utils::addr::*;

// use crate::arch::riscv::sfence_vma_va;
// use crate::config::{KERNEL_SPACE_OFFSET, PAGE_SIZE};
use alloc::collections::BTreeMap;
// use crate::arch::TLB;

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

    fn start_va(&self) -> VirtAddr {
        self.range_va().start
    }
    fn end_va(&self) -> VirtAddr {
        self.range_va().end
    }

    fn vpn_range(&self) -> Range<VirtPageNum> {
        self.start_vpn()..self.end_vpn()
    }
    fn start_vpn(&self) -> VirtPageNum {
        self.start_va().floor()
    }
    fn end_vpn(&self) -> VirtPageNum {
        self.end_va().ceil()
    }
    fn perm(&self) -> &MapPermission;
    fn perm_mut(&mut self) -> &mut MapPermission;

    fn map_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum);
    fn unmap_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum);
    fn map(&mut self, page_table: &mut PageTable);
    fn unmap(&mut self, page_table: &mut PageTable);

    // fn copy_data(&mut self, page_table: &PageTable, data: &[u8]) {
    //     //assert_eq!(self.map_type, MapType::Framed);
    //     let mut start: usize = 0;
    //     let mut current_vpn = self.start_vpn();
    //     let len = data.len();
    //     loop {
    //         let src = &data[start..len.min(start + PAGE_SIZE)];
    //         let dst = &mut page_table
    //             .translate(current_vpn)
    //             .unwrap()
    //             .ppn()
    //             .get_bytes_array()[..src.len()];
    //         dst.copy_from_slice(src);
    //         start += PAGE_SIZE;
    //         if start >= len {
    //             break;
    //         }
    //         current_vpn.step();
    //     }
    // }
    //按照传入的虚拟地址和数据，进行跨页复制，之前是忽略起始的offset，这里进行了debug修复
    fn copy_data(&mut self, page_table: &PageTable, data: &[u8], mut exact_start_va: usize) {
        info!("copy data");
        let mut offset = 0;
        while offset < data.len() {
            let page_offset = exact_start_va % PAGE_SIZE;
            let write_len = (PAGE_SIZE - page_offset).min(data.len() - offset);
            let ppn = page_table
                .translate(VirtAddr::from(exact_start_va).floor())
                .unwrap()
                .ppn();
            let dst_ptr = ((ppn.0 << 12) + page_offset + VIRT_ADDR_START) as *mut u8;
            // let dst_ptr = (exact_start_va + VIRT_ADDR_START) as *mut u8;
            let dst_slice = unsafe { core::slice::from_raw_parts_mut(dst_ptr, write_len) };
            let src_slice = &data[offset..offset + write_len];
            dst_slice.copy_from_slice(src_slice);
            exact_start_va += write_len;
            offset += write_len;
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
///
pub enum UserMapAreaType {
    ///
    Elf,
    ///
    Stack,
    ///
    Heap,
    ///
    TrapContext,
    ///
    RtSigreturnTrampoline,
    ///
    Mmap,
    ///
    Shm,
}
#[derive(Clone, Copy, PartialEq, Eq)]
///
pub enum MmapType {
    ///共享映射
    MapShared,
    ///私有映射
    MapPrivate,
}

/// Lazily allocated pages belonging to one anonymous `MAP_SHARED` mapping.
///
/// Each process keeps its own page table and `data_frames`, but mappings
/// inherited across `fork` share this object. Consequently, a page first
/// faulted after `fork` is still backed by the same physical frame in every
/// process, as required by Linux `MAP_SHARED` semantics.
pub struct SharedAnonymousFrames {
    frames: SpinNoIrqLock<Vec<(usize, Arc<FrameTracker>)>>,
}

impl SharedAnonymousFrames {
    fn new() -> Self {
        Self {
            frames: SpinNoIrqLock::new(Vec::new()),
        }
    }

    fn get(&self, page_index: usize) -> Option<Arc<FrameTracker>> {
        let frames = self.frames.lock();
        let index = frames
            .binary_search_by_key(&page_index, |(index, _)| *index)
            .ok()?;
        Some(frames[index].1.clone())
    }

    fn install_or_get(
        &self,
        page_index: usize,
        candidate: Arc<FrameTracker>,
    ) -> Option<Arc<FrameTracker>> {
        // Keep the index sparse: very large lazy mappings must not allocate
        // metadata for every virtual page at mmap time. Capacity growth is
        // prepared outside the shared lock, then installed without allocating
        // while the lock is held.
        let mut replacement = Vec::new();
        loop {
            let mut frames = self.frames.lock();
            match frames.binary_search_by_key(&page_index, |(index, _)| *index) {
                Ok(index) => return Some(frames[index].1.clone()),
                Err(index) if frames.len() < frames.capacity() => {
                    frames.insert(index, (page_index, candidate.clone()));
                    return Some(candidate);
                }
                Err(_) => {
                    let required = frames.len().checked_add(1)?;
                    let target_capacity = required.max(frames.capacity().saturating_mul(2)).max(4);
                    drop(frames);
                    if replacement.capacity() < target_capacity {
                        replacement.try_reserve_exact(target_capacity).ok()?;
                    }

                    let mut frames = self.frames.lock();
                    match frames.binary_search_by_key(&page_index, |(index, _)| *index) {
                        Ok(index) => return Some(frames[index].1.clone()),
                        Err(_) if frames.len() < frames.capacity() => continue,
                        Err(_) if replacement.capacity() <= frames.len() => continue,
                        Err(index) => {
                            core::mem::swap(&mut *frames, &mut replacement);
                            frames.extend(replacement.drain(..));
                            frames.insert(index, (page_index, candidate.clone()));
                            return Some(candidate);
                        }
                    }
                }
            }
        }
    }
}

///
pub trait LazyAlloc {
    ///
    fn get_lazy_flag(&self) -> bool;
    ///
    fn set_lazy_flag(&mut self);
    ///
    fn clear_lazy_flag(&mut self);
}
#[allow(missing_docs)]
pub struct UserMapArea {
    pub va_range: VARange,
    pub data_frames: BTreeMap<VirtPageNum, Arc<FrameTracker>>,
    pub map_type: MapType,
    pub map_perm: MapPermission,
    pub area_type: UserMapAreaType,
    pub cow_flag: bool,
    pub lazy_flag: bool,
    pub growdown_flag: bool,             // MAP_GROWSDOWN 标志，用于栈向下扩展
    pub map_file: Option<Arc<dyn File>>, // 绑定的文件，匿名映射就是 None
    /// Resolved backing path captured when the mapping is installed.
    /// Stall diagnostics can read this without entering the file's inner lock.
    pub mapping_path: Option<Arc<str>>,
    pub file_offset: usize, // 映射从文件的哪个字节开始
    /// First virtual byte that is zero-filled instead of read from `map_file`.
    /// This is used by ELF PT_LOAD mappings to represent the file/BSS boundary.
    /// Ordinary mmap regions leave it as `None` and retain SIGBUS semantics
    /// beyond the underlying file size.
    pub file_zero_start: Option<usize>,
    pub flags: MmapType,      // mmap 的 flags，比如 MAP_SHARED 还是 MAP_PRIVATE
    pub shmid: Option<usize>, // SysV 共享内存标识符（若非共享内存则为 None）
    /// Shared lazy backing for anonymous `MAP_SHARED` mappings.
    pub shared_anonymous: Option<Arc<SharedAnonymousFrames>>,
    /// Page offset into `shared_anonymous` after VMA prefix splits.
    pub shared_anonymous_offset: usize,
}

/// Build temporary read-only leaf PTE flags for copy-on-write mappings.
pub fn cow_mapping_flags(map_perm: MapPermission) -> MappingFlags {
    let mut flags = MappingFlags::from(map_perm);
    flags.remove(MappingFlags::W);
    if !flags.contains(MappingFlags::R) {
        flags.insert(MappingFlags::R);
    }
    flags
}

impl LazyAlloc for UserMapArea {
    fn clear_lazy_flag(&mut self) {
        self.lazy_flag = false;
    }
    fn get_lazy_flag(&self) -> bool {
        self.lazy_flag
    }
    fn set_lazy_flag(&mut self) {
        self.lazy_flag = true;
    }
}

impl Drop for UserMapArea {
    fn drop(&mut self) {
        if !self.data_frames.is_empty() {
            warn!(
                "[MEMDEBUG] UserMapArea dropped with {} remaining frames, type={:?}, range={:#x}..{:#x}",
                self.data_frames.len(),
                self.area_type,
                self.start_va().0,
                self.end_va().0
            );
        }
    }
}

#[allow(unused)]
#[allow(missing_docs)]
impl UserMapArea {
    /// Whether writable file-backed MAP_SHARED pages need a write fault before
    /// userspace may modify the page-cache frame.
    pub fn tracks_shared_file_dirty(&self) -> bool {
        self.area_type == UserMapAreaType::Mmap
            && self.flags == MmapType::MapShared
            && self.map_file.is_some()
            && self.map_perm.contains(MapPermission::W)
    }

    /// PTE permissions used before a shared file page has been dirtied.
    ///
    /// A writable MAP_SHARED VMA keeps W in its Linux-visible VMA permissions,
    /// but its initial PTE is read-only. The first store therefore reaches the
    /// kernel, which marks the underlying page-cache page dirty before granting
    /// write access.
    pub fn initial_mapping_flags(&self) -> MappingFlags {
        let mut flags = MappingFlags::from(self.map_perm);
        if self.tracks_shared_file_dirty() {
            flags.remove(MappingFlags::W);
        }
        flags
    }

    pub fn expand(&mut self, end_va: VirtAddr) {
        self.va_range.end = end_va
    }
    pub fn access_check(&self, access: AccessType) -> ExceptionType {
        match access {
            AccessType::Read => {
                if self.perm().contains(MapPermission::R) {
                    ExceptionType::Read
                } else {
                    ExceptionType::None
                }
            }
            AccessType::Write => {
                if self.cow_flag {
                    ExceptionType::Cow
                } else if self.perm().contains(MapPermission::W) {
                    ExceptionType::Write
                } else {
                    ExceptionType::None
                }
            }
            AccessType::Execute => {
                if self.perm().contains(MapPermission::X) {
                    ExceptionType::Execute
                } else {
                    ExceptionType::None
                }
            }
            _ => ExceptionType::None,
        }
    }

    pub fn new(
        start_va: VirtAddr,
        end_va: VirtAddr,
        map_type: MapType,
        map_perm: MapPermission,
        area_type: UserMapAreaType,
        lazy_flag: bool,
    ) -> Self {
        Self {
            va_range: start_va..end_va,
            data_frames: BTreeMap::new(),
            map_type: map_type,
            map_perm: map_perm,
            area_type,
            cow_flag: false,
            lazy_flag,
            growdown_flag: false,
            map_file: None,
            mapping_path: None,
            file_offset: 0,
            file_zero_start: None,
            flags: MmapType::MapPrivate,
            shmid: None,
            shared_anonymous: None,
            shared_anonymous_offset: 0,
        }
    }
    pub fn with_frames(
        start_va: VirtAddr,
        end_va: VirtAddr,
        map_type: MapType,
        map_perm: MapPermission,
        area_type: UserMapAreaType,
        data_frames: BTreeMap<VirtPageNum, Arc<FrameTracker>>,
    ) -> Self {
        Self {
            va_range: start_va..end_va,
            data_frames,
            map_type,
            map_perm,
            area_type,
            cow_flag: false,
            lazy_flag: false,
            growdown_flag: false,
            map_file: None,
            mapping_path: None,
            file_offset: 0,
            file_zero_start: None,
            flags: MmapType::MapPrivate,
            shmid: None,
            shared_anonymous: None,
            shared_anonymous_offset: 0,
        }
    }
    pub fn areatype(&self) -> UserMapAreaType {
        self.area_type
    }
    /// Copy the VMA metadata without cloning its resident-frame index.
    ///
    /// Range splitting can then move the existing `BTreeMap` nodes into the
    /// resulting VMAs instead of cloning every `Arc<FrameTracker>` only to
    /// discard most of the clones with `retain`.
    pub(crate) fn metadata_from_another(another: &UserMapArea) -> Self {
        Self {
            va_range: another.start_va()..another.end_va(),
            data_frames: BTreeMap::new(),
            map_type: another.map_type,
            map_perm: another.map_perm,
            area_type: another.area_type,
            cow_flag: another.cow_flag,
            lazy_flag: another.lazy_flag,
            growdown_flag: another.growdown_flag,
            map_file: another.map_file.clone(),
            mapping_path: another.mapping_path.clone(),
            file_offset: another.file_offset,
            file_zero_start: another.file_zero_start,
            flags: another.flags,
            shmid: another.shmid,
            shared_anonymous: another.shared_anonymous.clone(),
            shared_anonymous_offset: another.shared_anonymous_offset,
        }
    }

    pub fn from_another(another: &UserMapArea) -> Self {
        let mut cloned = Self::metadata_from_another(another);
        cloned.data_frames = another.data_frames.clone();
        cloned
    }

    /// Attach shared lazy backing to a newly created anonymous mapping.
    pub fn enable_shared_anonymous(&mut self) {
        self.shared_anonymous = Some(Arc::new(SharedAnonymousFrames::new()));
        self.shared_anonymous_offset = 0;
    }

    /// Move the start of a VMA forward while preserving its backing offset.
    pub fn trim_start(&mut self, new_start: VirtAddr) {
        let old_start = self.start_va();
        debug_assert!(new_start >= old_start);
        let delta_bytes = new_start
            .0
            .checked_sub(old_start.0)
            .expect("VMA start cannot move backwards");
        if self.map_file.is_some() {
            self.file_offset = self
                .file_offset
                .checked_add(delta_bytes)
                .expect("file-backed VMA offset overflow");
        }
        if self.shared_anonymous.is_some() {
            let delta_pages = new_start.floor().0.saturating_sub(old_start.floor().0);
            self.shared_anonymous_offset = self
                .shared_anonymous_offset
                .checked_add(delta_pages)
                .expect("shared anonymous backing offset overflow");
        }
        self.va_range.start = new_start;
    }

    /// Allocate one zero-filled page from an anonymous `MAP_SHARED` backing.
    pub fn allocate_shared_anonymous_frame(&self, vpn: VirtPageNum) -> Option<Arc<FrameTracker>> {
        let backing = self.shared_anonymous.as_ref()?;
        let relative = vpn.0.checked_sub(self.start_vpn().0)?;
        let page_index = self.shared_anonymous_offset.checked_add(relative)?;
        if let Some(frame) = backing.get(page_index) {
            return Some(frame);
        }

        // Allocate and initialize outside the shared lock. Concurrent faults
        // race only when publishing the candidate frame below.
        let candidate = Arc::new(frame_alloc()?);
        candidate.ppn.get_bytes_array().fill(0);
        backing.install_or_get(page_index, candidate)
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
        let ppn = if let Some(frame) = self.data_frames.get(&vpn) {
            frame.ppn
        } else {
            let Some(frame) = frame_alloc() else {
                log::error!(
                    "[OOM] user_map_area map_one failed: type={:?} range=[{:#x}, {:#x}) vpn={:#x} perm={:?} lazy={} cow={} resident_pages={}",
                    self.area_type,
                    self.start_va().0,
                    self.end_va().0,
                    vpn.0,
                    self.map_perm.bits(),
                    self.lazy_flag,
                    self.cow_flag,
                    self.data_frames.len()
                );
                crate::task::print_oom_snapshot();
                panic!("failed to allocate user map area frame");
            };
            let ppn = frame.ppn;

            // 清零物理页，避免残留垃圾数据（尤其是 bss 段）
            let zero_ptr = ((ppn.0 << 12) + VIRT_ADDR_START) as *mut u8;
            unsafe {
                core::ptr::write_bytes(zero_ptr, 0, PAGE_SIZE);
            }

            self.data_frames.insert(vpn, Arc::new(frame));
            ppn
        };

        // if vpn.0 == 0x10||vpn.0 == 0x11{
        //     error!("pagetable {:#x}", page_table.root().0);
        //     error!("vpn {:#x}", vpn.0);
        //     error!("ppn {:#x}", ppn.0);
        // }

        // let pte_flags = PTEFlags::from_bits(self.map_perm.bits()).unwrap();
        page_table.map_page(vpn, ppn, self.initial_mapping_flags(), MappingSize::Page4KB);
    }
    fn unmap_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
        // The PTE and its cached translation must disappear before the last
        // FrameTracker reference can recycle the physical page.
        page_table.unmap_page(vpn);
        self.data_frames.remove(&vpn);
    }
    fn map(&mut self, page_table: &mut PageTable) {
        let vpn_range = VPNRange::new(self.start_va().floor(), self.end_va().ceil());
        if !self.cow_flag {
            match self.area_type {
                UserMapAreaType::Elf
                | UserMapAreaType::TrapContext
                | UserMapAreaType::RtSigreturnTrampoline => {
                    for vpn in vpn_range {
                        // if self.start_va().0 == 0x10000{
                        //     error!("{:#x}", vpn.0);
                        // }
                        self.map_one(page_table, vpn);
                    }
                }
                _ => {
                    for vpn in vpn_range {
                        self.map_one(page_table, vpn);
                    }
                }
            }
        } else {
            for (&vpn, frame) in self.data_frames.iter() {
                self.map_cow(page_table, vpn, frame.ppn);
            }
        }
    }
    fn unmap(&mut self, page_table: &mut PageTable) {
        // let vpn_range = VPNRange::new(self.start_va().floor(), self.end_va().ceil());
        // for vpn in vpn_range {
        //     self.unmap_one(page_table, vpn);
        // }
        for vpn in self.vpn_range() {
            if self.data_frames.contains_key(&vpn) {
                self.unmap_one(page_table, vpn);
            }
        }
    }
}

///
pub trait COW {
    ///
    fn cow_flag(&self) -> bool;
    ///
    fn set_cow_flag(&mut self);
    ///
    fn clear_cow_flag(&mut self);
    ///
    fn map_cow(&self, page_table: &mut PageTable, vpn: VirtPageNum, ppn: PhysPageNum);
}

impl COW for UserMapArea {
    fn cow_flag(&self) -> bool {
        self.cow_flag
    }

    fn clear_cow_flag(&mut self) {
        self.cow_flag = false;
    }

    fn set_cow_flag(&mut self) {
        self.cow_flag = true;
    }

    fn map_cow(&self, page_table: &mut PageTable, vpn: VirtPageNum, ppn: PhysPageNum) {
        //info!("map_cow start vma:{:#x}, end vma:{:#x}",vpn.0,vpn.0 + PAGE_SIZE);
        // let pte_flags = PTEFlags::from(self.map_perm);
        page_table.map_page(
            vpn,
            ppn,
            cow_mapping_flags(self.map_perm),
            MappingSize::Page4KB,
        );
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
    Text,
    Rodata,
    Data,
    Bss,
    PhysMem,
    MemMappedReg,
    KernelStack,
}

#[allow(unused, missing_docs)]
impl KernelMapArea {
    pub fn new(
        start_va: VirtAddr,
        end_va: VirtAddr,
        map_type: MapType,
        map_perm: MapPermission,
        area_type: KernelAreaType,
    ) -> Self {
        let range = start_va..end_va;

        Self {
            va_range: start_va..end_va,
            data_frames: BTreeMap::new(),
            map_type: map_type,
            map_perm: map_perm,
            area_type: area_type,
        }
    }

    pub fn with_frames(
        start_va: VirtAddr,
        end_va: VirtAddr,
        map_type: MapType,
        map_perm: MapPermission,
        area_type: KernelAreaType,
        data_frames: BTreeMap<VirtPageNum, FrameTracker>,
    ) -> Self {
        Self {
            va_range: start_va..end_va,
            data_frames,
            map_type,
            map_perm,
            area_type,
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

    fn identical_map(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
        let ppn = PhysPageNum(vpn.0 & !(VIRT_ADDR_START >> 12));
        // println!("{}", flags.bits());
        page_table.map_page(vpn, ppn, (*self.perm()).into(), MappingSize::Page4KB);
    }

    /// Map an identity-backed kernel area while its page table is still inactive.
    ///
    /// The page table is flushed once when it is activated, so per-page TLB
    /// invalidation here would only add boot-time work and cannot invalidate a
    /// live translation.
    pub(crate) fn map_no_flush(&mut self, page_table: &mut PageTable) {
        assert_ne!(
            self.area_type,
            KernelAreaType::KernelStack,
            "inactive bulk mapping only supports identity-backed kernel areas"
        );
        let flags = self.map_perm.into();
        for vpn in VPNRange::new(self.start_vpn(), self.end_vpn()) {
            let ppn = PhysPageNum(vpn.0 & !(VIRT_ADDR_START >> 12));
            page_table.map_page_no_flush(vpn, ppn, flags, MappingSize::Page4KB);
        }
    }

    fn frame_map(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
        let ppn = if let Some(frame) = self.data_frames.get(&vpn) {
            frame.ppn
        } else {
            let Some(frame) = frame_alloc() else {
                log::error!(
                    "[OOM] kernel_map_area frame_map failed: type={:?} range=[{:#x}, {:#x}) vpn={:#x} perm={:?} resident_pages={}",
                    self.area_type,
                    self.start_va().0,
                    self.end_va().0,
                    vpn.0,
                    self.map_perm.bits(),
                    self.data_frames.len()
                );
                crate::task::print_oom_snapshot();
                panic!("failed to allocate kernel map area frame");
            };
            let ppn = frame.ppn;
            self.data_frames.insert(vpn, frame);
            ppn
        };
        page_table.map_page(vpn, ppn, (*self.perm()).into(), MappingSize::Page4KB);
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

    // #[cfg(target_arch = "riscv64")]
    fn map_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
        match self.area_type {
            KernelAreaType::Bss
            | KernelAreaType::Data
            | KernelAreaType::MemMappedReg
            | KernelAreaType::PhysMem
            | KernelAreaType::Rodata
            | KernelAreaType::Text => self.identical_map(page_table, vpn),

            KernelAreaType::KernelStack => self.frame_map(page_table, vpn),
        }
    }

    // #[cfg(target_arch = "loongarch64")]
    // fn map_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
    //     self.identical_map(page_table, vpn);
    // }

    // #[cfg(target_arch = "riscv64")]
    fn unmap_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
        match self.area_type {
            KernelAreaType::Bss
            | KernelAreaType::Data
            | KernelAreaType::MemMappedReg
            | KernelAreaType::PhysMem
            | KernelAreaType::Rodata
            | KernelAreaType::Text => page_table.unmap_page(vpn),

            KernelAreaType::KernelStack => {
                // Keep the stack frame alive until the old translation has
                // been invalidated.  Reversing this order lets a stale stack
                // TLB entry overwrite the frame allocator's free-list link.
                page_table.unmap_page(vpn);
                self.data_frames.remove(&vpn);
            }
        }
    }

    // #[cfg(target_arch = "loongarch64")]
    // fn unmap_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
    //     page_table.unmap_page(vpn);
    // }

    fn map(&mut self, page_table: &mut PageTable) {
        let vpn_range = VPNRange::new(self.start_vpn(), self.end_vpn());

        for vpn in vpn_range {
            self.map_one(page_table, vpn);
        }
    }

    fn unmap(&mut self, page_table: &mut PageTable) {
        let vpn_range = VPNRange::new(self.start_vpn(), self.end_vpn());
        for vpn in vpn_range {
            self.unmap_one(page_table, vpn);
        }
    }
}
