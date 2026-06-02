#![allow(missing_docs)]

use crate::error::{SysError, SysResult};
use crate::fs::vfs::inode::{InodeInner, InodeMode};
use crate::fs::vfs::{Dentry, DentryInner, File, FileInner, Inode, OpenFlags};
use crate::mm::{
    MapArea, MapPermission, MmapType, UserBuffer, UserMapArea, UserMapAreaType, UserVMSet,
    frame_alloc,
};
use crate::task::all_processes;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use lazy_static::lazy_static;
use log::warn;
use polyhal::common::FrameTracker;
use polyhal::consts::PAGE_SIZE;
use polyhal::pagetable::{MappingFlags, MappingSize, PTE, PTEFlags, TLB};
use polyhal::timer::current_time;
use polyhal::utils::addr::{VirtAddr, VirtPageNum};
use spin::{Mutex, MutexGuard};

#[derive(Clone)]
pub struct KsmPage {
    pub stable_id: usize,
    pub orig_perm: MapPermission,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct KsmMapping {
    pid: usize,
    vpn: VirtPageNum,
}

struct KsmStableNode {
    id: usize,
    hash: u64,
    mappings: BTreeSet<KsmMapping>,
}

#[derive(Clone, Copy)]
struct KsmTunables {
    run: usize,
    pages_to_scan: usize,
    sleep_millisecs: usize,
    max_page_sharing: usize,
    merge_across_nodes: usize,
    smart_scan: usize,
}

#[derive(Clone, Copy)]
struct KsmStats {
    pages_shared: usize,
    pages_sharing: usize,
    pages_unshared: usize,
    pages_volatile: usize,
    full_scans: usize,
    pages_skipped: usize,
}

struct KsmState {
    tunables: KsmTunables,
    stats: KsmStats,
    stable_nodes: Vec<KsmStableNode>,
    next_stable_id: usize,
    scan_generation: usize,
    scanned_generation: usize,
    stable_generation: usize,
    scanning: bool,
    next_scan_millis: u128,
}

#[derive(Clone, Copy)]
pub enum KsmSysfsKind {
    Run,
    PagesToScan,
    SleepMillisecs,
    MaxPageSharing,
    MergeAcrossNodes,
    SmartScan,
    PagesShared,
    PagesSharing,
    PagesUnshared,
    PagesVolatile,
    FullScans,
    PagesSkipped,
}

struct Candidate {
    pid: usize,
    vpn: VirtPageNum,
    hash: u64,
    frame: Arc<FrameTracker>,
}

#[derive(Default)]
struct CandidateGroup {
    chunks: Vec<Vec<Candidate>>,
    len: usize,
}

struct RemovedStableMapping {
    stable_id: usize,
    mapping: KsmMapping,
}

static KSM_INODE_ALLOC: AtomicUsize = AtomicUsize::new(0x6b73_6d00);
const KSM_CANDIDATE_CHUNK_LEN: usize = 256;
const KSM_REMOVED_BATCH_LEN: usize = 256;
const KSM_WRITE_FAULT_AROUND_PAGES: usize = 256;

lazy_static! {
    static ref KSM_STATE: Mutex<KsmState> = Mutex::new(KsmState::new());
}

impl KsmState {
    fn new() -> Self {
        Self {
            tunables: KsmTunables {
                run: 0,
                pages_to_scan: 100,
                sleep_millisecs: 20,
                max_page_sharing: 256,
                merge_across_nodes: 1,
                smart_scan: 1,
            },
            stats: KsmStats {
                pages_shared: 0,
                pages_sharing: 0,
                pages_unshared: 0,
                pages_volatile: 0,
                full_scans: 0,
                pages_skipped: 0,
            },
            stable_nodes: Vec::new(),
            next_stable_id: 1,
            scan_generation: 0,
            scanned_generation: 0,
            stable_generation: 0,
            scanning: false,
            next_scan_millis: 0,
        }
    }

    fn reset_stats(&mut self) {
        self.stats.pages_shared = 0;
        self.stats.pages_sharing = 0;
        self.stats.pages_unshared = 0;
        self.stats.pages_volatile = 0;
    }

    fn value(&self, kind: KsmSysfsKind) -> usize {
        let _stable_checksum = self.stable_checksum();
        match kind {
            KsmSysfsKind::Run => self.tunables.run,
            KsmSysfsKind::PagesToScan => self.tunables.pages_to_scan,
            KsmSysfsKind::SleepMillisecs => self.tunables.sleep_millisecs,
            KsmSysfsKind::MaxPageSharing => self.tunables.max_page_sharing,
            KsmSysfsKind::MergeAcrossNodes => self.tunables.merge_across_nodes,
            KsmSysfsKind::SmartScan => self.tunables.smart_scan,
            KsmSysfsKind::PagesShared => self.stable_nodes.len(),
            KsmSysfsKind::PagesSharing => self
                .stable_nodes
                .iter()
                .map(|node| node.mappings.len().saturating_sub(1))
                .sum(),
            KsmSysfsKind::PagesUnshared => self.stats.pages_unshared,
            KsmSysfsKind::PagesVolatile => self.stats.pages_volatile,
            KsmSysfsKind::FullScans => self.stats.full_scans,
            KsmSysfsKind::PagesSkipped => self.stats.pages_skipped,
        }
    }

    fn stable_checksum(&self) -> usize {
        self.stable_nodes.iter().fold(0usize, |acc, node| {
            let mappings = node
                .mappings
                .iter()
                .fold(0usize, |macc, mapping| macc ^ mapping.pid ^ mapping.vpn.0);
            acc ^ node.id ^ node.hash as usize ^ mappings
        })
    }

    fn mark_scan_needed(&mut self) {
        self.scan_generation = self.scan_generation.saturating_add(1);
    }

    fn finish_scan(&mut self, scan_generation: usize) {
        self.scanning = false;
        self.scanned_generation = scan_generation;
        self.stable_generation = scan_generation;
        self.next_scan_millis =
            current_time().as_millis() + self.tunables.sleep_millisecs as u128;
    }

    fn finish_clean_scan(&mut self) {
        self.scanning = false;
        self.stats.full_scans = self.stats.full_scans.saturating_add(1);
        self.next_scan_millis =
            current_time().as_millis() + self.tunables.sleep_millisecs as u128;
    }

    fn set_value(&mut self, kind: KsmSysfsKind, value: usize) -> SysResult<()> {
        match kind {
            KsmSysfsKind::Run => {
                if value > 2 {
                    return Err(SysError::EINVAL);
                }
                self.tunables.run = value;
            }
            KsmSysfsKind::PagesToScan => self.tunables.pages_to_scan = value.max(1),
            KsmSysfsKind::SleepMillisecs => self.tunables.sleep_millisecs = value,
            KsmSysfsKind::MaxPageSharing => self.tunables.max_page_sharing = value.max(2),
            KsmSysfsKind::MergeAcrossNodes => {
                self.tunables.merge_across_nodes = usize::from(value != 0)
            }
            KsmSysfsKind::SmartScan => self.tunables.smart_scan = usize::from(value != 0),
            KsmSysfsKind::PagesShared
            | KsmSysfsKind::PagesSharing
            | KsmSysfsKind::PagesUnshared
            | KsmSysfsKind::PagesVolatile
            | KsmSysfsKind::FullScans
            | KsmSysfsKind::PagesSkipped => return Err(SysError::EPERM),
        }
        Ok(())
    }
}

impl CandidateGroup {
    fn push(&mut self, candidate: Candidate) {
        if self
            .chunks
            .last()
            .map(|chunk| chunk.len() >= KSM_CANDIDATE_CHUNK_LEN)
            .unwrap_or(true)
        {
            self.chunks.push(Vec::new());
        }
        self.chunks.last_mut().unwrap().push(candidate);
        self.len += 1;
    }

    fn pop(&mut self) -> Option<Candidate> {
        loop {
            let chunk = self.chunks.last_mut()?;
            if let Some(candidate) = chunk.pop() {
                self.len -= 1;
                if chunk.is_empty() {
                    self.chunks.pop();
                }
                return Some(candidate);
            }
            self.chunks.pop();
        }
    }

    fn get(&self, mut idx: usize) -> Option<&Candidate> {
        if idx >= self.len {
            return None;
        }
        for chunk in self.chunks.iter() {
            if idx < chunk.len() {
                return chunk.get(idx);
            }
            idx -= chunk.len();
        }
        None
    }

    fn swap_remove(&mut self, idx: usize) -> Option<Candidate> {
        if idx >= self.len {
            return None;
        }
        let last = self.pop()?;
        if idx == self.len {
            return Some(last);
        }

        let mut remaining = idx;
        for chunk in self.chunks.iter_mut() {
            if remaining < chunk.len() {
                return Some(core::mem::replace(&mut chunk[remaining], last));
            }
            remaining -= chunk.len();
        }
        None
    }

    fn len(&self) -> usize {
        self.len
    }
}

pub fn mark_scan_needed() {
    let mut state = KSM_STATE.lock();
    state.mark_scan_needed();
    state.next_scan_millis = 0;
}

impl KsmSysfsKind {
    fn writable(self) -> bool {
        matches!(
            self,
            KsmSysfsKind::Run
                | KsmSysfsKind::PagesToScan
                | KsmSysfsKind::SleepMillisecs
                | KsmSysfsKind::MaxPageSharing
                | KsmSysfsKind::MergeAcrossNodes
                | KsmSysfsKind::SmartScan
        )
    }

    fn inode_mode(self) -> InodeMode {
        let read_perm = InodeMode::OWNER_READ | InodeMode::GROUP_READ | InodeMode::OTHER_READ;
        if self.writable() {
            InodeMode::FILE | read_perm | InodeMode::OWNER_WRITE
        } else {
            InodeMode::FILE | read_perm
        }
    }
}

pub fn read_sysfs(kind: KsmSysfsKind) -> String {
    let state = KSM_STATE.lock();
    format!("{}\n", state.value(kind))
}

pub fn write_sysfs(kind: KsmSysfsKind, input: &str) -> SysResult<usize> {
    let trimmed = input.trim();
    let value = parse_usize(trimmed)?;
    let len = input.len();

    let mut run_unmerge = false;
    {
        let mut state = KSM_STATE.lock();
        state.set_value(kind, value)?;
        if matches!(kind, KsmSysfsKind::Run) {
            run_unmerge = value == 2;
            if value == 1 {
                state.mark_scan_needed();
                state.next_scan_millis = 0;
            }
        } else if state.tunables.run == 1 {
            state.mark_scan_needed();
            state.next_scan_millis = 0;
        }
    }

    if run_unmerge {
        unmerge_all();
    }

    Ok(len)
}

fn parse_usize(input: &str) -> SysResult<usize> {
    if input.is_empty() {
        return Err(SysError::EINVAL);
    }
    let mut value = 0usize;
    for byte in input.bytes() {
        if !byte.is_ascii_digit() {
            return Err(SysError::EINVAL);
        }
        value = value
            .checked_mul(10)
            .and_then(|v| v.checked_add((byte - b'0') as usize))
            .ok_or(SysError::EINVAL)?;
    }
    Ok(value)
}

fn advance_if_running() {
    let scan_generation = {
        let mut state = KSM_STATE.lock();
        if state.tunables.run != 1 || state.scanning {
            None
        } else {
            state.scanning = true;
            Some(state.scan_generation)
        }
    };
    if let Some(scan_generation) = scan_generation {
        scan_once();
        let mut state = KSM_STATE.lock();
        state.finish_scan(scan_generation);
    }
}

pub fn run_until_quiescent() {
    if KSM_STATE.lock().tunables.run != 1 {
        return;
    }
    advance_if_running();
}

pub fn tick() {
    enum ScanWork {
        Full(usize),
        Clean,
    }

    let work = {
        let mut state = KSM_STATE.lock();
        if state.tunables.run != 1 || state.scanning {
            None
        } else {
            let now = current_time().as_millis();
            let has_new_work = state.scanned_generation != state.scan_generation;
            let timer_due = now >= state.next_scan_millis;
            if has_new_work {
                state.scanning = true;
                Some(ScanWork::Full(state.scan_generation))
            } else if timer_due {
                state.scanning = true;
                Some(ScanWork::Clean)
            } else {
                None
            }
        }
    };
    match work {
        Some(ScanWork::Full(scan_generation)) => {
            scan_once();
            let mut state = KSM_STATE.lock();
            state.finish_scan(scan_generation);
        }
        Some(ScanWork::Clean) => {
            let mut state = KSM_STATE.lock();
            state.finish_clean_scan();
        }
        None => {}
    }
}

pub fn mark_range(
    vm_set: &mut UserVMSet,
    start: usize,
    end: usize,
    mergeable: bool,
) -> SysResult<()> {
    split_range(vm_set, start, end);
    let mut unmerge_ranges = Vec::new();
    for area in vm_set.areas.iter_mut() {
        if area.end_va().0 <= start || area.start_va().0 >= end {
            continue;
        }
        if mergeable && !is_area_ksm_eligible(area) {
            continue;
        }
        if !mergeable {
            unmerge_ranges.push((area.start_vpn(), area.end_vpn()));
        }
        area.ksm_mergeable = mergeable;
    }
    for (start_vpn, end_vpn) in unmerge_ranges {
        unmerge_area_pages(vm_set, start_vpn, end_vpn);
    }
    let mut state = KSM_STATE.lock();
    state.mark_scan_needed();
    state.next_scan_millis = 0;
    Ok(())
}

pub fn split_range(vm_set: &mut UserVMSet, start: usize, end: usize) {
    if start >= end {
        return;
    }

    let mut idx = 0;
    while idx < vm_set.areas.len() {
        let area_start = vm_set.areas[idx].start_va().0;
        let area_end = vm_set.areas[idx].end_va().0;
        if start > area_start && start < area_end {
            split_area_at(vm_set, idx, start);
            continue;
        }
        if end > area_start && end < area_end {
            split_area_at(vm_set, idx, end);
            continue;
        }
        idx += 1;
    }
}

fn split_area_at(vm_set: &mut UserVMSet, idx: usize, split: usize) {
    let split_vpn = VirtAddr::from(split).floor();
    let mut right = UserMapArea::from_another(&vm_set.areas[idx]);
    right.data_frames = vm_set.areas[idx]
        .data_frames
        .iter()
        .filter(|(vpn, _)| **vpn >= split_vpn)
        .map(|(vpn, frame)| (*vpn, frame.clone()))
        .collect();
    right.ksm_pages = vm_set.areas[idx]
        .ksm_pages
        .iter()
        .filter(|(vpn, _)| **vpn >= split_vpn)
        .map(|(vpn, page)| (*vpn, page.clone()))
        .collect();

    vm_set.areas[idx].range_va_mut().end = VirtAddr::from(split);
    right.range_va_mut().start = VirtAddr::from(split);

    let left_start = vm_set.areas[idx].start_vpn();
    let left_end = vm_set.areas[idx].end_vpn();
    vm_set.areas[idx]
        .data_frames
        .retain(|vpn, _| *vpn >= left_start && *vpn < left_end);
    vm_set.areas[idx]
        .ksm_pages
        .retain(|vpn, _| *vpn >= left_start && *vpn < left_end);

    let right_start = right.start_vpn();
    let right_end = right.end_vpn();
    right
        .data_frames
        .retain(|vpn, _| *vpn >= right_start && *vpn < right_end);
    right
        .ksm_pages
        .retain(|vpn, _| *vpn >= right_start && *vpn < right_end);

    vm_set.areas.insert(idx + 1, right);
}

pub fn is_area_ksm_eligible(area: &crate::mm::UserMapArea) -> bool {
    area.areatype() == UserMapAreaType::Mmap
        && area.map_file.is_none()
        && area.flags == MmapType::MapPrivate
        && area.perm().contains(MapPermission::R)
        && area.perm().contains(MapPermission::U)
}

pub fn cleanup_area_metadata(area: &mut crate::mm::UserMapArea) {
    area.ksm_pages.clear();
    area.ksm_mergeable = false;
}

pub fn unmerge_area_pages(vm_set: &mut UserVMSet, start_vpn: VirtPageNum, end_vpn: VirtPageNum) {
    let pid = crate::task::current_process().getpid();
    unmerge_area_pages_for_pid(vm_set, Some(pid), start_vpn, end_vpn);
}

fn unmerge_area_pages_for_pid(
    vm_set: &mut UserVMSet,
    pid: Option<usize>,
    start_vpn: VirtPageNum,
    end_vpn: VirtPageNum,
) {
    let mut changed = false;
    for idx in 0..vm_set.areas.len() {
        let overlap_start = core::cmp::max(vm_set.areas[idx].start_vpn(), start_vpn);
        let overlap_end = core::cmp::min(vm_set.areas[idx].end_vpn(), end_vpn);
        if overlap_start >= overlap_end {
            continue;
        }
        loop {
            let pages: Vec<_> = vm_set.areas[idx]
                .ksm_pages
                .range(overlap_start..overlap_end)
                .map(|(vpn, _)| *vpn)
                .take(KSM_REMOVED_BATCH_LEN)
                .collect();
            if pages.is_empty() {
                break;
            }

            let mut removed = Vec::new();
            for vpn in pages {
                if unmerge_one_locked(vm_set, idx, vpn, true, pid, &mut removed).unwrap_or(false) {
                    changed = true;
                }
            }
            remove_mappings_from_stable(removed);
        }
    }
    if changed {
        TLB::flush_all();
    }
}

pub fn handle_ksm_write_fault(vm_set: &mut UserVMSet, va: VirtAddr) -> bool {
    let vpn = va.floor();
    let idx = match vm_set
        .areas
        .iter()
        .position(|area| area.range_va().contains(&va) && area.ksm_pages.contains_key(&vpn))
    {
        Some(idx) => idx,
        None => return false,
    };
    if !vm_set.areas[idx]
        .ksm_pages
        .get(&vpn)
        .map(|page| page.orig_perm.contains(MapPermission::W))
        .unwrap_or(false)
    {
        return false;
    }
    let pid = crate::task::current_process().getpid();
    let start_vpn = vpn;
    let end_vpn = VirtPageNum(
        start_vpn
            .0
            .saturating_add(KSM_WRITE_FAULT_AROUND_PAGES)
            .min(vm_set.areas[idx].end_vpn().0),
    );
    let pages: Vec<_> = vm_set.areas[idx]
        .ksm_pages
        .range(start_vpn..end_vpn)
        .filter_map(|(vpn, page)| {
            page.orig_perm
                .contains(MapPermission::W)
                .then_some(*vpn)
        })
        .collect();
    let mut ok = false;
    let mut removed = Vec::new();
    for vpn in pages {
        if unmerge_one_locked(vm_set, idx, vpn, true, Some(pid), &mut removed).unwrap_or(false) {
            ok = true;
        }
    }
    if ok {
        remove_mappings_from_stable(removed);
        TLB::flush_all();
        mark_scan_needed();
    }
    ok
}

pub fn unmerge_for_kernel_access(vm_set: &mut UserVMSet, va: VirtAddr) {
    let vpn = va.floor();
    let idx = match vm_set
        .areas
        .iter()
        .position(|area| area.range_va().contains(&va) && area.ksm_pages.contains_key(&vpn))
    {
        Some(idx) => idx,
        None => return,
    };
    let pid = crate::task::current_process().getpid();
    let mut removed = Vec::new();
    if unmerge_one_locked(vm_set, idx, vpn, true, Some(pid), &mut removed).unwrap_or(false) {
        remove_mappings_from_stable(removed);
        TLB::flush_vaddr(va);
        mark_scan_needed();
    }
}

pub fn cleanup_vmset(vm_set: &mut UserVMSet) {
    for area in vm_set.areas.iter_mut() {
        cleanup_area_metadata(area);
    }
}

pub fn cleanup_process(pid: usize, vm_set: &mut UserVMSet) {
    cleanup_vmset(vm_set);
    let mut state = KSM_STATE.lock();
    for node in state.stable_nodes.iter_mut() {
        let stale: Vec<_> = node
            .mappings
            .iter()
            .copied()
            .filter(|mapping| mapping.pid == pid)
            .collect();
        for mapping in stale {
            node.mappings.remove(&mapping);
        }
    }
    state.stable_nodes.retain(|node| node.mappings.len() > 1);
}

pub fn page_perm(area: &UserMapArea, vpn: VirtPageNum) -> MapPermission {
    if let Some(ksm_page) = area.ksm_pages.get(&vpn) {
        let mut perm = ksm_page.orig_perm;
        perm.remove(MapPermission::W);
        if !perm.contains(MapPermission::R) {
            perm.insert(MapPermission::R);
        }
        perm
    } else {
        *area.perm()
    }
}

pub fn unmerge_all() {
    let processes = all_processes();
    for process in processes {
        let pid = process.getpid();
        if let Some(mut inner) = process.try_inner_exclusive_access() {
            let mut ranges = Vec::new();
            for area in inner.vm_set.areas.iter() {
                if !area.ksm_pages.is_empty() {
                    ranges.push((area.start_vpn(), area.end_vpn()));
                }
            }
            for (start, end) in ranges {
                unmerge_area_pages_for_pid(&mut inner.vm_set, Some(pid), start, end);
            }
        }
    }

    let mut state = KSM_STATE.lock();
    state.stable_nodes.clear();
    state.reset_stats();
}

fn scan_once() {
    let processes = all_processes();
    let mut groups: BTreeMap<u64, CandidateGroup> = BTreeMap::new();

    for process in processes {
        let pid = process.getpid();
        let inner = match process.try_inner_exclusive_access() {
            Some(inner) => inner,
            None => continue,
        };
        let vm_set = &inner.vm_set;

        for area in vm_set.areas.iter() {
            if !area.ksm_mergeable || !is_area_ksm_eligible(area) {
                continue;
            }

            for (vpn, frame) in area.data_frames.iter() {
                let hash = hash_page(frame.ppn.get_bytes_array());
                groups.entry(hash).or_default().push(Candidate {
                    pid,
                    vpn: *vpn,
                    hash,
                    frame: frame.clone(),
                });
            }
        }
    }

    rebuild_from_groups(groups);
}

fn rebuild_from_groups(groups: BTreeMap<u64, CandidateGroup>) {
    let unmerged = unmerge_all_without_stats();

    let (mut next_id, max_page_sharing) = {
        let state = KSM_STATE.lock();
        (state.next_stable_id, state.tunables.max_page_sharing)
    };
    let mut stable_nodes = Vec::new();
    let mut pages_shared = 0usize;
    let mut pages_sharing = 0usize;
    let mut unshared = 0usize;

    for (_, mut hash_group) in groups {
        while let Some(first) = hash_group.pop() {
            let Some(frame) = frame_alloc() else {
                unshared = unshared.saturating_add(hash_group.len().saturating_add(1));
                break;
            };
            let shared_frame = Arc::new(frame);
            if !copy_candidate_to_frame(&first, &shared_frame) {
                unshared = unshared.saturating_add(1);
                continue;
            }

            let mut same = CandidateGroup::default();
            same.push(first);
            let mut idx = 0;
            let limit = max_page_sharing.max(2);
            while idx < hash_group.len() && same.len() < limit {
                if candidate_matches_frame(hash_group.get(idx).unwrap(), &shared_frame) {
                    same.push(hash_group.swap_remove(idx).unwrap());
                } else {
                    idx += 1;
                }
            }

            if same.len() < 2 {
                unshared = unshared.saturating_add(same.len());
                continue;
            }

            let id = next_id;
            next_id += 1;
            if let Some(node) = merge_group(id, &same, shared_frame) {
                pages_shared += 1;
                pages_sharing += node.mappings.len().saturating_sub(1);
                stable_nodes.push(node);
            } else {
                unshared = unshared.saturating_add(same.len());
            }
        }
    }

    let smart_scan = KSM_STATE.lock().tunables.smart_scan;
    let mut state = KSM_STATE.lock();
    state.next_stable_id = next_id;
    state.stable_nodes = stable_nodes;
    state.stats.pages_shared = pages_shared;
    state.stats.pages_sharing = pages_sharing;
    state.stats.pages_unshared = unshared;
    state.stats.pages_volatile = 0;
    state.stats.full_scans = state.stats.full_scans.saturating_add(1);
    if smart_scan != 0 {
        state.stats.pages_skipped = state.stats.pages_skipped.saturating_add(unshared);
    }
    drop(state);
    if unmerged || pages_shared > 0 {
        TLB::flush_all();
    }
}

fn unmerge_all_without_stats() -> bool {
    let processes = all_processes();
    let mut changed = false;
    for process in processes {
        let pid = process.getpid();
        if let Some(mut inner) = process.try_inner_exclusive_access() {
            for idx in 0..inner.vm_set.areas.len() {
                loop {
                    let batch: Vec<_> = inner.vm_set.areas[idx]
                        .ksm_pages
                        .keys()
                        .copied()
                        .take(KSM_REMOVED_BATCH_LEN)
                        .collect();
                    if batch.is_empty() {
                        break;
                    }

                    let mut removed = Vec::new();
                    for vpn in batch {
                        if unmerge_one_locked(
                            &mut inner.vm_set,
                            idx,
                            vpn,
                            false,
                            Some(pid),
                            &mut removed,
                        )
                        .unwrap_or(false)
                        {
                            changed = true;
                        }
                    }
                    remove_mappings_from_stable(removed);
                }
            }
        }
    }
    changed
}

fn copy_candidate_to_frame(candidate: &Candidate, frame: &Arc<FrameTracker>) -> bool {
    frame
        .ppn
        .get_bytes_array()
        .copy_from_slice(candidate.frame.ppn.get_bytes_array());
    true
}

fn candidate_matches_frame(candidate: &Candidate, frame: &Arc<FrameTracker>) -> bool {
    candidate.frame.ppn.get_bytes_array() == frame.ppn.get_bytes_array()
}

fn merge_group(
    id: usize,
    candidates: &CandidateGroup,
    shared_frame: Arc<FrameTracker>,
) -> Option<KsmStableNode> {
    let first = candidates.get(0)?;
    let mut mappings = BTreeSet::new();
    for chunk in candidates.chunks.iter() {
        for candidate in chunk {
            if map_candidate_to_shared(candidate, id, shared_frame.clone()) {
                mappings.insert(KsmMapping {
                    pid: candidate.pid,
                    vpn: candidate.vpn,
                });
            }
        }
    }

    if mappings.len() < 2 {
        unmerge_temporary_mappings(id, &mappings);
        return None;
    }

    Some(KsmStableNode {
        id,
        hash: first.hash,
        mappings,
    })
}

fn unmerge_temporary_mappings(stable_id: usize, mappings: &BTreeSet<KsmMapping>) {
    for mapping in mappings {
        let Some(process) = crate::task::pid2process(mapping.pid) else {
            continue;
        };
        let mut inner = match process.try_inner_exclusive_access() {
            Some(inner) => inner,
            None => continue,
        };
        let Some(idx) = inner.vm_set.areas.iter().position(|area| {
            area.ksm_pages
                .get(&mapping.vpn)
                .map(|page| page.stable_id == stable_id)
                .unwrap_or(false)
        }) else {
            continue;
        };
        let mut removed = Vec::new();
        if unmerge_one_locked(
            &mut inner.vm_set,
            idx,
            mapping.vpn,
            true,
            Some(mapping.pid),
            &mut removed,
        )
        .unwrap_or(false)
        {
            remove_mappings_from_stable(removed);
            TLB::flush_vaddr(VirtAddr::from(mapping.vpn));
        }
    }
}

fn map_candidate_to_shared(
    candidate: &Candidate,
    stable_id: usize,
    shared_frame: Arc<FrameTracker>,
) -> bool {
    let process = match crate::task::pid2process(candidate.pid) {
        Some(process) => process,
        None => return false,
    };
    let mut inner = match process.try_inner_exclusive_access() {
        Some(inner) => inner,
        None => return false,
    };
    let vm_set = &mut inner.vm_set;
    let idx = match vm_set
        .areas
        .iter()
        .position(|area| area.ksm_mergeable && area.vpn_range().contains(&candidate.vpn))
    {
        Some(idx) => idx,
        None => return false,
    };
    if !is_area_ksm_eligible(&vm_set.areas[idx]) {
        return false;
    }
    let Some(old_frame) = vm_set.areas[idx].data_frames.get(&candidate.vpn) else {
        return false;
    };
    if old_frame.ppn.get_bytes_array() != shared_frame.ppn.get_bytes_array() {
        let mut state = KSM_STATE.lock();
        state.stats.pages_volatile = state.stats.pages_volatile.saturating_add(1);
        return false;
    }

    let mut orig_perm = *vm_set.areas[idx].perm();
    if let Some(pte) = vm_set.page_table.find_pte(candidate.vpn) {
        if pte.is_valid() {
            orig_perm = map_permission_from_flags(MappingFlags::from(pte.flags()));
        }
    }
    let mut ro_perm = orig_perm;
    ro_perm.remove(MapPermission::W);
    if !ro_perm.contains(MapPermission::R) {
        ro_perm.insert(MapPermission::R);
    }

    vm_set.areas[idx]
        .data_frames
        .insert(candidate.vpn, shared_frame);
    vm_set.areas[idx].ksm_pages.insert(candidate.vpn, KsmPage {
        stable_id,
        orig_perm,
    });
    if let Some(pte) = vm_set.page_table.find_pte(candidate.vpn) {
        *pte = PTE::new(
            vm_set.areas[idx].data_frames[&candidate.vpn].ppn,
            PTEFlags::from(MappingFlags::from(ro_perm)),
        );
    } else {
        vm_set.page_table.map_page(
            candidate.vpn,
            vm_set.areas[idx].data_frames[&candidate.vpn].ppn,
            MappingFlags::from(ro_perm),
            MappingSize::Page4KB,
        );
    }
    true
}

fn unmerge_one_locked(
    vm_set: &mut UserVMSet,
    idx: usize,
    vpn: VirtPageNum,
    restore_writable: bool,
    pid: Option<usize>,
    removed: &mut Vec<RemovedStableMapping>,
) -> SysResult<bool> {
    if idx >= vm_set.areas.len() {
        return Err(SysError::EINVAL);
    }
    let ksm_page = match vm_set.areas[idx].ksm_pages.get(&vpn) {
        Some(ksm_page) => ksm_page.clone(),
        None => return Ok(false),
    };
    let frame = match vm_set.areas[idx].data_frames.get(&vpn) {
        Some(frame) => frame.clone(),
        None => {
            vm_set.areas[idx].ksm_pages.remove(&vpn);
            return Ok(false);
        }
    };
    let new_frame = Arc::new(frame_alloc().ok_or(SysError::ENOMEM)?);
    new_frame
        .ppn
        .get_bytes_array()
        .copy_from_slice(frame.ppn.get_bytes_array());
    vm_set.areas[idx].ksm_pages.remove(&vpn);
    vm_set.areas[idx].data_frames.insert(vpn, new_frame.clone());

    let mut perm = ksm_page.orig_perm;
    if restore_writable && ksm_page.orig_perm.contains(MapPermission::W) {
        perm.insert(MapPermission::W);
    }
    if let Some(pte) = vm_set.page_table.find_pte(vpn) {
        *pte = PTE::new(new_frame.ppn, PTEFlags::from(MappingFlags::from(perm)));
    } else {
        vm_set.page_table.map_page(
            vpn,
            new_frame.ppn,
            MappingFlags::from(perm),
            MappingSize::Page4KB,
        );
    }
    if let Some(pid) = pid {
        removed.push(RemovedStableMapping {
            stable_id: ksm_page.stable_id,
            mapping: KsmMapping { pid, vpn },
        });
    }
    Ok(true)
}

fn remove_mappings_from_stable(removed: Vec<RemovedStableMapping>) {
    if removed.is_empty() {
        return;
    }
    let mut by_stable_id: BTreeMap<usize, Vec<KsmMapping>> = BTreeMap::new();
    for removed_mapping in removed {
        by_stable_id
            .entry(removed_mapping.stable_id)
            .or_default()
            .push(removed_mapping.mapping);
    }

    let mut state = KSM_STATE.lock();
    for node in state.stable_nodes.iter_mut() {
        if let Some(mappings) = by_stable_id.get(&node.id) {
            for mapping in mappings {
                node.mappings.remove(mapping);
            }
        }
    }
    state.stable_nodes.retain(|node| node.mappings.len() > 1);
}

fn map_permission_from_flags(flags: MappingFlags) -> MapPermission {
    let mut perm = MapPermission::empty();
    if flags.contains(MappingFlags::R) {
        perm.insert(MapPermission::R);
    }
    if flags.contains(MappingFlags::W) {
        perm.insert(MapPermission::W);
    }
    if flags.contains(MappingFlags::X) {
        perm.insert(MapPermission::X);
    }
    if flags.contains(MappingFlags::U) {
        perm.insert(MapPermission::U);
    }
    if flags.contains(MappingFlags::G) {
        perm.insert(MapPermission::G);
    }
    if !flags.contains(MappingFlags::Cache) {
        perm.insert(MapPermission::MAT_NOCACHE);
    }
    perm
}

fn hash_page(bytes: &[u8; PAGE_SIZE]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let words = unsafe {
        core::slice::from_raw_parts(bytes.as_ptr() as *const u64, PAGE_SIZE / core::mem::size_of::<u64>())
    };
    for word in words {
        hash ^= *word;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
        hash ^= hash >> 32;
    }
    hash
}

pub struct KsmSysfsFile {
    inner: Mutex<FileInner>,
    kind: KsmSysfsKind,
}

impl KsmSysfsFile {
    pub fn new(dentry: Arc<dyn Dentry>, kind: KsmSysfsKind) -> Self {
        Self {
            inner: Mutex::new(FileInner {
                offset: 0,
                dentry,
                flags: OpenFlags::empty(),
            }),
            kind,
        }
    }
}

impl File for KsmSysfsFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        self.inner.lock()
    }

    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        self.kind.writable()
    }

    fn read(&self, mut buf: UserBuffer) -> SysResult<usize> {
        let mut inner = self.get_fileinner();
        let content = read_sysfs(self.kind);
        let data = content.as_bytes();
        let offset = inner.offset;
        if offset >= data.len() {
            return Ok(0);
        }
        let remaining = &data[offset..];
        let mut total = 0usize;
        for slice in buf.buffers.iter_mut() {
            let len = slice.len().min(remaining.len() - total);
            if len == 0 {
                break;
            }
            slice[..len].copy_from_slice(&remaining[total..total + len]);
            total += len;
        }
        inner.offset = offset + total;
        if let Some(inode) = inner.dentry.get_inode() {
            inode.set_size(data.len());
        }
        Ok(total)
    }

    fn write(&self, buf: UserBuffer) -> SysResult<usize> {
        if !self.writable() {
            return Err(SysError::EPERM);
        }
        let mut bytes = Vec::new();
        for slice in buf.buffers.iter() {
            bytes.extend_from_slice(slice);
        }
        let text = core::str::from_utf8(&bytes).map_err(|_| SysError::EINVAL)?;
        write_sysfs(self.kind, text)
    }

    fn open(&self) -> crate::error::SyscallResult {
        Ok(0)
    }

    fn release(&self) -> crate::error::SyscallResult {
        Ok(0)
    }
}

pub struct KsmSysfsDentry {
    inner: DentryInner,
    kind: KsmSysfsKind,
}

impl KsmSysfsDentry {
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>, kind: KsmSysfsKind) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(Arc::downgrade);
        Arc::new_cyclic(|_me| Self {
            inner: DentryInner::new(name, parent_weak),
            kind,
        })
    }
}

impl Dentry for KsmSysfsDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }

    fn name(&self) -> &str {
        &self.inner.name
    }

    fn open(self: Arc<Self>, _flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Ok(Arc::new(KsmSysfsFile::new(self.clone(), self.kind)))
    }
}

pub struct KsmSysfsInode {
    inner: InodeInner,
}

impl KsmSysfsInode {
    pub fn new(kind: KsmSysfsKind) -> Self {
        Self {
            inner: InodeInner::new(
                KSM_INODE_ALLOC.fetch_add(1, Ordering::Relaxed),
                0,
                kind.inode_mode(),
                0,
            ),
        }
    }
}

impl Inode for KsmSysfsInode {
    fn get_mode(&self) -> InodeMode {
        self.inner.mode
    }

    fn get_size(&self) -> usize {
        self.inner.size.load(Ordering::SeqCst)
    }

    fn set_size(&self, new_size: usize) {
        self.inner.size.store(new_size, Ordering::SeqCst);
    }

    fn get_ino(&self) -> usize {
        self.inner.ino
    }

    fn get_nlink(&self) -> usize {
        self.inner.nlink.load(Ordering::SeqCst)
    }

    fn get_rdev(&self) -> usize {
        self.inner.rdev.load(Ordering::Relaxed)
    }

    fn set_rdev(&self, rdev: usize) {
        self.inner.rdev.store(rdev, Ordering::Relaxed);
    }

    fn inc_nlink(&self) {
        self.inner.nlink.fetch_add(1, Ordering::SeqCst);
    }

    fn dec_nlink(&self) {
        self.inner.nlink.fetch_sub(1, Ordering::SeqCst);
    }

    fn get_atime(&self) -> (i64, i64) {
        (
            self.inner.atime_sec.load(Ordering::SeqCst),
            self.inner.atime_nsec.load(Ordering::SeqCst),
        )
    }

    fn set_atime(&self, sec: i64, nsec: i64) {
        self.inner.atime_sec.store(sec, Ordering::SeqCst);
        self.inner.atime_nsec.store(nsec, Ordering::SeqCst);
    }

    fn get_mtime(&self) -> (i64, i64) {
        (
            self.inner.mtime_sec.load(Ordering::SeqCst),
            self.inner.mtime_nsec.load(Ordering::SeqCst),
        )
    }

    fn set_mtime(&self, sec: i64, nsec: i64) {
        self.inner.mtime_sec.store(sec, Ordering::SeqCst);
        self.inner.mtime_nsec.store(nsec, Ordering::SeqCst);
    }

    fn get_ctime(&self) -> (i64, i64) {
        (
            self.inner.ctime_sec.load(Ordering::SeqCst),
            self.inner.ctime_nsec.load(Ordering::SeqCst),
        )
    }

    fn set_ctime(&self, sec: i64, nsec: i64) {
        self.inner.ctime_sec.store(sec, Ordering::SeqCst);
        self.inner.ctime_nsec.store(nsec, Ordering::SeqCst);
    }
}
